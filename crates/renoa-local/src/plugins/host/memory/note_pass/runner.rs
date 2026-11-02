//! Running note passes: the timer's run over the conversations it is due
//! for, and the nightly sweep.
//!
//! Every decision is Mnem's, made again under the conversation's claim before
//! a pass is frozen, so a turn that ran meanwhile counts. A pass is settled
//! before its claim is released, so the person's surface, which waits for the
//! claim, finds the pass it cancelled settled, unless its command could not
//! run or its outcome could not be recorded; that pass is closed before the
//! person's message. One conversation's trouble stays with it: the run goes
//! on to the next.

use std::{path::Path, time::Instant};

use jiff::civil::Date;
use mnem::pass::{self, PassDecision};
use renoa_kernel::AgentId;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    super::{nightly::driver, pass::Pass},
    Kind, catalog_call, event, input,
    report::{Tally, trace_decision, trouble},
    schedule::{self, decide, ending_day, follow, zone_and_time},
    settle::{Outcome, Ran, Settled, close_stale},
    store::{self, Conversation},
};
use crate::{
    AutomationRun, LocalHost, LocalHostError, RunStatus, plugins::PluginError,
    plugins::host::triggers::Ended,
};

const OFF: &str = "renoa.memory is off for this agent, or it is no longer the Host's main agent.";

/// Runs one admitted run of the note pass timer as of `now_ms`: a daytime
/// pass in each conversation due to be asked, most unreviewed text first,
/// while Mnem says it is due, then compaction of the notebooks they noted in.
///
/// # Errors
///
/// None: every ending is recorded in its [`Ended`].
pub(crate) async fn run(
    host: &LocalHost,
    run: &AutomationRun,
    now_ms: i64,
) -> Result<Ended, LocalHostError> {
    let database = host.database().to_path_buf();
    let agent = run.agent_id;
    let day = catalog_call(&database, move |db| {
        if driver(db)? != Some(agent) {
            return Ok(None);
        }
        let (_, now) = zone_and_time(db, now_ms)?;
        Ok(Some(pass::day(now)))
    })
    .await;
    let day = match day {
        Ok(Some(day)) => day,
        Ok(None) => return Ok(ended(RunStatus::Skipped, OFF.to_owned(), None)),
        Err(error) => {
            clear_due(&database, agent, run.id, now_ms).await;
            return Ok(ended(RunStatus::Failed, error.to_string(), None));
        }
    };
    let mut review = Review::new(host, agent, run.id, now_ms, Kind::Daytime, day);
    let halted = review.daytime().await.err();
    clear_due(&database, agent, run.id, now_ms).await;
    // Afterwards, with this run's id, every notebook a pass noted in.
    let compaction = if review.tally.noted_in.is_empty() {
        None
    } else {
        let pass = Pass::new(host, agent, run.id, now_ms);
        Some(
            match catalog_call(&database, move |db| zone_and_time(db, now_ms)).await {
                Ok((_, local_time)) => pass.compact_all(&review.tally.noted_in, local_time).await,
                Err(error) => error.to_string(),
            },
        )
    };
    let mut output = review.tally.summary();
    if let Some(compaction) = compaction {
        output.push_str(" Compaction: ");
        output.push_str(&compaction);
    }
    if let Some(error) = &halted {
        output.push_str(" Stopped: ");
        output.push_str(&error.to_string());
    }
    let status = if halted.is_some() {
        RunStatus::Failed
    } else {
        RunStatus::Succeeded
    };
    Ok(ended(status, output, Some(review.tally.failed_calls)))
}

