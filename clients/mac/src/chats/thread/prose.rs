//! Message rows inside a conversation: selectable owner cards, editorial
//! Markdown replies, and the working beat.

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, IntoElement, ParentElement as _, SharedString,
    StyleRefinement, Styled as _,
    component::{
        Theme,
        clipboard::Clipboard,
        text::{TextView, TextViewStyle},
    },
    div, ease_out_quint, relative, rems,
};

use super::Agent;
use crate::{
    chat::{Line, Speaker},
    face::Mood,
    theme::{Palette, u},
};

/// An owner's new line rises into place; an agent's settles.
const RISE: Duration = Duration::from_millis(360);
const SETTLE: Duration = Duration::from_millis(420);

/// One beat of the working dots.
const DOTS: Duration = Duration::from_millis(1100);

/// The dots need no more frames than this a second.
const DOT_FPS: f32 = 30.;

/// The owner's message: a clean, selectable card aligned to the right.
pub(super) fn owner(index: usize, line: &Line, live: bool, palette: Palette) -> AnyElement {
    let bubble = div()
        .max_w(relative(0.76))
        .px(u(14.))
        .py(u(10.))
        .rounded(u(16.))
        .bg(palette.chat_user)
        .border_1()
        .border_color(palette.outline)
        .shadow_2xs()
        .text_size(u(14.5))
        .line_height(u(21.5))
        .text_color(palette.text)
        .child(
            TextView::markdown(("owner", index), escape_markdown(&line.text))
                .selectable(true)
                .style(TextViewStyle::default().paragraph_gap(rems(6. / 16.))),
        );
    let placed = div().w_full().flex().justify_end().child(bubble);
    if !live {
        return placed.into_any_element();
    }
    placed
        .relative()
        .with_animation(
            ("rise", index),
            Animation::new(RISE).with_easing(ease_out_quint()),
            |placed, t| placed.top(u(16. * (1. - t))).opacity(t),
        )
        .into_any_element()
}

/// An agent's line: comfortable reading typography and copyable code blocks,
/// without repeating the agent's name above every reply.
pub(super) fn agent_line(index: usize, line: &Line, live: bool, palette: Palette) -> AnyElement {
    let body = if line.failed {
        div()
            .w_full()
            .px(u(14.))
            .py(u(10.))
            .rounded(u(12.))
            .bg(palette.danger.opacity(0.08))
            .border_1()
            .border_color(palette.danger.opacity(0.28))
            .text_size(u(14.))
            .line_height(u(21.))
            .text_color(palette.danger)
            .child(SharedString::from(line.text.clone()))
            .into_any_element()
    } else {
        let code_block_bg = palette.card;
        let code_block_border = palette.outline;
        let inline_style = StyleRefinement::default()
            .bg(palette.chat_agent)
            .border_1()
            .border_color(palette.outline)
            .rounded(u(5.))
            .px(u(4.))
            .py(u(0.5))
            .text_color(palette.text);
        let list_style = StyleRefinement::default().text_color(palette.secondary);
        let blockquote_style = StyleRefinement::default()
            .border_color(palette.outline_strong)
            .text_color(palette.secondary);
        let mut block_style = StyleRefinement::default()
            .pt(u(34.))
            .rounded(u(10.))
            .border_1()
            .border_color(code_block_border);
        block_style.background = Some(code_block_bg.into());
        let style = TextViewStyle::default()
            .paragraph_gap(rems(10. / 16.))
            .code_block(block_style);

        div()
            .w_full()
            .min_w_0()
            .text_size(u(14.5))
            .line_height(u(22.5))
            .text_color(palette.text_soft)
            .child(
                TextView::markdown(("said", index), line.text.clone())
                    .selectable(true)
                    .style(style)
                    .inline_code_style(inline_style)
                    .list_style(list_style)
                    .blockquote_style(blockquote_style)
                    .code_block_actions(move |block, _, cx| {
                        let sans = Theme::global(cx).font_family.clone();
                        let lang = block.lang().filter(|lang| !lang.is_empty()).map(|lang| {
                            div()
                                .font_family(sans.clone())
                                .text_size(u(11.))
                                .line_height(u(16.))
                                .text_color(palette.secondary)
                                .child(lang)
                        });
                        div()
                            .px(u(4.))
                            .flex()
                            .items_center()
                            .gap(u(6.))
                            .children(lang)
                            .child(
                                Clipboard::new("copy")
                                    .value(block.code().clone())
                                    .tooltip("Copy"),
                            )
                    }),
            )
            .into_any_element()
    };

    if !live {
        return body;
    }
    div()
        .w_full()
        .relative()
        .child(body)
        .with_animation(
            ("settle", index),
            Animation::new(SETTLE).with_easing(ease_out_quint()),
            |placed, t| placed.top(u(8. * (1. - t))).opacity(t),
        )
        .into_any_element()
}

/// The agent at work: its face bobbing, and dots that come and go in turn.
pub(super) fn working(agent: &Agent, palette: Palette) -> impl IntoElement {
    let dots = (0..3_u8).map(move |dot| {
        let offset = f32::from(dot) * 0.18;
        div()
            .size(u(4.5))
            .rounded_full()
            .bg(palette.secondary)
            .with_animation(
                ("dot", usize::from(dot)),
                Animation::new(DOTS).repeat().with_max_fps(DOT_FPS),
                move |dot, t| {
                    // Bright at its moment in the beat, faint the rest.
                    let at = (t - offset).rem_euclid(1.);
                    dot.opacity(0.3 + 0.7 * (1. - (at * 2. - 0.5).abs().min(1.)))
                },
            )
    });
    div()
        .w_full()
        .pt(u(14.))
        .flex()
        .items_center()
        .gap(u(8.))
        .child(
            agent
                .face(palette, 20.)
                .mood(Mood::Working)
                .alive("working-face"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(u(7.))
                .text_size(u(13.))
                .text_color(palette.secondary)
                .child(format!("{} is working on it", agent.name))
                .child(div().flex().gap(u(3.)).children(dots)),
        )
}

/// Escapes CommonMark syntax in owner messages so plain text renders literally
/// inside a selectable `TextView::markdown` while preserving newlines.
pub(super) fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let normalized = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    for (idx, raw_line) in lines.iter().enumerate() {
        for ch in raw_line.chars() {
            if matches!(
                ch,
                '\\' | '`'
                    | '*'
                    | '_'
                    | '#'
                    | '['
                    | ']'
                    | '<'
                    | '>'
                    | '~'
                    | '|'
                    | '!'
                    | '+'
                    | '-'
                    | '.'
                    | '('
                    | ')'
            ) {
                out.push('\\');
            }
            out.push(ch);
        }
        if idx + 1 < lines.len() {
            let next_empty = lines[idx + 1].is_empty();
            if raw_line.is_empty() || next_empty {
                out.push('\n');
            } else {
                // Two spaces before a newline emit a hard break within a paragraph.
                out.push_str("  \n");
            }
        }
    }
    out
}

/// Space above a row depending on whether the speaker switched.
pub(super) fn row_top_padding(before: Option<Speaker>, current: Speaker) -> f32 {
    match before {
        None => 4.,
        Some(prev) if prev == current => 10.,
        Some(_) => 20.,
    }
}

#[cfg(test)]
mod tests {
    use super::escape_markdown;

    #[test]
    fn escape_markdown_preserves_plain_text_and_line_breaks() {
        assert_eq!(
            escape_markdown("1. save *this* `code`\nnext line\n\npara 2"),
            "1\\. save \\*this\\* \\`code\\`  \nnext line\n\npara 2"
        );
    }
}
