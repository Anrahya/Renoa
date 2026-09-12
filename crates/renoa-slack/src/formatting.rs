// Slack's cumulative Markdown-block payload limit, not an agent output limit:
// https://docs.slack.dev/reference/block-kit/blocks/markdown-block/
pub(crate) const MESSAGE_CHARACTERS: usize = 12_000;

/// Produce complete delivery pages before persisting the outbox. Prefer whole
/// paragraphs and fenced blocks; oversized code blocks repeat their fences so
/// each Slack message renders independently. The stored turn result is untouched.
pub(crate) fn chunks(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec!["Done.".to_owned()];
    }
    let mut pages = Vec::new();
    let mut current = String::new();
    let mut characters = 0;
    for block in blocks(text) {
        let length = block.chars().count();
        if characters + length > MESSAGE_CHARACTERS && !current.is_empty() {
            pages.push(std::mem::take(&mut current));
            characters = 0;
        }
        if length > MESSAGE_CHARACTERS {
            pages.extend(split_block(block));
        } else {
            current.push_str(block);
            characters += length;
        }
    }
    if !current.is_empty() {
        pages.push(current);
    }
    pages
}

fn blocks(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let (mut start, mut end) = (0, 0);
    let mut fence = None;
    for line in text.split_inclusive('\n') {
        if let Some(marker) = fence {
            end += line.len();
            if closes(line, marker) {
                fence = None;
                result.push(&text[start..end]);
                start = end;
            }
        } else {
            if let Some(marker) = opens(line) {
                if start != end {
                    result.push(&text[start..end]);
                    start = end;
                }
                fence = Some(marker);
            }
            end += line.len();
            if fence.is_none() && line.trim().is_empty() {
                result.push(&text[start..end]);
                start = end;
            }
        }
    }
    if start != end {
        result.push(&text[start..end]);
    }
    result
}

fn opens(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let first = trimmed.as_bytes().first().copied()?;
    if !matches!(first, b'`' | b'~') {
        return None;
    }
    let length = trimmed.bytes().take_while(|byte| *byte == first).count();
    if length < 3 || (first == b'`' && trimmed[length..].contains('`')) {
        return None;
    }
    Some(&trimmed[..length])
}

fn closes(line: &str, marker: &str) -> bool {
    opens(line).is_some_and(|candidate| candidate.starts_with(marker) && line.trim() == candidate)
}

fn split_block(block: &str) -> Vec<String> {
    if let Some((opening, rest)) = block.split_once('\n')
        && let Some(marker) = opens(opening)
    {
        let closing = format!("\n{marker}\n");
        let overhead = opening.chars().count() + 1 + closing.chars().count();
        if overhead < MESSAGE_CHARACTERS {
            let last_start = rest.trim_end_matches('\n').rfind('\n').map_or(0, |i| i + 1);
            let code = if closes(&rest[last_start..], marker) {
                &rest[..last_start]
            } else {
                rest
            };
            return split_text(code, MESSAGE_CHARACTERS - overhead)
                .into_iter()
                .map(|part| format!("{opening}\n{part}{closing}"))
                .collect();
        }
    }
    split_text(block, MESSAGE_CHARACTERS)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn split_text(mut text: &str, limit: usize) -> Vec<&str> {
    let mut result = Vec::new();
    while let Some((end, _)) = text.char_indices().nth(limit) {
        let prefix = &text[..end];
        let boundary = prefix
            .rfind('\n')
            .map(|i| i + 1)
            .or_else(|| {
                prefix
                    .char_indices()
                    .rev()
                    .find(|(_, c)| c.is_whitespace())
                    .map(|(i, c)| i + c.len_utf8())
            })
            .unwrap_or(end);
        result.push(&text[..boundary]);
        text = &text[boundary..];
    }
    if !text.is_empty() {
        result.push(text);
    }
    result
}

#[cfg(test)]
mod tests;