/// Before the nightly run of `run` compacts and dreams as of `now_ms`, a
/// sweep pass in every conversation of the main agent with unreviewed text,
/// counted toward the pass day that night ends. A conversation the person is
/// still in is left. Returns what was done, for the run's output, and the
/// passes' failed tool calls, unless nothing was; a sweep that fails never
/// stops the run.
pub(crate) async fn sweep(
    host: &LocalHost,
    run: &AutomationRun,
    now_ms: i64,
) -> Option<(String, u32)> {
    let database = host.database().to_path_buf();
    let agent = run.agent_id;
    let day = catalog_call(&database, move |db| {
        if driver(db)? != Some(agent) {
            return Ok(None);
        }
        Ok(Some(ending_day(db, now_ms)?))
    })
    .await;
    let day = match day {
        Ok(Some(day)) => day,
        // The nightly run reports that renoa.memory is off.
        Ok(None) => return None,
        Err(error) => return Some((format!("Memory passes stopped: {error}"), 0)),
    };
    let mut review = Review::new(host, agent, run.id, now_ms, Kind::Sweep, day);
    let halted = review.sweep().await.err();
    if halted.is_none() && review.tally.is_empty() {
        return None;
    }
    let mut output = review.tally.summary();
    if let Some(error) = halted {
        event(
            "warn",
            "memory_sweep_stopped",
            &json!({"agent_id": agent, "run_id": run.id, "error": error.to_string()}),
        );
        output.push_str(" Stopped: ");
        output.push_str(&error.to_string());
    }
    Some((output, review.tally.failed_calls))
}

/// One run's passes of one kind for one agent, counted toward `day`.
struct Review<'a> {
    host: &'a LocalHost,
    agent: AgentId,
    run: Uuid,
    now_ms: i64,
    kind: Kind,
    day: Date,
    tally: Tally,
}

/// When a visited conversation is asked again.
enum Next {
    /// As the daytime decision now stands.
    Reschedule,
    /// As this decision says.
    Follow(PassDecision),
    /// At this time, or not until new activity.
    At(Option<i64>),
    /// As it was.
    Keep,
}

/// What became of a conversation a pass was due in.
enum Visit {
    Settled(Settled),
    /// A turn holds it.
    Busy,
    /// Mnem decided otherwise under its claim: a turn ran since.
    Changed(PassDecision),
    /// Its command could not run and is unfinished; the next message or run
    /// closes it.
    Unsettled,
}

impl<'a> Review<'a> {
    fn new(
        host: &'a LocalHost,
        agent: AgentId,
        run: Uuid,
        now_ms: i64,
        kind: Kind,
        day: Date,
    ) -> Self {
        Self {
            host,
            agent,
            run,
            now_ms,
            kind,
            day,
            tally: Tally::default(),
        }
    }

    fn database(&self) -> &Path {
        self.host.database()
    }

    /// The conversations the timer is due for, in order.
    async fn daytime(&mut self) -> Result<(), PluginError> {
        self.recover().await?;
        let (agent, now_ms) = (self.agent, self.now_ms);
        let due = catalog_call(self.database(), move |db| {
            Ok(store::due(db, agent, now_ms)?)
        })
        .await?;
        for conversation in due {
            self.visit(conversation.session).await?;
        }
        Ok(())
    }

    /// Every conversation with unreviewed text, in order.
    async fn sweep(&mut self) -> Result<(), PluginError> {
        self.recover().await?;
        let agent = self.agent;
        let unreviewed =
            catalog_call(self.database(), move |db| Ok(store::unreviewed(db, agent)?)).await?;
        for conversation in unreviewed {
            self.visit(conversation.session).await?;
        }
        Ok(())
    }

    /// Closes every pass a stopped process left in a conversation no surface
    /// holds; a surface that holds one closes it before its message.
    async fn recover(&mut self) -> Result<(), PluginError> {
        let agent = self.agent;
        let left = catalog_call(self.database(), move |db| {
            let mut left = Vec::new();
            for frozen in store::unsettled_of(db, agent)? {
                left.extend(store::conversation(db, frozen.session)?);
            }
            Ok(left)
        })
        .await?;
        for conversation in left {
            let claim = CancellationToken::new();
            let opened = self
                .host
                .open_for_note_pass(agent, conversation.session, &conversation.workspace, &claim)
                .await;
            match opened {
                Ok(Some(session)) => {
                    if let Err(error) = close_stale(&session).await {
                        self.unclosed(conversation.session, &error);
                    }
                }
                Ok(None) => {}
                Err(error) => self.unopened(conversation.session, &error),
            }
        }
        Ok(())
    }

