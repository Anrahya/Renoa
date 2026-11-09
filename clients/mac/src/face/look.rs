//! How an agent looks, as the Host picked it: one choice of each part. The
//! names are the Host's (`crates/renoa-local/src/host/appearance/look.rs`);
//! the colours are this app's.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Look {
    pub hair_front: HairFront,
    pub hair_back: HairBack,
    pub hair_colour: HairColour,
    pub skin: Skin,
    pub eyes: Eyes,
    pub cheeks: Cheeks,
    pub wear: Wear,
    pub tile: Tile,
    /// A stray tuft standing up from the top of the head.
    pub tuft: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HairFront {
    Short,
    Fringe,
    SidePart,
    Curls,
    Princess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HairBack {
    None,
    Bun,
    Long,
    TwinTails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HairColour {
    Ink,
    Brown,
    Auburn,
    Blonde,
    Grey,
}

/// Lightest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skin {
    Fair,
    Light,
    Medium,
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eyes {
    Round,
    Oval,
    Lashes,
    Sparkle,
    Cat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cheeks {
    None,
    Blush,
    Freckles,
    Lines,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wear {
    None,
    Glasses,
    Bow,
    Clip,
    Beanie,
    Headphones,
    CatEars,
}

/// The tile's colour: the landing page's clay, cobalt and ochre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tile {
    Clay,
    Cobalt,
    Ochre,
}

/// Renoa's own look, worn by the main agent whatever the Host picked. Her
/// tile is Paper, not this one's.
pub const RENOA: Look = Look {
    hair_front: HairFront::Fringe,
    hair_back: HairBack::None,
    hair_colour: HairColour::Ink,
    skin: Skin::Fair,
    eyes: Eyes::Round,
    cheeks: Cheeks::Blush,
    wear: Wear::None,
    tile: Tile::Clay,
    tuft: false,
};

/// Before the Host has said how an agent looks: a short-haired head on a
/// quiet tile.
pub const PLAIN: Look = Look {
    hair_front: HairFront::Short,
    hair_back: HairBack::None,
    hair_colour: HairColour::Grey,
    skin: Skin::Light,
    eyes: Eyes::Round,
    cheeks: Cheeks::None,
    wear: Wear::None,
    tile: Tile::Clay,
    tuft: false,
};

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;

    use super::{Cheeks, Eyes, HairBack, HairColour, HairFront, Skin, Tile, Wear};

    /// Reads each of `names` as a `T`, and finds them all different.
    fn every<T: DeserializeOwned + PartialEq + std::fmt::Debug>(names: &[&str]) {
        let read: Vec<T> = names
            .iter()
            .map(|name| {
                serde_json::from_str(&format!("\"{name}\""))
                    .unwrap_or_else(|error| panic!("{name}: {error}"))
            })
            .collect();
        for (at, part) in read.iter().enumerate() {
            assert!(
                !read[..at].contains(part),
                "{} reads as an earlier name",
                names[at]
            );
        }
    }

    /// Every name the Host gives a part
    /// (`crates/renoa-local/src/host/appearance/look.rs`) reads as its own choice.
    #[test]
    fn every_name_the_host_gives_reads() {
        every::<HairFront>(&["short", "fringe", "side_part", "curls", "princess"]);
        every::<HairBack>(&["none", "bun", "long", "twin_tails"]);
        every::<HairColour>(&["ink", "brown", "auburn", "blonde", "grey"]);
        every::<Skin>(&["fair", "light", "medium", "deep"]);
        every::<Eyes>(&["round", "oval", "lashes", "sparkle", "cat"]);
        every::<Cheeks>(&["none", "blush", "freckles", "lines"]);
        every::<Wear>(&[
            "none",
            "glasses",
            "bow",
            "clip",
            "beanie",
            "headphones",
            "cat_ears",
        ]);
        every::<Tile>(&["clay", "cobalt", "ochre"]);
    }
}
