//! Safe link parsing and Markdown sanitization for agent replies: valid
//! `/s/<lowercase-uuid>` shared-file links and external web links stay
//! clickable, failed shares show inline errors, and everything else becomes
//! plain text.

use std::collections::BTreeSet;

use uuid::Uuid;

/// Returns the shared-file UUID when `url` is strictly `/s/<lowercase-uuid>`,
/// matching Android's `SHARED_FILE` rule.
pub(crate) fn shared_file_id(url: &str) -> Option<Uuid> {
    let rest = url.strip_prefix("/s/")?;
    let id = Uuid::parse_str(rest).ok()?;
    (rest == id.to_string()).then_some(id)
}

/// Normalizes an `http://`, `https://`, or bare `www.` destination for opening
/// in the owner's browser; returns `None` for any other URL or relative path.
pub(crate) fn web_link_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    if let Some(rest) = strip_ascii_prefix(trimmed, "http://") {
        return (!rest.is_empty() && !rest.starts_with('/')).then(|| format!("http://{rest}"));
    }
    if let Some(rest) = strip_ascii_prefix(trimmed, "https://") {
        return (!rest.is_empty() && !rest.starts_with('/')).then(|| format!("https://{rest}"));
    }
    if strip_ascii_prefix(trimmed, "www.").is_some_and(|rest| !rest.is_empty())
        && !trimmed.chars().any(char::is_whitespace)
    {
        return Some(format!("https://{trimmed}"));
    }
    None
}

/// `value` past `prefix`, matched without regard to ASCII case. A wide
/// character across the prefix's end is no match, never a split.
fn strip_ascii_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

/// Rewrites agent reply Markdown before passing it to `TextView::markdown`:
/// - valid `/s/<lowercase-uuid>` links listed in `failed_shares` become
///   `Couldn't open <label>` plain text;
/// - valid `/s/<lowercase-uuid>` links not in `failed_shares` and web links
///   (`http://`, `https://`, `www.`) stay clickable;
/// - all other inline links (including malformed `/s/...` links) become their
///   plain label text;
/// - inline code spans and fenced code blocks are left untouched.
pub(crate) fn prepare_reply_markdown(body: &str, failed_shares: &BTreeSet<Uuid>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_fence: Option<(u8, usize)> = None;

    for raw_line in body.split_inclusive('\n') {
        let line_no_nl = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line_no_nl.strip_suffix('\r').unwrap_or(line_no_nl);
        let nl = &raw_line[line.len()..];

        if let Some((fence_ch, fence_len)) = in_fence {
            out.push_str(raw_line);
            if closes_fence(line, fence_ch, fence_len) {
                in_fence = None;
            }
            continue;
        }
        if let Some(opened) = opens_fence(line) {
            in_fence = Some(opened);
            out.push_str(raw_line);
            continue;
        }
        rewrite_line(line, failed_shares, &mut out);
        out.push_str(nl);
    }
    out
}

fn opens_fence(line: &str) -> Option<(u8, usize)> {
    let trimmed = strip_up_to_three_spaces(line);
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let count = trimmed.bytes().take_while(|&b| b == first).count();
    if count < 3 {
        return None;
    }
    // Backtick info strings cannot contain backticks in CommonMark.
    if first == b'`' && trimmed[count..].contains('`') {
        return None;
    }
    Some((first, count))
}