    /// Asks Mnem about `session`, runs its pass when it is due, and sets when
    /// it is asked again.
    async fn visit(&mut self, session: Uuid) -> Result<(), PluginError> {
        let Some((conversation, decision)) = self.decide(session).await? else {
            return Ok(());
        };
        self.trace(&conversation, decision, None);
        let next = match decision {
            PassDecision::Run => match self.pass(&conversation).await? {
                Visit::Settled(settled) => {
                    self.tally.count(conversation.principal, &settled);
                    match settled.outcome {
                        Outcome::Completed => Next::Reschedule,
                        // The surface that cancelled it notes its message, if
                        // it sends one; this covers one that sends none.
                        Outcome::Cancelled => self.later(),
                        // Not again until new activity, so a failing model is
                        // not asked over and over.
                        Outcome::Failed => Next::At(None),
                    }
                }
                Visit::Busy => {
                    self.tally.busy += 1;
                    let reason = if self.host.memory_runs_stopped() {
                        "the Host is stopping"
                    } else {
                        "a turn holds it"
                    };
                    self.trace(&conversation, decision, Some(reason));
                    // The turn's end asks again; this covers one that ended
                    // before the claim was tried.
                    self.later()
                }
                Visit::Changed(changed) => {
                    self.trace(&conversation, changed, None);
                    self.not_run(changed)
                }
                Visit::Unsettled => {
                    self.tally.unsettled += 1;
                    Next::At(None)
                }
            },
            decision => self.not_run(decision),
        };
        let now_ms = self.now_ms;
        catalog_call(self.database(), move |db| match next {
            Next::Reschedule => schedule::reschedule(db, session, now_ms),
            Next::Follow(decision) => follow(db, session, decision, now_ms),
            Next::At(at) => Ok(store::check_at(db, session, at)?),
            Next::Keep => Ok(()),
        })
        .await
    }

    /// Asks again once a quiet spell from now has passed.
    const fn later(&self) -> Next {
        Next::At(Some(
            self.now_ms.saturating_add(pass::QUIET_MINUTES * 60_000),
        ))
    }

    /// Counts a pass Mnem did not run; a sweep leaves the daytime timer as
    /// it is.
    fn not_run(&mut self, decision: PassDecision) -> Next {
        if matches!(decision, PassDecision::Wait { .. }) {
            self.tally.waiting += 1;
        } else {
            self.tally.skipped += 1;
        }
        match self.kind {
            Kind::Daytime => Next::Follow(decision),
            Kind::Sweep => Next::Keep,
        }
    }

    /// `session`'s conversation and Mnem's decision on its pass, while
    /// `renoa.memory` runs for this agent.
    async fn decide(
        &self,
        session: Uuid,
    ) -> Result<Option<(Conversation, PassDecision)>, PluginError> {
        let (agent, kind, day, now_ms) = (self.agent, self.kind, self.day, self.now_ms);
        catalog_call(self.database(), move |db| {
            if driver(db)? != Some(agent) {
                return Err(PluginError::Unavailable(OFF.to_owned()));
            }
            let Some(conversation) = store::conversation(db, session)? else {
                return Ok(None);
            };
            let decision = decide(db, &conversation, kind, day, now_ms)?;
            Ok(Some((conversation, decision)))
        })
        .await
    }

    fn trace(&self, conversation: &Conversation, decision: PassDecision, busy: Option<&str>) {
        trace_decision(
            (self.agent, self.run, self.kind),
            conversation,
            decision,
            busy,
        );
    }

