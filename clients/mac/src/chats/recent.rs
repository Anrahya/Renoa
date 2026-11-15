//! The chats the owner opened this run, most recent first, and the walk
//! Ctrl+Tab makes through them while Ctrl is held: each press goes one
//! further, and letting go of Ctrl makes the chat it stopped on the most
//! recent, as an app switcher does.

use renoa_rcp_wire::TaskId;

#[derive(Debug, Default)]
pub(super) struct Recent {
    order: Vec<TaskId>,
    /// Where in `order` the walk is, while Ctrl stays held.
    walk: Option<usize>,
}

impl Recent {
    /// The owner opened `task` some other way: it becomes the most recent,
    /// after the walk, if one was left unfinished, settles.
    pub(super) fn opened(&mut self, task: TaskId) {
        self.settle();
        self.order.retain(|opened| *opened != task);
        self.order.insert(0, task);
    }

    /// The chat one step further down the list, or back up it, wrapping
    /// around. A new walk starts from the most recent chat, so its first step
    /// is the one opened before it. `None` until two chats were opened.
    pub(super) fn step(&mut self, back: bool) -> Option<TaskId> {
        let len = self.order.len();
        if len < 2 {
            return None;
        }
        let from = self.walk.unwrap_or(0);
        let at = if back {
            (from + len - 1) % len
        } else {
            (from + 1) % len
        };
        self.walk = Some(at);
        Some(self.order[at])
    }

    /// Ctrl came up: the chat the walk stopped on becomes the most recent.
    pub(super) fn settle(&mut self) {
        if let Some(at) = self.walk.take() {
            let task = self.order.remove(at);
            self.order.insert(0, task);
        }
    }

    /// Drops the chats whose agents are gone, ending any walk.
    pub(super) fn keep(&mut self, listed: impl Fn(TaskId) -> bool) {
        self.walk = None;
        self.order.retain(|task| listed(*task));
    }
}

#[cfg(test)]
mod tests {
    use renoa_rcp_wire::TaskId;

    use super::Recent;

    fn opened(tasks: &[TaskId]) -> Recent {
        let mut recent = Recent::default();
        for task in tasks {
            recent.opened(*task);
        }
        recent
    }

    #[test]
    fn one_chat_has_nowhere_to_go() {
        let mut recent = opened(&[TaskId::new()]);
        assert_eq!(recent.step(false), None);
        assert_eq!(recent.step(true), None);
    }

    #[test]
    fn holding_ctrl_walks_back_through_what_was_opened() {
        let [a, b, c] = [TaskId::new(), TaskId::new(), TaskId::new()];
        let mut recent = opened(&[a, b, c]);
        assert_eq!(recent.step(false), Some(b));
        assert_eq!(recent.step(false), Some(a));
        assert_eq!(recent.step(false), Some(c), "wraps around");
        assert_eq!(recent.step(true), Some(a), "Shift steps back");
    }

    #[test]
    fn letting_go_makes_the_chosen_chat_the_most_recent() {
        let [a, b, c] = [TaskId::new(), TaskId::new(), TaskId::new()];
        let mut recent = opened(&[a, b, c]);
        recent.step(false);
        recent.step(false);
        recent.settle();
        assert_eq!(recent.step(false), Some(c), "a was settled on, then c");
        recent.settle();
        assert_eq!(recent.step(false), Some(a), "one press toggles back");
    }

    #[test]
    fn opening_another_way_ends_an_unfinished_walk() {
        let [a, b, c] = [TaskId::new(), TaskId::new(), TaskId::new()];
        let mut recent = opened(&[a, b]);
        assert_eq!(recent.step(false), Some(a));
        recent.opened(c);
        assert_eq!(recent.step(false), Some(a));
    }

    #[test]
    fn a_gone_agent_leaves_the_walk() {
        let [a, b, c] = [TaskId::new(), TaskId::new(), TaskId::new()];
        let mut recent = opened(&[a, b, c]);
        recent.keep(|task| task != b);
        assert_eq!(recent.step(false), Some(a));
    }
}