fn closes_fence(line: &str, fence_ch: u8, min_len: usize) -> bool {
    let trimmed = strip_up_to_three_spaces(line);
    let count = trimmed.bytes().take_while(|&b| b == fence_ch).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

fn strip_up_to_three_spaces(line: &str) -> &str {
    let spaces = line.bytes().take(3).take_while(|&b| b == b' ').count();
    &line[spaces..]
}

fn rewrite_line(line: &str, failed_shares: &BTreeSet<Uuid>, out: &mut String) {
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            out.push('\\');
            let next_ch = line[i + 1..].chars().next().expect("non-empty tail");
            out.push(next_ch);
            i += 1 + next_ch.len_utf8();
            continue;
        }

        if bytes[i] == b'`' {
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if let Some(close_at) = find_closing_backticks(bytes, i + ticks, ticks) {
                out.push_str(&line[i..close_at + ticks]);
                i = close_at + ticks;
            } else {
                out.push_str(&line[i..i + ticks]);
                i += ticks;
            }
            continue;
        }

        // Preserve image syntax `![alt](url)` untouched.
        if bytes[i] == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            out.push_str("![");
            i += 2;
            continue;
        }

        if bytes[i] == b'['
            && let Some((end, label, raw_dest)) = parse_inline_link(line, i)
        {
            let dest = unescape_commonmark(raw_dest.trim());
            if let Some(share_id) = shared_file_id(&dest) {
                if failed_shares.contains(&share_id) {
                    let display = label.trim();
                    if display.is_empty() {
                        out.push_str("Couldn't open shared file");
                    } else {
                        out.push_str("Couldn't open ");
                        out.push_str(display);
                    }
                } else {
                    out.push_str(&line[i..end]);
                }
            } else if web_link_url(&dest).is_some() {
                out.push_str(&line[i..end]);
            } else {
                out.push_str(label);
            }
            i = end;
            continue;
        }

        let ch = line[i..].chars().next().expect("valid utf8 boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
}

fn find_closing_backticks(bytes: &[u8], mut pos: usize, target: usize) -> Option<usize> {
    while pos < bytes.len() {
        if bytes[pos] == b'`' {
            let run = bytes[pos..].iter().take_while(|&&b| b == b'`').count();
            if run == target {
                return Some(pos);
            }
            pos += run;
        } else {
            pos += 1;
        }
    }
    None
}

fn parse_inline_link(text: &str, start: usize) -> Option<(usize, &str, &str)> {
    let bytes = text.as_bytes();
    let label_end = find_label_end(text, start + 1)?;
    if bytes.get(label_end + 1) != Some(&b'(') {
        return None;
    }

    let label = &text[start + 1..label_end];
    let mut pos = label_end + 2;
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') {
        pos += 1;
    }

    let dest = if bytes.get(pos) == Some(&b'<') {
        pos += 1;
        let dest_start = pos;
        while pos < bytes.len() {
            match bytes[pos] {
                b'\\' if pos + 1 < bytes.len() => pos += 2,
                b'>' => break,
                b'<' | b'\n' | b'\r' => return None,
                _ => pos += 1,
            }
        }
        if bytes.get(pos) != Some(&b'>') {
            return None;
        }
        let d = &text[dest_start..pos];
        pos += 1;
        d
    } else {
        let dest_start = pos;
        let mut paren_depth = 0usize;
        while pos < bytes.len() {
            match bytes[pos] {
                b'\\' if pos + 1 < bytes.len() && bytes[pos + 1].is_ascii_punctuation() => {
                    pos += 2;
                }
                b'(' => {
                    paren_depth += 1;
                    pos += 1;
                }
                b')' => {
                    if paren_depth == 0 {
                        break;
                    }
                    paren_depth -= 1;
                    pos += 1;
                }
                b' ' | b'\t' | b'\n' | b'\r' => break,
                b if b.is_ascii_control() => return None,
                _ => pos += 1,
            }
        }
        if paren_depth != 0 {
            return None;
        }
        &text[dest_start..pos]
    };

    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') {
        pos += 1;
    }

    // Optional title ("...", '...', (...))
    if pos < bytes.len() && matches!(bytes[pos], b'"' | b'\'' | b'(') {
        let opener = bytes[pos];
        let closer = if opener == b'(' { b')' } else { opener };
        pos += 1;
        while pos < bytes.len() {
            match bytes[pos] {
                b'\\' if pos + 1 < bytes.len() => pos += 2,
                b if b == closer => {
                    pos += 1;
                    break;
                }
                _ => pos += 1,
            }
        }
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') {
            pos += 1;
        }
    }

    (bytes.get(pos) == Some(&b')')).then_some((pos + 1, label, dest))
}

