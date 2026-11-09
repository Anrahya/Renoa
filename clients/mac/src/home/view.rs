//! How Home looks: the date and the day in a line, then what needs the
//! owner, the notes left while they were away, and what runs next.

use chrono::Local;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, assets::IconName, div,
    prelude::FluentBuilder as _, px,
};

use super::{Home, HomeEvent, feed::Item, feed::Kind};
use crate::{
    face::Face,
    sheets::{Loaded, Page, group, pill, schedule, section},
    stamp,
    theme::Palette,
};

/// The widest Home's column grows.
const COLUMN: f32 = 720.;

pub(super) fn render(home: &Home, palette: Palette, cx: &mut Context<Home>) -> impl IntoElement {
    let now = Local::now();
    let column = div()
        .w_full()
        .max_w(px(COLUMN))
        .px(px(32.))
        .pt(px(48.))
        .pb(px(32.))
        .flex()
        .flex_col()
        .gap(px(26.));
    let column = match &home.feed {
        Loaded::Ready(held) => {
            let asks = held.asks();
            let notes = held.notes();
            let quiet = asks.is_empty() && notes.is_empty();
            column
                .child(top(
                    &now.format("%A, %-d %B").to_string(),
                    &held.today,
                    palette,
                ))
                .when_some(home.refused.clone(), |column, refused| {
                    column.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.attention)
                            .child(refused),
                    )
                })
                .when(!asks.is_empty(), |column| {
                    column.child(needs_you(home, &asks, palette, cx))
                })
                .when(!notes.is_empty(), |column| {
                    column.child(away(home, &notes, palette, cx))
                })
                .when(quiet, |column| {
                    column.child(
                        div()
                            .text_color(palette.secondary)
                            .child("Nothing new. Your agents leave notes and questions here."),
                    )
                })
        }
        Loaded::Waiting => column
            .child(top(&now.format("%A, %-d %B").to_string(), "", palette))
            .child(
                div()
                    .text_color(palette.secondary)
                    .child("Reading your day…"),
            ),
        Loaded::Unread => column
            .child(top(&now.format("%A, %-d %B").to_string(), "", palette))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_color(palette.secondary)
                    .child("Renoa couldn't be reached just now.")
                    .child(
                        pill("again", "Try again", false, palette)
                            .on_click(cx.listener(|home, _, _, cx| home.read(cx))),
                    ),
            ),
    };
    let column = column.when(!home.coming.is_empty(), |column| {
        column.child(coming(home, palette, cx))
    });
    div()
        .id("home")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .justify_center()
        .child(column)
}

fn top(date: &str, today: &str, palette: Palette) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(palette.secondary)
                .child(date.to_owned()),
        )
        .child(
            div()
                .text_size(px(20.))
                .line_height(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(if today.is_empty() {
                    "Here's your day.".to_owned()
                } else {
                    today.to_owned()
                }),
        )
}

/// A small face for an item's agent: its own, or a plain one for an agent
/// this Mac does not list.
fn face(home: &Home, item: &Item, palette: Palette, size: f32) -> Face {
    let member = home.member(item.agent.id);
    Face::new(
        member.and_then(|member| member.look),
        member.is_some_and(|member| member.primary),
        palette,
        px(size),
    )
}

