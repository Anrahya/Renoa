use super::*;

#[test]
fn normal_markdown_reaches_slack_without_rewriting_its_syntax() {
    let text = "**What it is**\n- First\n- Second\n\n[API](https://example.com)\n\n```rust\nlet x = \"**literal**\";\n```\n\nEnd.";
    assert_eq!(chunks(text), [text]);
    assert_eq!(chunks(""), ["Done."]);
}

#[test]
fn paragraphs_lists_and_fences_that_fit_stay_together() {
    let first = format!("{}\n\n", "a".repeat(8000));
    let second = format!("**List**\n- {}\n- two\n\n", "b".repeat(6000));
    let code = format!("```rust\n{}\n```\n", "x".repeat(8000));
    let text = format!("{first}{second}{code}");
    let pages = chunks(&text);
    assert_eq!(pages, [first, second, code]);
    assert_eq!(pages.concat(), text);
}

#[test]
fn oversized_unicode_text_is_complete_and_within_slacks_payload_limit() {
    let text = "🦀".repeat(MESSAGE_CHARACTERS * 3 + 1);
    let pages = chunks(&text);
    assert_eq!(pages.len(), 4);
    assert_eq!(pages.concat(), text);
    assert!(
        pages
            .iter()
            .all(|p| p.chars().count() <= MESSAGE_CHARACTERS)
    );
    assert_eq!(chunks(&"x".repeat(MESSAGE_CHARACTERS)).len(), 1);
}

#[test]
fn oversized_fenced_code_repeats_language_and_preserves_every_code_character() {
    for (opening, marker, ending) in [
        ("```rust\n", "```", "```\n"),
        ("~~~~text\n", "~~~~", "~~~~\n"),
        ("```\n", "```", ""),
    ] {
        let code = format!(
            "let text = \"🦀 <tag> **stars**\";\n{}\n``` inside code\n",
            "x".repeat(MESSAGE_CHARACTERS * 2)
        );
        let text = format!("{opening}{code}{ending}");
        let pages = chunks(&text);
        assert!(pages.len() >= 3);
        let closing = format!("\n{marker}\n");
        let reconstructed: String = pages
            .iter()
            .map(|page| {
                assert!(page.chars().count() <= MESSAGE_CHARACTERS);
                page.strip_prefix(opening)
                    .expect("language retained")
                    .strip_suffix(&closing)
                    .expect("independent closed fence")
            })
            .collect();
        assert_eq!(reconstructed, code);
    }
}

#[test]
fn oversized_fence_metadata_still_delivers_all_text_without_truncation() {
    let text = format!("```{}\ncode\n```", "x".repeat(MESSAGE_CHARACTERS));
    assert_eq!(chunks(&text).concat(), text);
}

#[test]
fn paragraphs_inside_code_are_not_treated_as_message_boundaries() {
    let text = "Before\n\n````md\n# Title\n\n```rust\nlet x = 1;\n```\n````\n\nAfter";
    assert_eq!(chunks(text), [text]);
}