fn find_label_end(text: &str, mut pos: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    while pos < bytes.len() {
        match bytes[pos] {
            b'\\' if pos + 1 < bytes.len() => pos += 2,
            b'`' => {
                let ticks = bytes[pos..].iter().take_while(|&&b| b == b'`').count();
                if let Some(close) = find_closing_backticks(bytes, pos + ticks, ticks) {
                    pos = close + ticks;
                } else {
                    pos += ticks;
                }
            }
            b'[' => {
                depth += 1;
                pos += 1;
            }
            b']' => {
                if depth == 0 {
                    return Some(pos);
                }
                depth -= 1;
                pos += 1;
            }
            _ => pos += 1,
        }
    }
    None
}

fn unescape_commonmark(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && let Some(&next) = chars.peek()
            && next.is_ascii_punctuation()
        {
            out.push(next);
            chars.next();
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_file_id_accepts_only_canonical_lowercase_uuid_under_slash_s() {
        let id = Uuid::new_v4();
        let valid = format!("/s/{id}");
        assert_eq!(shared_file_id(&valid), Some(id));

        assert_eq!(
            shared_file_id(&format!("/s/{}", id.to_string().to_uppercase())),
            None
        );
        assert_eq!(shared_file_id(&format!("/s/{id}/extra")), None);
        assert_eq!(shared_file_id(&format!("/s/{id}?ticket=abc")), None);
        assert_eq!(shared_file_id(&format!("/s/{}", id.simple())), None);
        assert_eq!(shared_file_id("/s/not-a-uuid"), None);
        assert_eq!(shared_file_id("https://renoa.live/s/not-a-uuid"), None);
    }

    #[test]
    fn web_link_url_allows_http_https_and_www_and_rejects_others() {
        assert_eq!(
            web_link_url("https://example.com/a"),
            Some("https://example.com/a".to_owned())
        );
        assert_eq!(
            web_link_url("HTTP://example.com"),
            Some("http://example.com".to_owned())
        );
        assert_eq!(
            web_link_url("www.example.com/docs"),
            Some("https://www.example.com/docs".to_owned())
        );
        assert_eq!(web_link_url("javascript:alert(1)"), None);
        assert_eq!(web_link_url("file:///etc/passwd"), None);
        assert_eq!(web_link_url("/local/path"), None);
    }

    #[test]
    fn a_destination_with_wide_characters_near_a_prefix_is_not_a_web_link() {
        for dest in ["aé€b", "httpé://x", "https:/€/x", "ww€.x"] {
            assert_eq!(web_link_url(dest), None, "{dest}");
            assert_eq!(
                prepare_reply_markdown(&format!("[x]({dest})"), &BTreeSet::new()),
                "x"
            );
        }
    }

    #[test]
    fn prepare_reply_markdown_preserves_valid_links_errors_failed_shares_and_strips_malformed() {
        let ok = Uuid::new_v4();
        let failed = Uuid::new_v4();
        let upper = ok.to_string().to_uppercase();
        let mut set = BTreeSet::new();
        set.insert(failed);

        let body = format!(
            "Here is [x.png](/s/{ok}), [broken.png](/s/{failed}), \
             [upper](/s/{upper}), [extra](/s/{ok}/more), [bad](/s/nope), \
             [docs](https://example.com), and `[keep](/s/bad)`.\n\n```md\n[fenced](/s/bad)\n```"
        );
        let prepared = prepare_reply_markdown(&body, &set);
        assert_eq!(
            prepared,
            format!(
                "Here is [x.png](/s/{ok}), Couldn't open broken.png, \
                 upper, extra, bad, [docs](https://example.com), and `[keep](/s/bad)`.\n\n```md\n[fenced](/s/bad)\n```"
            )
        );
    }
}
