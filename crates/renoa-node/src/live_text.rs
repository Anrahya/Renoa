//! The ephemeral text a running turn streams to attached surfaces.
//!
//! Only visible assistant text and bounded status lines distilled from
//! summarized reasoning leave the node: raw reasoning and live tool-call
//! fragments never do. Text is coalesced into at most one frame per
//! [`FLUSH_INTERVAL`]. Text that arrives inside the interval waits for the
//! message's next fragment of any kind once the interval has passed (the
//! fragment itself stays on the node), so a tool call that streams for seconds
//! after a sentence does not hold the sentence's tail back. A model that
//! stalls sends nothing until its next fragment. Whatever is still pending
//! when a message ends is dropped, because the turn's durable
//! `assistant_message` supersedes the live text. A message's frames count from
//! zero, so a surface can tell that it missed one.

use std::time::{Duration, Instant};

use renoa_agent::{AgentEvent, AssistantDelta};
use renoa_protocol::LiveActivity;

/// The least time between two text frames of one message.
pub(crate) const FLUSH_INTERVAL: Duration = Duration::from_millis(50);

/// Caps the retained reasoning tail so long thoughts stay bounded on node.
const MAX_REASONING_BUFFER_BYTES: usize = 32 * 1024;

#[derive(Debug, Default)]
pub(crate) struct LiveText {
    /// Whether a model request is in flight, so its text may stream.
    streaming: bool,
    next_sequence: u64,
    pending: String,
    last_sent: Option<Instant>,
    reasoning_index: Option<usize>,
    reasoning: String,
    summary_title: Option<String>,
    last_activity: Option<String>,
    last_activity_sent: Option<Instant>,
}

impl LiveText {
    /// The live frame one agent event completes, if any. Only visible text
    /// and a bounded line from summarized reasoning leave the node: raw
    /// reasoning and tool-call fragments never do. A context summary emits no
    /// message events at all (the loop's `renoa.agent.compaction` binding drops
    /// them), so it never streams.
    pub(crate) fn observe(&mut self, event: &AgentEvent, now: Instant) -> Option<LiveActivity> {
        match event {
            AgentEvent::ModelRequestStart { .. } => {
                self.request_started();
                None
            }
            AgentEvent::MessageStart { .. } => {
                self.message_started();
                None
            }
            AgentEvent::MessageUpdate {
                delta: AssistantDelta::Text { text },
                ..
            } => {
                self.clear_reasoning();
                self.push(text, now)
            }
            AgentEvent::MessageUpdate {
                content_index,
                delta: AssistantDelta::ReasoningSummary { text },
            } => self.push_reasoning(*content_index, text, now),
            // Raw reasoning and tool-call fragments are not sent. Flush any
            // buffered visible text or pending summary line once the interval
            // has passed.
            AgentEvent::MessageUpdate { .. } => self
                .push("", now)
                .or_else(|| self.flush_reasoning(now, false)),
            AgentEvent::MessageAbort => self.abort(),
            AgentEvent::ModelRequestEnd { .. } | AgentEvent::ModelRequestFailed { .. } => {
                self.request_ended();
                None
            }
            _ => None,
        }
    }

    /// A model request starts.
    fn request_started(&mut self) {
        self.streaming = true;
        self.message_started();
    }

    /// The request ended, completed or not.
    fn request_ended(&mut self) {
        self.streaming = false;
        self.message_started();
    }

    /// A new message starts, so its frames count from zero.
    fn message_started(&mut self) {
        self.next_sequence = 0;
        self.pending.clear();
        self.last_sent = None;
        self.clear_reasoning();
    }

    fn clear_reasoning(&mut self) {
        self.reasoning_index = None;
        self.reasoning.clear();
        self.summary_title = None;
        self.last_activity = None;
        self.last_activity_sent = None;
    }

    fn push_reasoning(
        &mut self,
        content_index: usize,
        text: &str,
        now: Instant,
    ) -> Option<LiveActivity> {
        if !self.streaming || text.is_empty() {
            return None;
        }
        if self.reasoning_index != Some(content_index) {
            self.reasoning.clear();
            self.summary_title = None;
            self.reasoning_index = Some(content_index);
        }
        self.reasoning.push_str(text);
        if let Some(title) = latest_markdown_title(&self.reasoning) {
            self.summary_title = Some(title);
        }
        bound_reasoning_tail(&mut self.reasoning);
        self.flush_reasoning(now, text.contains('\n'))
    }

    fn flush_reasoning(&mut self, now: Instant, force_line_end: bool) -> Option<LiveActivity> {
        if !self.streaming {
            return None;
        }
        let candidate = self
            .summary_title
            .clone()
            .or_else(|| fallback_summary_line(&self.reasoning))?;
        if self.last_activity.as_deref() == Some(candidate.as_str()) {
            return None;
        }
        if !force_line_end
            && self
                .last_activity_sent
                .is_some_and(|sent| now.saturating_duration_since(sent) < FLUSH_INTERVAL)
        {
            return None;
        }
        self.last_activity_sent = Some(now);
        self.last_activity = Some(candidate.clone());
        Some(LiveActivity::Activity { text: candidate })
    }

