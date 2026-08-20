use std::fmt::Write as _;

pub(crate) const MAX_TOOL_OUTPUT_BYTES: usize = 50 * 1024;
pub(crate) const MAX_TOOL_OUTPUT_LINES: usize = 2_000;
const NOTICE_RESERVE_BYTES: usize = 256;

pub(crate) struct HeadOutput {
    text: String,
    lines: usize,
}

impl HeadOutput {
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            lines: 0,
        }
    }

    pub(crate) fn remaining_bytes(&self) -> usize {
        MAX_TOOL_OUTPUT_BYTES
            .saturating_sub(NOTICE_RESERVE_BYTES)
            .saturating_sub(self.text.len())
    }

    pub(crate) fn push_line(&mut self, line: &str) -> bool {
        if self.lines == MAX_TOOL_OUTPUT_LINES || line.len() > self.remaining_bytes() {
            return false;
        }
        self.text.push_str(line);
        self.lines += 1;
        true
    }

    pub(crate) fn line_count(&self) -> usize {
        self.lines
    }

    pub(crate) fn finish(mut self, notice: Option<&str>) -> String {
        if let Some(notice) = notice {
            if !self.text.is_empty() && !self.text.ends_with('\n') {
                self.text.push('\n');
            }
            if !self.text.is_empty() {
                self.text.push('\n');
            }
            let available = MAX_TOOL_OUTPUT_BYTES.saturating_sub(self.text.len());
            if notice.len() <= available {
                self.text.push_str(notice);
            } else {
                let mut end = available;
                while end > 0 && !notice.is_char_boundary(end) {
                    end -= 1;
                }
                self.text.push_str(&notice[..end]);
            }
        }
        self.text
    }
}

pub(crate) fn truncation_notice(kind: &str, next_offset: usize) -> String {
    let mut notice = String::new();
    write!(
        notice,
        "[{kind} output capped at 50 KiB or 2,000 lines. Continue with offset={next_offset}.]"
    )
    .expect("writing to String cannot fail");
    notice
}

pub(crate) fn tail(text: &str, max_bytes: usize, max_lines: usize) -> (&str, bool) {
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }

    let byte_truncated = start > 0;
    let candidate = &text[start..];
    let mut line_start = 0;
    let mut separators = 0;
    for (index, _) in candidate.match_indices('\n').rev() {
        if index + 1 == candidate.len() {
            continue;
        }
        separators += 1;
        if separators == max_lines {
            line_start = index + 1;
            break;
        }
    }
    (&candidate[line_start..], byte_truncated || line_start > 0)
}

#[cfg(test)]
mod tests {
    use super::{HeadOutput, MAX_TOOL_OUTPUT_BYTES, tail};

    #[test]
    fn head_output_reserves_room_for_a_continuation_notice() {
        let mut output = HeadOutput::new();
        let oversized = "x".repeat(MAX_TOOL_OUTPUT_BYTES);

        assert!(!output.push_line(&oversized));
        let rendered = output.finish(Some("[continue]"));

        assert_eq!(rendered, "[continue]");
        assert!(rendered.len() <= MAX_TOOL_OUTPUT_BYTES);
    }

    #[test]
    fn tail_preserves_complete_utf8_and_the_last_lines() {
        let input = "first\nsecond\nthird ☃\n";

        let (rendered, truncated) = tail(input, 128, 2);

        assert_eq!(rendered, "second\nthird ☃\n");
        assert!(truncated);
    }
}
