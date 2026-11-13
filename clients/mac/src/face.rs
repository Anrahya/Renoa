//! An agent's face on its tile, drawn from the look the Host picked, as the
//! Android app draws it (`badge/Face.kt`).

pub mod blink;
pub mod look;
mod shapes;

pub use look::Look;

use std::{f32::consts::TAU, time::Duration};

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, ElementId, Hsla, IntoElement,
    ParentElement as _, PathBuilder, PathStyle, Pixels, Rems, RenderOnce, Rgba, SharedString,
    StrokeOptions, Styled as _, Transform, Window, canvas, div, rgb,
};
use look::{Cheeks, Eyes, HairBack, HairColour, HairFront, PLAIN, RENOA, Skin, Tile, Wear};
use lyon_tessellation::{LineCap, LineJoin, geom::Angle, math::vector};

use crate::theme::{Palette, u};

/// What the face shows the agent doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    /// Looks ahead and smiles.
    Resting,
    /// Looks down at its work.
    Working,
}

/// A face `size` across: the agent wearing its look, or Renoa's own on the
/// main agent's paper tile with her star, or a plain one before the Host has
/// said how it looks.
#[derive(IntoElement)]
pub struct Face {
    look: Option<Look>,
    primary: bool,
    mood: Mood,
    blinking: bool,
    palette: Palette,
    size: Rems,
}

impl Face {
    pub const fn new(look: Option<Look>, primary: bool, palette: Palette, size: Rems) -> Self {
        Self {
            look,
            primary,
            mood: Mood::Resting,
            blinking: false,
            palette,
            size,
        }
    }

    pub const fn mood(mut self, mood: Mood) -> Self {
        self.mood = mood;
        self
    }

    /// Shuts the eyes for a blink.
    pub const fn blinking(mut self, blinking: bool) -> Self {
        self.blinking = blinking;
        self
    }

    /// The face as it sits on screen, bobbing while it works. `id` names it
    /// among its siblings. Its blinks come from [`blink`], through
    /// [`Face::blinking`].
    pub fn alive(self, id: impl Into<SharedString>) -> AnyElement {
        let working = self.mood == Mood::Working;
        let size = self.size;
        let held = div().relative().size(size).flex_none().child(self);
        if !working {
            return held.into_any_element();
        }
        let lift = size * BOB_LIFT;
        held.with_animation(
            ElementId::Name(format!("{}-bob", id.into()).into()),
            Animation::new(BOB_EVERY).repeat().with_max_fps(BOB_FPS),
            move |held, t| held.top(-lift * (t * TAU).sin().abs()),
        )
        .into_any_element()
    }
}

/// A bob needs no more frames than this a second.
const BOB_FPS: f32 = 30.;

/// How long a working face takes to bob twice.
const BOB_EVERY: Duration = Duration::from_millis(2800);

/// How high a working face bobs, as a part of its size.
const BOB_LIFT: f32 = 0.06;

impl RenderOnce for Face {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let size = self.size;
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| self.paint(bounds, window),
        )
        .size(size)
    }
}

/// The ink every face is drawn in, on either theme.
fn ink() -> Rgba {
    rgb(0x141414)
}

/// The main agent's tile: paper, on either theme.
fn paper() -> Rgba {
    rgb(0xffffff)
}

/// What the main agent's rim and shoulders are shaded from, since paper has no shade of its own.
fn paper_shade() -> Rgba {
    rgb(0xd9d4c8)
}

/// What a face wears: a bow, a beanie, cat ears.
fn wear() -> Rgba {
    rgb(0xffffff)
}

fn blush() -> Rgba {
    rgb(0xe9a493)
}

const fn skin(skin: Skin) -> u32 {
    match skin {
        Skin::Fair => 0xf6dcc6,
        Skin::Light => 0xe8b892,
        Skin::Medium => 0xc68b62,
        Skin::Deep => 0x8d5a3c,
    }
}

const fn hair(hair: HairColour) -> u32 {
    match hair {
        HairColour::Ink => 0x2b2420,
        HairColour::Brown => 0x6b4228,
        HairColour::Auburn => 0x9a4a2c,
        HairColour::Blonde => 0xd9b26a,
        HairColour::Grey => 0xb9b3ab,
    }
}

