//! The parts' lines, as the Android app draws them
//! (`clients/android/.../badge/Face.kt`): in a 100-unit square, the head a
//! circle of radius 22 at (50, 54) on a pointed-top hexagon. GPUI clips only
//! to rectangles, so the shapes that would cross the hexagon (the long hair,
//! the twin tails, the shoulders) end on its edges.

use gpui_kit::{PathBuilder, Pixels, Point, point, px};

pub const TILE: &str = "M50 4 L90 27 L90 73 L50 96 L10 73 L10 27 Z";
pub const SHOULDERS: &str = "M29.1 84 Q50 72 70.9 84 L50 96 Z";
pub const LONG: &str = "M27 52 Q27 28 50 28 Q73 28 73 52 L75.9 81.1 L50 96 L24.1 81.1 Z";
pub const TWIN_TAILS: &str =
    "M29 40 Q13 54 18 77.6 Q25 66 31 58 Z M71 40 Q87 54 82 77.6 Q75 66 69 58 Z";
pub const SHORT: &str = "M28 52 Q28 30 50 30 Q72 30 72 52 Q65 41 50 41 Q35 41 28 52 Z";
pub const FRINGE: &str = "M27.5 56 Q26 29 50 29 Q74 29 72.5 56 L70 47 Q60 47 58 42 Q53 48 47 44 Q41 49 34 45 Q31 50 27.5 56 Z";
pub const SIDE_PART: &str = "M27.5 56 Q25 28 53 29 Q75 31 72.5 52 Q68 40 57 38 Q45 47 27.5 56 Z";
pub const PRINCESS: &str = "M27.5 58 Q26 29 50 29 Q74 29 72.5 58 L70 46 L30 46 Z M27 46 L33.5 46 L32.5 74 L26 72 Z M73 46 L66.5 46 L67.5 74 L74 72 Z";
pub const TUFT: &str = "M49 30.5 Q45.5 18 56 13.5 Q50.5 21.5 54.5 30.5 Z";
pub const BLUSH_LINES: &str = "M33.5 63 l2.2 -2.4 M36.8 63.6 l2.2 -2.4 M40.1 64.2 l2.2 -2.4 M57.7 64.2 l2.2 -2.4 M61 63.6 l2.2 -2.4 M64.3 63 l2.2 -2.4";
pub const LASHES: &str = "M39.6 53.4 l-2.2 -1.6 M60.4 53.4 l2.2 -1.6";
pub const CAT_EYES: &str = "M37.6 56.4 Q42 51.2 46.4 55.6 Q42 59.4 37.6 56.4 Z M62.4 56.4 Q58 51.2 53.6 55.6 Q58 59.4 62.4 56.4 Z";
pub const SMILE: &str = "M46 63.5 Q50 66.8 54 63.5";
pub const CLOSED: &str = "M38.5 56 q3.5 2.6 7 0 M54.5 56 q3.5 2.6 7 0";
pub const BRIDGE: &str = "M48.2 56 h3.6";
pub const BOW: &str = "M64 33 L73.5 28 L72.5 38.5 Z M64 33 L55.5 26.5 L56.5 37 Z";
pub const BEANIE: &str = "M27 47 Q27 23 50 23 Q73 23 73 47 Z";
pub const BAND: &str = "M27 55 Q26 26 50 26 Q74 26 73 55";
pub const EARS: &str = "M29 37 L31 17 L44 30 Z M71 37 L69 17 L56 30 Z";
pub const INNER_EARS: &str = "M32.5 31 L33.5 22.5 L39.5 28.5 Z M67.5 31 L66.5 22.5 L60.5 28.5 Z";
/// Round its own centre, which the main agent wears at (66, 34), turned 12°.
pub const STAR: &str =
    "M0 -6.5 L1.9 -2.6 L6.2 -2 L3.1 1 L3.8 5.3 L0 3.3 L-3.8 5.3 L-3.1 1 L-6.2 -2 L-1.9 -2.6 Z";

