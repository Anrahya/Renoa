use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use thiserror::Error;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
enum Activity {
    Idle,
    Running(Arc<RunControl>),
}

struct RunControl {
    cancellation: CancellationToken,
}

pub(crate) struct AgentControl {
    activity: watch::Sender<Activity>,
    queues: Arc<PendingQueues>,
}

impl AgentControl {
    pub(crate) fn new(queue_limit: usize) -> Self {
        let (activity, _) = watch::channel(Activity::Idle);
        Self {
            activity,
            queues: Arc::new(PendingQueues::new(queue_limit)),
        }
    }

    pub(crate) fn handle(&self) -> AgentHandle {
        AgentHandle {
            activity: self.activity.subscribe(),
            queues: Arc::clone(&self.queues),
        }
    }

    pub(crate) fn start(&self) -> RunGuard {
        RunGuard::start(&self.activity)
    }

    pub(crate) fn set_queue_limit(&self, limit: usize) -> Result<(), usize> {
        self.queues.set_limit(limit)
    }

    pub(crate) fn take_steering(&self, mode: QueueMode) -> Vec<String> {
        self.queues.drain(QueueKind::Steering, mode)
    }

    pub(crate) fn has_steering(&self) -> bool {
        !self.queues.is_empty(QueueKind::Steering)
    }

    pub(crate) fn take_follow_up(&self, mode: QueueMode) -> Vec<String> {
        self.queues.drain(QueueKind::FollowUp, mode)
    }

    pub(crate) fn has_follow_up(&self) -> bool {
        !self.queues.is_empty(QueueKind::FollowUp)
    }

    pub(crate) fn clear_queues(&self) {
        self.queues.clear_all();
    }
}

impl Drop for AgentControl {
    fn drop(&mut self) {
        self.queues.close();
    }
}

/// Controls and observes the current run without borrowing its [`Agent`](crate::Agent).
#[derive(Clone)]
pub struct AgentHandle {
    activity: watch::Receiver<Activity>,
    queues: Arc<PendingQueues>,
}