/// `color` taken `by` of the way to black.
fn darker(color: Rgba, by: f32) -> Rgba {
    let keep = 1. - by;
    Rgba {
        r: color.r * keep,
        g: color.g * keep,
        b: color.b * keep,
        a: color.a,
    }
}

const CURLS: [(f32, f32, f32); 6] = [
    (31., 45., 7.),
    (36., 36., 8.),
    (45., 31., 8.),
    (55., 31., 8.),
    (64., 36., 8.),
    (69., 45., 7.),
];

const FRECKLES: [(f32, f32); 6] = [
    (35.5, 60.5),
    (38.5, 62.5),
    (35., 64.),
    (64.5, 60.5),
    (61.5, 62.5),
    (65., 64.),
];

impl Face {
    /// The tile's colour: paper for the main agent, its own for any other, a
    /// quiet grey before the Host has said.
    fn tile(&self) -> Rgba {
        if self.primary {
            return paper();
        }
        let Some(look) = self.look else {
            return self.palette.chat_user.to_rgb();
        };
        match look.tile {
            Tile::Clay => self.palette.clay,
            Tile::Cobalt => self.palette.cobalt,
            Tile::Ochre => self.palette.ochre,
        }
        .to_rgb()
    }

    fn paint(self, bounds: Bounds<Pixels>, window: &mut Window) {
        let unit = f32::from(bounds.size.width.min(bounds.size.height)) / 100.;
        let origin = vector(f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let mut pen = Pen {
            place: Transform::scale(unit, unit).then_translate(origin),
            unit,
            finest: 1. / window.scale_factor(),
            window,
        };
        let worn = if self.primary {
            RENOA
        } else {
            self.look.unwrap_or(PLAIN)
        };
        let tile = self.tile();
        let skin = rgb(skin(worn.skin));
        let hair = rgb(hair(worn.hair_colour));
        // Hair and head carry a rim a shade darker than the tile, so neither melts into it.
        let shade = if self.primary { paper_shade() } else { tile };
        let rim = darker(shade, 0.35);
        let shape = |pen: &mut Pen, d: &str, color: Rgba| {
            pen.fill(color, |b| shapes::trace(b, d));
            pen.stroke(1.2, rim, |b| shapes::trace(b, d));
        };
        let circle = |pen: &mut Pen, (x, y, r): (f32, f32, f32), color: Rgba| {
            pen.fill(color, |b| shapes::ellipse(b, (x, y), (r, r)));
        };

        pen.fill(tile, |b| shapes::trace(b, shapes::TILE));
        match worn.hair_back {
            HairBack::Long => shape(&mut pen, shapes::LONG, hair),
            HairBack::TwinTails => shape(&mut pen, shapes::TWIN_TAILS, hair),
            HairBack::None | HairBack::Bun => {}
        }
        pen.fill(darker(shade, if self.primary { 0. } else { 0.28 }), |b| {
            shapes::trace(b, shapes::SHOULDERS);
        });
        if worn.hair_back == HairBack::Bun {
            circle(&mut pen, (50., 27., 9.), hair);
            pen.stroke(1.2, rim, |b| shapes::ellipse(b, (50., 27.), (9., 9.)));
        }
        circle(&mut pen, (50., 54., 22.), skin);
        pen.stroke(1.2, rim, |b| shapes::ellipse(b, (50., 54.), (22., 22.)));
        match worn.hair_front {
            HairFront::Short => shape(&mut pen, shapes::SHORT, hair),
            HairFront::Fringe => shape(&mut pen, shapes::FRINGE, hair),
            HairFront::SidePart => shape(&mut pen, shapes::SIDE_PART, hair),
            HairFront::Princess => shape(&mut pen, shapes::PRINCESS, hair),
            HairFront::Curls => {
                for (x, y, r) in CURLS {
                    pen.stroke(1.2, rim, |b| shapes::ellipse(b, (x, y), (r, r)));
                }
                for curl in CURLS {
                    circle(&mut pen, curl, hair);
                }
            }
        }
        if worn.hair_back == HairBack::TwinTails {
            for x in [28.5, 71.5] {
                circle(&mut pen, (x, 42., 3.2), wear());
                pen.stroke(0.9, ink(), |b| shapes::ellipse(b, (x, 42.), (3.2, 3.2)));
            }
        }
        if worn.tuft {
            shape(&mut pen, shapes::TUFT, hair);
        }
        match worn.cheeks {
            Cheeks::Lines => pen.stroke(1.2, rgb(0xd9826e), |b| {
                shapes::trace(b, shapes::BLUSH_LINES);
            }),
            Cheeks::Blush => {
                for x in [37., 63.] {
                    circle(&mut pen, (x, 62., 3.3), blush());
                }
            }
            Cheeks::Freckles => {
                for (x, y) in FRECKLES {
                    circle(&mut pen, (x, y, 1.), darker(skin, 0.38));
                }
            }
            Cheeks::None => {}
        }
        // What it wears goes under the face's lines, glasses over them.
        if worn.wear != Wear::Glasses {
            pen.wear(worn.wear);
        }
        self.expression(&mut pen, worn.eyes);
        if worn.wear == Wear::Glasses {
            pen.wear(worn.wear);
        }
        if self.primary {
            let star = Transform::rotation(Angle::degrees(12.)).then_translate(vector(66., 34.));
            pen.fill_at(star, wear(), |b| shapes::trace(b, shapes::STAR));
            pen.stroke_at(star, 1., ink(), |b| shapes::trace(b, shapes::STAR));
        }
    }

    /// The eyes and mouth.
    fn expression(&self, pen: &mut Pen, eyes: Eyes) {
        let pair = |pen: &mut Pen, (rx, ry): (f32, f32), cy: f32, dx: f32| {
            for x in [42., 58.] {
                pen.fill(ink(), |b| shapes::ellipse(b, (x + dx, cy), (rx, ry)));
            }
        };
        match self.mood {
            Mood::Resting => {
                if self.blinking {
                    pen.stroke(1.6, ink(), |b| shapes::trace(b, shapes::CLOSED));
                } else {
                    match eyes {
                        Eyes::Round => pair(pen, (2.8, 2.8), 56., 0.),
                        Eyes::Oval => pair(pen, (2.3, 3.4), 56., 0.),
                        Eyes::Lashes => {
                            pair(pen, (2.4, 3.2), 56., 0.);
                            pen.stroke(1.3, ink(), |b| shapes::trace(b, shapes::LASHES));
                        }
                        Eyes::Sparkle => {
                            pair(pen, (3.3, 4.4), 55.5, 0.);
                            for (x, y, r) in [
                                (43.2, 53.8, 1.4),
                                (59.2, 53.8, 1.4),
                                (41., 57.6, 0.7),
                                (57., 57.6, 0.7),
                            ] {
                                pen.fill(paper(), |b| shapes::ellipse(b, (x, y), (r, r)));
                            }
                        }
                        Eyes::Cat => pen.fill(ink(), |b| shapes::trace(b, shapes::CAT_EYES)),
                    }
                }
                pen.stroke(1.8, ink(), |b| shapes::trace(b, shapes::SMILE));
            }
            Mood::Working => {
                if self.blinking {
                    pen.stroke(1.6, ink(), |b| shapes::trace(b, shapes::CLOSED));
                } else {
                    pair(pen, (2.4, 2.6), 58.5, -1.5);
                }
                pen.fill(ink(), |b| shapes::ellipse(b, (50., 65.), (1.6, 1.6)));
            }
        }
    }
}

/// Paints shapes given in the board's units into the face's bounds.
struct Pen<'a> {
    window: &'a mut Window,
    /// From the board's units to the window's pixels.
    place: Transform,
    /// Pixels to a unit.
    unit: f32,
    /// The thinnest line worth drawing: one of the screen's pixels.
    finest: f32,
}