fn needs_you(
    home: &Home,
    asks: &[&Item],
    palette: Palette,
    cx: &mut Context<Home>,
) -> impl IntoElement {
    let now = Local::now();
    let cards = asks.iter().map(|ask| {
        let id = ask.id;
        let chat = home.member(ask.agent.id).map(|member| member.task_id);
        let choices = ask.choices.clone().unwrap_or_default();
        let answers = choices.into_iter().enumerate().map(|(index, choice)| {
            let said = choice.clone();
            pill(
                SharedString::from(format!("answer-{id}-{index}")),
                choice,
                index == 0,
                palette,
            )
            .on_click(cx.listener(move |home, _, _, cx| home.answer(id, said.clone(), cx)))
            .into_any_element()
        });
        let open = chat.map(|chat| {
            pill(
                SharedString::from(format!("open-{id}")),
                if ask.choices.is_some() {
                    "Open chat"
                } else {
                    "Answer in the chat"
                },
                ask.choices.is_none(),
                palette,
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(HomeEvent::Open(chat))))
            .into_any_element()
        });
        div()
            .p(px(14.))
            .rounded(px(16.))
            .bg(palette.attention.opacity(0.12))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(face(home, ask, palette, 22.))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} asks", ask.agent.name)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette.secondary)
                            .child(stamp::short(ask.at_ms, &now)),
                    ),
            )
            .child(div().line_height(px(19.)).child(ask.body.clone()))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    // Answering needs the agent's chat on this Mac.
                    .when(chat.is_some(), |row| row.children(answers))
                    .children(open)
                    .when(chat.is_none(), |row| {
                        row.child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.secondary)
                                .child(format!(
                                    "{} isn't online, so its chat can't open here yet.",
                                    ask.agent.name
                                )),
                        )
                    }),
            )
            .into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.attention)
                .child("NEEDS YOU"),
        )
        .child(div().grid().grid_cols(2).gap(px(12.)).children(cards))
}

fn away(
    home: &Home,
    notes: &[&Item],
    palette: Palette,
    cx: &mut Context<Home>,
) -> impl IntoElement {
    let now = Local::now();
    let rows = notes.iter().map(|note| {
        let id = note.id;
        let problem = note.kind == Kind::Problem;
        div()
            .flex()
            .items_start()
            .gap(px(12.))
            .px(px(12.))
            .py(px(10.))
            .child(face(home, note, palette, 24.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(note.agent.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette.secondary)
                                    .child(stamp::short(note.at_ms, &now)),
                            ),
                    )
                    .child(
                        div()
                            .line_height(px(19.))
                            .when(problem, |words| words.text_color(palette.danger))
                            .child(note.words().to_owned()),
                    ),
            )
            .when(problem, |row| {
                row.child(
                    pill(
                        SharedString::from(format!("again-{id}")),
                        "Run again",
                        false,
                        palette,
                    )
                    .on_click(cx.listener(move |home, _, _, cx| home.run_again(id, cx))),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("dismiss-{id}")))
                    .size(px(26.))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette.secondary)
                    .cursor_pointer()
                    .hover(|close| close.bg(palette.chat_agent).text_color(palette.text))
                    .on_click(cx.listener(move |home, _, _, cx| home.dismiss(id, cx)))
                    .child(IconName::Close),
            )
            .into_any_element()
    });
    let rows: Vec<AnyElement> = rows.collect();
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(section("While you were away", palette))
        .child(group(rows, palette))
}

fn coming(home: &Home, palette: Palette, cx: &mut Context<Home>) -> impl IntoElement {
    let now = Local::now();
    let rows = home.coming.iter().map(|routine| {
        let member = home.member(routine.agent_id);
        let words = schedule::words(&routine.schedule, &Local, home.here_zone.as_deref());
        let next = if routine.next_due_ms <= now.timestamp_millis() {
            "Any moment".to_owned()
        } else {
            stamp::ahead(routine.next_due_ms, &now)
        };
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(9.))
            .child(Face::new(
                member.and_then(|member| member.look),
                member.is_some_and(|member| member.primary),
                palette,
                px(20.),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(routine.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.secondary)
                            .truncate()
                            .child(format!("{words} · {next}")),
                    ),
            )
            .into_any_element()
    });
    let rows: Vec<AnyElement> = rows.collect();
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().child(section("Coming up", palette)))
                .child(
                    pill("all-routines", "All routines", false, palette).on_click(
                        cx.listener(|_, _, _, cx| cx.emit(HomeEvent::Sheet(Page::Routines))),
                    ),
                ),
        )
        .child(group(rows, palette))
}