    /// Claims `conversation`, decides again under the claim, and runs and
    /// settles its pass before releasing it.
    async fn pass(&self, conversation: &Conversation) -> Result<Visit, PluginError> {
        let claim = CancellationToken::new();
        let opened = self
            .host
            .open_for_note_pass(
                self.agent,
                conversation.session,
                &conversation.workspace,
                &claim,
            )
            .await;
        let session = match opened {
            Ok(Some(session)) => session,
            Ok(None) => return Ok(Visit::Busy),
            Err(error) => {
                self.unopened(conversation.session, &error);
                return Ok(Visit::Unsettled);
            }
        };
        // A pass left unfinished is closed first: the kernel admits nothing
        // else in the session until it is. One that cannot close stays with
        // its conversation, and the run goes on to the next.
        if let Err(error) = close_stale(&session).await {
            self.unclosed(conversation.session, &error);
            return Ok(Visit::Unsettled);
        }
        let Some((conversation, decision)) = self.decide(conversation.session).await? else {
            return Ok(Visit::Busy);
        };
        if decision != PassDecision::Run {
            return Ok(Visit::Changed(decision));
        }
        if claim.is_cancelled() {
            return Ok(Visit::Busy);
        }
        let at = session.pass_ref();
        let (agent, run, kind, day, now_ms) =
            (self.agent, self.run, self.kind, self.day, self.now_ms);
        let data_directory = at.data_directory.clone();
        let frozen = catalog_call(self.database(), move |db| {
            let (zone, now) = zone_and_time(db, now_ms)?;
            let prompt =
                input::message(db, &data_directory, agent, &conversation, &zone, (day, now))?;
            let command = store::freeze(
                db,
                run,
                &conversation,
                kind,
                &day.to_string(),
                &prompt,
                now_ms,
            )?;
            Ok((command, prompt, conversation.principal))
        })
        .await?;
        let (command, prompt, principal) = frozen;
        let ran = Ran {
            at: &at,
            command,
            principal,
            run: Some(run),
            kind: Some(kind),
            started: Instant::now(),
        };
        let outcome = match session
            .execute_note_pass(command, &prompt, principal, claim)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                ran.trace("unsettled", 0, 0, Some(&error.to_string()));
                return Ok(Visit::Unsettled);
            }
        };
        let settled = match ran.settle(&session, &outcome, now_ms).await {
            Ok(settled) => settled,
            Err(error) => {
                ran.trace("unsettled", 0, 0, Some(&error.to_string()));
                return Ok(Visit::Unsettled);
            }
        };
        if settled.outcome == Outcome::Cancelled {
            ran.cancelled(if self.host.memory_runs_stopped() {
                "the Host is stopping"
            } else {
                "the person came back"
            });
        }
        drop(session);
        Ok(Visit::Settled(settled))
    }

    /// A conversation whose pass a stopped process left could not be closed.
    fn unclosed(&self, session: Uuid, error: &PluginError) {
        trouble(
            "memory_pass_unclosed",
            (self.agent, self.run),
            session,
            error,
        );
    }

    /// A conversation could not be opened for its pass.
    fn unopened(&self, session: Uuid, error: &LocalHostError) {
        trouble(
            "memory_pass_unopened",
            (self.agent, self.run),
            session,
            error,
        );
    }
}

/// Stops asking again every conversation still due: one would bring the timer
/// back at once.
async fn clear_due(database: &Path, agent: AgentId, run: Uuid, now_ms: i64) {
    let cleared = catalog_call(database, move |db| Ok(store::clear_due(db, agent, now_ms)?)).await;
    let fields = match cleared {
        Ok(0) => return,
        Ok(cleared) => json!({"agent_id": agent, "run_id": run, "conversations": cleared}),
        Err(error) => json!({"agent_id": agent, "run_id": run, "error": error.to_string()}),
    };
    event("warn", "memory_pass_timer_cleared", &fields);
}

fn ended(status: RunStatus, output: String, failed_tool_calls: Option<u32>) -> Ended {
    Ended {
        status,
        output,
        failed_tool_calls,
    }
}