/// A point of the board, in its units; the face's transform places it.
fn at(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

/// Adds `d`, an SVG path of the commands M, L, H, V, Q and Z, absolute or
/// relative, to `builder`, in the board's units, and says whether it read
/// all of `d`. The shapes above are its only input, and a test reads each one
/// to the end.
pub fn trace(builder: &mut PathBuilder, d: &str) -> bool {
    let mut tokens = Tokens::new(d);
    let (mut x, mut y) = (0.0_f32, 0.0_f32);
    let (mut start_x, mut start_y) = (0.0_f32, 0.0_f32);
    let mut command = 'M';
    while let Some(next) = tokens.command_or_number() {
        if let Token::Command(letter) = next {
            command = letter;
            if letter.eq_ignore_ascii_case(&'z') {
                builder.close();
                (x, y) = (start_x, start_y);
                continue;
            }
        } else {
            tokens.put_back(next);
        }
        let relative = command.is_ascii_lowercase();
        let (dx, dy) = if relative { (x, y) } else { (0.0, 0.0) };
        match command.to_ascii_uppercase() {
            'M' | 'L' => {
                let (Some(nx), Some(ny)) = (tokens.number(), tokens.number()) else {
                    return false;
                };
                (x, y) = (nx + dx, ny + dy);
                if command.eq_ignore_ascii_case(&'m') {
                    builder.move_to(at(x, y));
                    (start_x, start_y) = (x, y);
                    // Pairs after a move are lines, as SVG reads them.
                    command = if relative { 'l' } else { 'L' };
                } else {
                    builder.line_to(at(x, y));
                }
            }
            'H' => {
                let Some(nx) = tokens.number() else {
                    return false;
                };
                x = nx + dx;
                builder.line_to(at(x, y));
            }
            'V' => {
                let Some(ny) = tokens.number() else {
                    return false;
                };
                y = ny + dy;
                builder.line_to(at(x, y));
            }
            'Q' => {
                let (Some(cx), Some(cy), Some(nx), Some(ny)) = (
                    tokens.number(),
                    tokens.number(),
                    tokens.number(),
                    tokens.number(),
                ) else {
                    return false;
                };
                let control = at(cx + dx, cy + dy);
                (x, y) = (nx + dx, ny + dy);
                builder.curve_to(at(x, y), control);
            }
            _ => return false,
        }
    }
    tokens.read_all()
}

/// Adds an ellipse round (`cx`, `cy`) to `builder`, as four cubic curves.
pub fn ellipse(builder: &mut PathBuilder, (cx, cy): (f32, f32), (rx, ry): (f32, f32)) {
    // How far a cubic's controls sit along the tangent to follow a quarter circle.
    const K: f32 = 0.552_284_8;
    let (kx, ky) = (rx * K, ry * K);
    builder.move_to(at(cx + rx, cy));
    builder.cubic_bezier_to(at(cx, cy + ry), at(cx + rx, cy + ky), at(cx + kx, cy + ry));
    builder.cubic_bezier_to(at(cx - rx, cy), at(cx - kx, cy + ry), at(cx - rx, cy + ky));
    builder.cubic_bezier_to(at(cx, cy - ry), at(cx - rx, cy - ky), at(cx - kx, cy - ry));
    builder.cubic_bezier_to(at(cx + rx, cy), at(cx + kx, cy - ry), at(cx + rx, cy - ky));
    builder.close();
}

/// Adds a rectangle from (`x`, `y`) of `w` by `h` with corners of radius `r`
/// to `builder`.
pub fn round_rect(builder: &mut PathBuilder, (x, y): (f32, f32), (w, h): (f32, f32), r: f32) {
    let (right, bottom) = (x + w, y + h);
    builder.move_to(at(x + r, y));
    builder.line_to(at(right - r, y));
    builder.curve_to(at(right, y + r), at(right, y));
    builder.line_to(at(right, bottom - r));
    builder.curve_to(at(right - r, bottom), at(right, bottom));
    builder.line_to(at(x + r, bottom));
    builder.curve_to(at(x, bottom - r), at(x, bottom));
    builder.line_to(at(x, y + r));
    builder.curve_to(at(x + r, y), at(x, y));
    builder.close();
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Command(char),
    Number(f32),
}

struct Tokens<'a> {
    rest: &'a str,
    back: Option<Token>,
    /// Whether a number would not read.
    broken: bool,
}