    /// One visible text delta arrived at `now`, or a fragment that is not text
    /// (empty `text`). Returns the frame that is due, carrying everything
    /// pending since the last one.
    fn push(&mut self, text: &str, now: Instant) -> Option<LiveActivity> {
        if !self.streaming {
            return None;
        }
        self.pending.push_str(text);
        if self.pending.is_empty() {
            return None;
        }
        if self
            .last_sent
            .is_some_and(|sent| now.saturating_duration_since(sent) < FLUSH_INTERVAL)
        {
            return None;
        }
        self.last_sent = Some(now);
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        Some(LiveActivity::TextDelta {
            sequence,
            text: std::mem::take(&mut self.pending),
        })
    }

    /// The message was abandoned. Returns a reset when a surface has been
    /// shown any live text or summary activity for it.
    fn abort(&mut self) -> Option<LiveActivity> {
        let shown = self.next_sequence > 0 || self.last_activity.is_some();
        self.message_started();
        shown.then_some(LiveActivity::TextReset)
    }
}

fn bound_reasoning_tail(buffer: &mut String) {
    if buffer.len() <= MAX_REASONING_BUFFER_BYTES {
        return;
    }
    if let Some((start, _)) = buffer.rmatch_indices("\n\n").find(|(idx, _)| {
        buffer[idx + 2..]
            .lines()
            .any(|line| !clean_markdown_line(line).is_empty())
    }) {
        buffer.drain(..start + 2);
    } else if let Some((last_sep, _)) = buffer.rmatch_indices("\n\n").next() {
        // Keep the trailing paragraph break when an earlier block blew past
        // the buffer cap so the next summary paragraph is not glued onto it.
        buffer.drain(..last_sep);
    }
    if buffer.len() > MAX_REASONING_BUFFER_BYTES {
        let end = buffer
            .char_indices()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .take_while(|&end| end <= MAX_REASONING_BUFFER_BYTES)
            .last()
            .unwrap_or(0);
        buffer.truncate(end);
    }
}

fn latest_markdown_title(raw: &str) -> Option<String> {
    raw.lines().rev().find_map(extract_markdown_title)
}

fn extract_markdown_title(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix('#')
        && rest.starts_with(['#', ' ', '\t'])
    {
        return fit_summary_word_boundary(&clean_markdown_line(trimmed));
    }
    let is_double_bold = (trimmed.starts_with("**")
        && trimmed.ends_with("**")
        && trimmed.len() > 4
        && !trimmed[2..trimmed.len() - 2].contains("**"))
        || (trimmed.starts_with("__")
            && trimmed.ends_with("__")
            && trimmed.len() > 4
            && !trimmed[2..trimmed.len() - 2].contains("__"));
    if is_double_bold {
        return fit_summary_word_boundary(&clean_markdown_line(trimmed));
    }
    None
}

fn fallback_summary_line(raw: &str) -> Option<String> {
    let block = raw.rsplit("\n\n").find(|part| {
        part.lines()
            .any(|line| !clean_markdown_line(line).is_empty())
    })?;
    let first_line = block
        .lines()
        .map(clean_markdown_line)
        .find(|line| !line.is_empty())?;
    fit_summary_word_boundary(&first_line)
}

fn fit_summary_word_boundary(cleaned: &str) -> Option<String> {
    let stripped = renoa_protocol::strip_format_chars(cleaned);
    let trimmed = stripped.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    if trimmed.len() <= renoa_protocol::MAX_ACTIVITY_BYTES {
        return renoa_protocol::normalize_activity(trimmed);
    }
    let end = trimmed
        .char_indices()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .take_while(|&end| end <= renoa_protocol::MAX_ACTIVITY_BYTES)
        .last()
        .unwrap_or(0);
    let prefix = &trimmed[..end];
    let cut = if trimmed[end..].starts_with(char::is_whitespace) {
        prefix
    } else if let Some(ws) = prefix.rfind(char::is_whitespace) {
        let word_bound = prefix[..ws].trim_end();
        if word_bound.is_empty() {
            prefix
        } else {
            word_bound
        }
    } else {
        prefix
    };
    renoa_protocol::normalize_activity(cut)
}

fn clean_markdown_line(line: &str) -> String {
    let spaced = line.replace('\t', " ");
    let no_heading = spaced.trim().trim_start_matches(['#', '>']).trim_start();
    let no_bullet = no_heading
        .strip_prefix("- ")
        .or_else(|| no_heading.strip_prefix("* "))
        .unwrap_or(no_heading);
    no_bullet
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim_matches('*')
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests;