impl Pen<'_> {
    fn fill<R>(&mut self, color: Rgba, shape: impl FnOnce(&mut PathBuilder) -> R) {
        self.fill_at(Transform::identity(), color, shape);
    }

    fn fill_at<R>(
        &mut self,
        at: Transform,
        color: Rgba,
        shape: impl FnOnce(&mut PathBuilder) -> R,
    ) {
        self.paint(PathBuilder::fill(), at, color, shape);
    }

    /// Strokes `shape` with a round-capped line `width` units wide.
    fn stroke<R>(&mut self, width: f32, color: Rgba, shape: impl FnOnce(&mut PathBuilder) -> R) {
        self.stroke_at(Transform::identity(), width, color, shape);
    }

    fn stroke_at<R>(
        &mut self,
        at: Transform,
        width: f32,
        color: Rgba,
        shape: impl FnOnce(&mut PathBuilder) -> R,
    ) {
        let width = (width * self.unit).max(self.finest);
        let line = StrokeOptions::default()
            .with_line_width(width)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round);
        self.paint(
            PathBuilder::fill().with_style(PathStyle::Stroke(line)),
            at,
            color,
            shape,
        );
    }

    fn paint<R>(
        &mut self,
        mut builder: PathBuilder,
        at: Transform,
        color: Rgba,
        shape: impl FnOnce(&mut PathBuilder) -> R,
    ) {
        shape(&mut builder);
        builder.transform(at.then(&self.place));
        // Only `shapes`' own are drawn, and its test reads and builds each one.
        if let Ok(path) = builder.build() {
            self.window.paint_path(path, Hsla::from(color));
        }
    }

    fn wear(&mut self, wear: Wear) {
        let line = ink();
        match wear {
            Wear::None => {}
            Wear::Glasses => {
                for x in [42., 58.] {
                    self.stroke(1.6, line, |b| shapes::ellipse(b, (x, 56.), (6.2, 6.2)));
                }
                self.stroke(1.6, line, |b| shapes::trace(b, shapes::BRIDGE));
            }
            Wear::Bow => {
                self.fill(self::wear(), |b| shapes::trace(b, shapes::BOW));
                self.stroke(0.9, line, |b| shapes::trace(b, shapes::BOW));
                self.fill(self::wear(), |b| shapes::ellipse(b, (64., 33.), (2.4, 2.4)));
                self.stroke(0.9, line, |b| shapes::ellipse(b, (64., 33.), (2.4, 2.4)));
            }
            Wear::Clip => {
                let turned = Transform::translation(-67.5, -39.2)
                    .then_rotate(Angle::degrees(-24.))
                    .then_translate(vector(67.5, 39.2));
                let clip = |b: &mut PathBuilder| shapes::round_rect(b, (62., 37.), (11., 4.4), 2.2);
                self.fill_at(turned, self::wear(), clip);
                self.stroke_at(turned, 0.9, line, clip);
            }
            Wear::Beanie => {
                self.fill(self::wear(), |b| shapes::trace(b, shapes::BEANIE));
                self.fill(darker(self::wear(), 0.22), |b| {
                    shapes::round_rect(b, (25.5, 43.), (49., 8.), 4.);
                });
                self.fill(self::wear(), |b| shapes::ellipse(b, (50., 21.), (4.5, 4.5)));
            }
            Wear::Headphones => {
                self.stroke(3., line, |b| shapes::trace(b, shapes::BAND));
                for x in [22., 70.] {
                    let cup = |b: &mut PathBuilder| shapes::round_rect(b, (x, 50.), (8., 13.), 3.5);
                    self.fill(self::wear(), cup);
                    self.stroke(1.2, line, cup);
                }
            }
            Wear::CatEars => {
                self.fill(self::wear(), |b| shapes::trace(b, shapes::EARS));
                self.stroke(1., line, |b| shapes::trace(b, shapes::EARS));
                self.fill(blush(), |b| shapes::trace(b, shapes::INNER_EARS));
            }
        }
    }
}

/// Renoa's five-pointed star badge at `size` units across.
pub(crate) fn star(size: f32, color: Hsla) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let side = f32::from(bounds.size.width.min(bounds.size.height));
            let unit = side / 13.5;
            let centre = vector(
                f32::from(bounds.origin.x) + side * 0.5,
                f32::from(bounds.origin.y) + side * 0.54,
            );
            let mut builder = PathBuilder::fill();
            shapes::trace(&mut builder, shapes::STAR);
            builder.transform(Transform::scale(unit, unit).then_translate(centre));
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(u(size))
    .flex_none()
    .into_any_element()
}
