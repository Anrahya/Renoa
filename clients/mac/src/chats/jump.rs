//! The box ⌘K opens: type part of an agent's name or a page's, and Return
//! goes there.

use gpui_kit::{
    AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window,
    assets::IconName,
    component::input::{Input, InputEvent, InputState},
    div,
    prelude::FluentBuilder as _,
    px,
};

use renoa_rcp_wire::TaskId;

use crate::{
    face::{Face, Look},
    sheets::Page,
    theme::Palette,
};

/// Somewhere the box can go.
#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    /// The agent whose chat this is.
    Agent {
        task_id: TaskId,
        name: SharedString,
        look: Option<Look>,
        primary: bool,
    },
    Home,
    Sheet(Page),
}

impl Place {
    fn name(&self) -> &str {
        match self {
            Self::Agent { name, .. } => name.as_ref(),
            Self::Home => "Home",
            Self::Sheet(page) => page.title(),
        }
    }
}

/// What the box asks of the chats.
#[derive(Debug, Clone, PartialEq)]
pub enum JumpEvent {
    Go(Place),
}

pub struct Jump {
    places: Vec<Place>,
    query: Entity<InputState>,
    /// Which of the matching places Return opens.
    selected: usize,
}

impl EventEmitter<JumpEvent> for Jump {}

/// The places whose name has `query` in it, in any case; every place for an
/// empty query. Agents come first, as the list has them.
pub fn matching<'a>(places: &'a [Place], query: &str) -> Vec<&'a Place> {
    let query = query.trim().to_lowercase();
    places
        .iter()
        .filter(|place| place.name().to_lowercase().contains(&query))
        .collect()
}

impl Jump {
    pub fn new(places: Vec<Place>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Jump to an agent or page"));
        cx.subscribe_in(
            &query,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    this.selected = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.go(cx),
                _ => {}
            },
        )
        .detach();
        query.update(cx, |query, cx| query.focus(window, cx));
        Self {
            places,
            query,
            selected: 0,
        }
    }

    fn shown(&self, cx: &Context<Self>) -> Vec<Place> {
        let query = self.query.read(cx).value().to_string();
        matching(&self.places, &query)
            .into_iter()
            .cloned()
            .collect()
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        if let Some(place) = self.shown(cx).into_iter().nth(self.selected) {
            cx.emit(JumpEvent::Go(place));
        }
    }

    /// Moves the choice up or down the matching places, stopping at the ends.
    pub fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let last = self.shown(cx).len().saturating_sub(1);
        self.selected = if down {
            (self.selected + 1).min(last)
        } else {
            self.selected.saturating_sub(1)
        };
        cx.notify();
    }
}

impl Render for Jump {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window, cx);
        let shown = self.shown(cx);
        let selected = self.selected.min(shown.len().saturating_sub(1));
        let rows = shown.into_iter().enumerate().map(|(at, place)| {
            let chosen = at == selected;
            let picked = place.clone();
            div()
                .id(("place", at))
                .h(px(40.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(10.))
                .cursor_pointer()
                .when(chosen, |row| row.bg(palette.selected))
                .hover(|row| row.bg(palette.selected))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(JumpEvent::Go(picked.clone()))))
                .map(|row| match &place {
                    Place::Agent {
                        name,
                        look,
                        primary,
                        ..
                    } => row
                        .child(Face::new(*look, *primary, palette, px(24.)))
                        .child(
                            div()
                                .flex_1()
                                .font_weight(FontWeight::MEDIUM)
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.secondary)
                                .child("Chat"),
                        ),
                    Place::Home => labelled(row, IconName::House, "Home", "⌘0", palette),
                    Place::Sheet(page) => {
                        labelled(row, page.icon(), page.title(), page.keys(), palette)
                    }
                })
        });
        div()
            .w(px(520.))
            .flex()
            .flex_col()
            .rounded(px(14.))
            .bg(palette.card)
            .border_1()
            .border_color(palette.outline)
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(palette.divider)
                    .text_color(palette.secondary)
                    .child(IconName::Search)
                    .child(
                        div()
                            .flex_1()
                            .text_color(palette.text)
                            .child(Input::new(&self.query).appearance(false)),
                    ),
            )
            .child(div().p(px(6.)).flex().flex_col().gap(px(2.)).children(rows))
            .child(
                div()
                    .px(px(16.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(palette.divider)
                    .text_size(px(11.))
                    .text_color(palette.secondary)
                    .child("↑↓ to choose · Return to open · Esc to close"),
            )
    }
}

/// A row for a place that is not an agent: its icon, its name and its keys.
fn labelled(
    row: Stateful<Div>,
    icon: IconName,
    title: &'static str,
    keys: &'static str,
    palette: Palette,
) -> Stateful<Div> {
    row.child(
        div()
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(palette.secondary)
            .child(icon),
    )
    .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(title))
    .child(
        div()
            .text_size(px(12.))
            .text_color(palette.secondary)
            .child(keys),
    )
}

#[cfg(test)]
mod tests {
    use renoa_rcp_wire::TaskId;

    use super::{Place, matching};
    use crate::sheets::Page;

    #[test]
    fn a_place_matches_part_of_its_name_in_any_case() {
        let places = vec![
            Place::Agent {
                task_id: TaskId::new(),
                name: "Renoa".into(),
                look: None,
                primary: true,
            },
            Place::Agent {
                task_id: TaskId::new(),
                name: "Chef".into(),
                look: None,
                primary: false,
            },
            Place::Sheet(Page::Routines),
            Place::Sheet(Page::Memory),
        ];
        let names = |query: &str| -> Vec<String> {
            matching(&places, query)
                .into_iter()
                .map(|place| match place {
                    Place::Agent { name, .. } => name.to_string(),
                    Place::Home => "Home".to_owned(),
                    Place::Sheet(page) => page.title().to_owned(),
                })
                .collect()
        };
        assert_eq!(names(""), ["Renoa", "Chef", "Routines", "Memory"]);
        assert_eq!(names(" ME "), ["Memory"]);
        assert_eq!(names("e"), ["Renoa", "Chef", "Routines", "Memory"]);
        assert_eq!(names("ch"), ["Chef"]);
        assert!(names("zz").is_empty());
    }
}