impl AgentHandle {
    /// Returns whether this Agent is currently settling a prompt.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(*self.activity.borrow(), Activity::Running(_))
    }

    /// Requests cancellation of the current prompt, if one exists.
    pub fn abort(&self) {
        if let Activity::Running(run) = self.activity.borrow().clone() {
            run.cancellation.cancel();
        }
    }

    /// Queues user input for the next model turn boundary.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Full`] when the shared queue limit has been
    /// reached, or [`QueueError::Closed`] after the owning Agent is dropped.
    pub fn steer(&self, text: impl Into<String>) -> Result<(), QueueError> {
        self.queues.push(QueueKind::Steering, text.into())
    }

    /// Queues user input to run after the agent would otherwise stop.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Full`] when the shared queue limit has been
    /// reached, or [`QueueError::Closed`] after the owning Agent is dropped.
    pub fn follow_up(&self, text: impl Into<String>) -> Result<(), QueueError> {
        self.queues.push(QueueKind::FollowUp, text.into())
    }

    #[must_use]
    pub fn has_queued_messages(&self) -> bool {
        !self.queues.is_empty(QueueKind::Steering) || !self.queues.is_empty(QueueKind::FollowUp)
    }

    /// Removes pending steering without changing follow-ups.
    pub fn clear_steering(&self) {
        self.queues.clear(QueueKind::Steering);
    }

    /// Removes pending follow-ups without changing steering.
    pub fn clear_follow_ups(&self) {
        self.queues.clear(QueueKind::FollowUp);
    }

    /// Removes all pending steering and follow-up input.
    pub fn clear_all_queued_messages(&self) {
        self.queues.clear_all();
    }

    /// Waits for the prompt active when this method is called to settle.
    pub async fn wait_for_idle(&self) {
        let mut activity = self.activity.clone();
        let current_run = match activity.borrow().clone() {
            Activity::Idle => return,
            Activity::Running(run) => run,
        };
        loop {
            if activity.changed().await.is_err() {
                return;
            }
            match activity.borrow_and_update().clone() {
                Activity::Running(run) if Arc::ptr_eq(&run, &current_run) => {}
                Activity::Idle | Activity::Running(_) => return,
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QueueError {
    #[error("message queue is full; its limit is {limit}")]
    Full { limit: usize },
    #[error("message queue is closed because its agent was dropped")]
    Closed,
}

/// Controls how many messages one queue poll claims.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueueMode {
    /// Claim only the oldest message.
    #[default]
    OneAtATime,
    /// Claim every message currently in that queue, preserving FIFO order.
    All,
}

struct PendingQueues {
    state: Mutex<QueueState>,
}

struct QueueState {
    steering: VecDeque<String>,
    follow_ups: VecDeque<String>,
    limit: usize,
    closed: bool,
}

#[derive(Clone, Copy)]
enum QueueKind {
    Steering,
    FollowUp,
}

impl PendingQueues {
    fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                steering: VecDeque::new(),
                follow_ups: VecDeque::new(),
                limit,
                closed: false,
            }),
        }
    }

    fn push(&self, kind: QueueKind, text: String) -> Result<(), QueueError> {
        let mut state = self
            .state
            .lock()
            .expect("message queue lock must not be poisoned");
        if state.closed {
            return Err(QueueError::Closed);
        }
        if state.steering.len() + state.follow_ups.len() >= state.limit {
            return Err(QueueError::Full { limit: state.limit });
        }
        match kind {
            QueueKind::Steering => state.steering.push_back(text),
            QueueKind::FollowUp => state.follow_ups.push_back(text),
        }
        Ok(())
    }

    fn drain(&self, kind: QueueKind, mode: QueueMode) -> Vec<String> {
        let mut state = self
            .state
            .lock()
            .expect("message queue lock must not be poisoned");
        let messages = match kind {
            QueueKind::Steering => &mut state.steering,
            QueueKind::FollowUp => &mut state.follow_ups,
        };
        match mode {
            QueueMode::OneAtATime => messages.pop_front().into_iter().collect(),
            QueueMode::All => messages.drain(..).collect(),
        }
    }

    fn is_empty(&self, kind: QueueKind) -> bool {
        let state = self
            .state
            .lock()
            .expect("message queue lock must not be poisoned");
        match kind {
            QueueKind::Steering => state.steering.is_empty(),
            QueueKind::FollowUp => state.follow_ups.is_empty(),
        }
    }

    fn clear(&self, kind: QueueKind) {
        let mut state = self
            .state
            .lock()
            .expect("message queue lock must not be poisoned");
        match kind {
            QueueKind::Steering => state.steering.clear(),
            QueueKind::FollowUp => state.follow_ups.clear(),
        }
    }

    fn clear_all(&self) {
        let mut state = self
            .state
            .lock()
            .expect("message queue lock must not be poisoned");
        state.steering.clear();
        state.follow_ups.clear();
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .expect("message queue lock must not be poisoned");
        state.closed = true;
        state.steering.clear();
        state.follow_ups.clear();
    }

    fn set_limit(&self, limit: usize) -> Result<(), usize> {
        let mut state = self
            .state
            .lock()
            .expect("message queue lock must not be poisoned");
        let pending = state.steering.len() + state.follow_ups.len();
        if pending > limit {
            return Err(pending);
        }
        state.limit = limit;
        Ok(())
    }
}

pub(crate) struct RunGuard {
    activity: watch::Sender<Activity>,
    run: Arc<RunControl>,
}

impl RunGuard {
    fn start(activity: &watch::Sender<Activity>) -> Self {
        let run = Arc::new(RunControl {
            cancellation: CancellationToken::new(),
        });
        activity.send_replace(Activity::Running(run.clone()));
        Self {
            activity: activity.clone(),
            run,
        }
    }

    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.run.cancellation.clone()
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.run.cancellation.cancel();
        self.activity.send_replace(Activity::Idle);
    }
}
