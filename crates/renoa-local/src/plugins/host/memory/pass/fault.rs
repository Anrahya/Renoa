//! Crashes a run at one point, once, as a process that stopped there would,
//! or runs a hook there once, as another process acting then would.

use std::{collections::BTreeSet, sync::Mutex};

use uuid::Uuid;

use super::{Halt, JobKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Point {
    /// The job is frozen; its command is not admitted.
    Frozen(JobKind),
    /// The model answered; nothing is applied.
    Answered(JobKind),
    /// The answer is applied; the job has no receipt.
    Applied(JobKind),
    /// A run that could not go on is counted; the agent has not moved on.
    Counted,
}

type Hook = Box<dyn FnOnce() + Send>;

static ARMED: Mutex<BTreeSet<(Uuid, Point)>> = Mutex::new(BTreeSet::new());
static HOOKS: Mutex<Vec<(Uuid, Point, Hook)>> = Mutex::new(Vec::new());

pub(crate) fn arm(run: Uuid, point: Point) {
    ARMED.lock().expect("fault lock").insert((run, point));
}

pub(crate) fn on(run: Uuid, point: Point, hook: impl FnOnce() + Send + 'static) {
    HOOKS
        .lock()
        .expect("hook lock")
        .push((run, point, Box::new(hook)));
}

pub(super) fn hit(run: Uuid, point: Point) -> Result<(), Halt> {
    if ARMED.lock().expect("fault lock").remove(&(run, point)) {
        return Err(Halt::Crashed);
    }
    let hook = {
        let mut hooks = HOOKS.lock().expect("hook lock");
        hooks
            .iter()
            .position(|(at, armed, _)| (*at, *armed) == (run, point))
            .map(|index| hooks.remove(index).2)
    };
    if let Some(hook) = hook {
        hook();
    }
    Ok(())
}