impl<'a> Tokens<'a> {
    const fn new(d: &'a str) -> Self {
        Self {
            rest: d,
            back: None,
            broken: false,
        }
    }

    const fn put_back(&mut self, token: Token) {
        self.back = Some(token);
    }

    fn command_or_number(&mut self) -> Option<Token> {
        if let Some(token) = self.back.take() {
            return Some(token);
        }
        self.rest = self
            .rest
            .trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        let first = self.rest.chars().next()?;
        if first.is_ascii_alphabetic() {
            self.rest = &self.rest[1..];
            return Some(Token::Command(first));
        }
        // A number runs to the next separator, letter, or sign after its start.
        let end = self
            .rest
            .char_indices()
            .skip(1)
            .find(|&(_, c)| c.is_whitespace() || c == ',' || c == '-' || c.is_ascii_alphabetic())
            .map_or(self.rest.len(), |(at, _)| at);
        let (number, rest) = self.rest.split_at(end);
        self.rest = rest;
        let parsed = number.parse().ok().map(Token::Number);
        self.broken |= parsed.is_none();
        parsed
    }

    /// Whether everything was read, and read as commands and numbers.
    fn read_all(&self) -> bool {
        !self.broken && self.back.is_none() && self.rest.trim().is_empty()
    }

    fn number(&mut self) -> Option<f32> {
        match self.command_or_number()? {
            Token::Number(value) => Some(value),
            command @ Token::Command(_) => {
                self.put_back(command);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::PathBuilder;

    use super::{Token, Tokens, trace};

    /// Every shape the face draws, by name.
    const SHAPES: [(&str, &str); 21] = [
        ("TILE", super::TILE),
        ("SHOULDERS", super::SHOULDERS),
        ("LONG", super::LONG),
        ("TWIN_TAILS", super::TWIN_TAILS),
        ("SHORT", super::SHORT),
        ("FRINGE", super::FRINGE),
        ("SIDE_PART", super::SIDE_PART),
        ("PRINCESS", super::PRINCESS),
        ("TUFT", super::TUFT),
        ("BLUSH_LINES", super::BLUSH_LINES),
        ("LASHES", super::LASHES),
        ("CAT_EYES", super::CAT_EYES),
        ("SMILE", super::SMILE),
        ("CLOSED", super::CLOSED),
        ("BRIDGE", super::BRIDGE),
        ("BOW", super::BOW),
        ("BEANIE", super::BEANIE),
        ("BAND", super::BAND),
        ("EARS", super::EARS),
        ("INNER_EARS", super::INNER_EARS),
        ("STAR", super::STAR),
    ];

    #[test]
    fn every_shape_reads_to_its_end_and_builds() {
        for (name, d) in SHAPES {
            let mut builder = PathBuilder::fill();
            assert!(trace(&mut builder, d), "{name} reads to its end");
            assert!(builder.build().is_ok(), "{name} builds");
        }
        for broken in ["M1 2 L3", "M1 2 C3 4 5 6", "M1 2 L3 x4", "M1 2 L3 4 1.2.3"] {
            assert!(
                !trace(&mut PathBuilder::fill(), broken),
                "{broken} does not read"
            );
        }
    }

    #[test]
    fn a_path_reads_as_its_commands_and_numbers() {
        let mut tokens = Tokens::new("M37.5 49 q4 -3 8-0.5Z");
        let read: Vec<Token> = std::iter::from_fn(|| tokens.command_or_number()).collect();
        assert_eq!(
            read,
            [
                Token::Command('M'),
                Token::Number(37.5),
                Token::Number(49.),
                Token::Command('q'),
                Token::Number(4.),
                Token::Number(-3.),
                Token::Number(8.),
                Token::Number(-0.5),
                Token::Command('Z'),
            ]
        );
    }
}
