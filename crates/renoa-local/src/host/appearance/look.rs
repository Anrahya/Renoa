//! An agent's look: one choice of each part. The Host picks it when the agent
//! is made and never gives two agents the same one; apps draw it.

use std::collections::HashSet;

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::host::catalog::HostCatalogError;

/// Declares one part: its choices, the names they travel and are stored as,
/// and the list the table's check and the picker both read.
macro_rules! part {
    ($(#[$doc:meta])* $part:ident { $($choice:ident = $name:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $part {
            $(#[serde(rename = $name)] $choice),+
        }

        impl $part {
            /// Every choice, in the order the picker counts them.
            pub const ALL: &'static [Self] = &[$(Self::$choice),+];

            /// The choice's stored and wire name.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$choice => $name),+
                }
            }

            fn parse(name: &str) -> Result<Self, HostCatalogError> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|choice| choice.as_str() == name)
                    .ok_or_else(|| {
                        HostCatalogError::Invalid(format!(
                            "stored look has unknown {} `{name}`",
                            stringify!($part)
                        ))
                    })
            }
        }
    };
}

part!(
    /// The hair over the face.
    HairFront { Short = "short", Fringe = "fringe", SidePart = "side_part", Curls = "curls", Princess = "princess" }
);
part!(
    /// The hair behind the head.
    HairBack { None = "none", Bun = "bun", Long = "long", TwinTails = "twin_tails" }
);
part!(
    /// The hair's colour.
    HairColour { Ink = "ink", Brown = "brown", Auburn = "auburn", Blonde = "blonde", Grey = "grey" }
);
part!(
    /// The skin's tone, lightest first.
    Skin { Fair = "fair", Light = "light", Medium = "medium", Deep = "deep" }
);
part!(
    /// The eyes' shape.
    Eyes { Round = "round", Oval = "oval", Lashes = "lashes", Sparkle = "sparkle", Cat = "cat" }
);
part!(
    /// What the cheeks carry.
    Cheeks { None = "none", Blush = "blush", Freckles = "freckles", Lines = "lines" }
);
part!(
    /// The one thing the agent wears.
    Wear { None = "none", Glasses = "glasses", Bow = "bow", Clip = "clip", Beanie = "beanie", Headphones = "headphones", CatEars = "cat_ears" }
);
part!(
    /// The tile's colour. Green and purple are never one.
    Tile { Clay = "clay", Cobalt = "cobalt", Ochre = "ochre" }
);

/// One choice of every part. Two agents of one Host never share a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// How many looks there are.
pub(super) const LOOKS: u64 = HairFront::ALL.len() as u64
    * HairBack::ALL.len() as u64
    * HairColour::ALL.len() as u64
    * Skin::ALL.len() as u64
    * Eyes::ALL.len() as u64
    * Cheeks::ALL.len() as u64
    * Wear::ALL.len() as u64
    * Tile::ALL.len() as u64
    * 2;

/// The picker's stride through the looks. It shares no factor with [`LOOKS`],
/// so walking by it from any start meets every look once.
pub(super) const STRIDE: u64 = 1009;

/// Where an agent's walk through the looks starts: from its id, so the same
/// agents on the same Host always get the same looks.
const START_DOMAIN: &str = "renoa.agent.look.v1\n";

impl Look {
    /// The look at `index` below [`LOOKS`], counting the parts in field order
    /// with the tuft fastest.
    pub(super) fn at(index: u64) -> Self {
        fn take<T: Copy>(index: &mut u64, all: &[T]) -> T {
            let count = all.len() as u64;
            let choice = all[usize::try_from(*index % count).unwrap_or_default()];
            *index /= count;
            choice
        }
        let mut rest = index;
        let tuft = rest % 2 == 1;
        rest /= 2;
        let tile = take(&mut rest, Tile::ALL);
        let wear = take(&mut rest, Wear::ALL);
        let cheeks = take(&mut rest, Cheeks::ALL);
        let eyes = take(&mut rest, Eyes::ALL);
        let skin = take(&mut rest, Skin::ALL);
        let hair_colour = take(&mut rest, HairColour::ALL);
        let hair_back = take(&mut rest, HairBack::ALL);
        let hair_front = take(&mut rest, HairFront::ALL);
        Self {
            hair_front,
            hair_back,
            hair_colour,
            skin,
            eyes,
            cheeks,
            wear,
            tile,
            tuft,
        }
    }
}

/// Schema 54's look table. The unique index is what keeps two agents from
/// sharing a look; each check lists exactly the choices of its part.
pub(super) fn initialize(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    fn one_of(column: &str, names: impl Iterator<Item = &'static str>) -> String {
        let quoted: Vec<String> = names.map(|name| format!("'{name}'")).collect();
        format!(
            "{column} TEXT NOT NULL CHECK ({column} IN ({}))",
            quoted.join(", ")
        )
    }
    let columns = [
        one_of("hair_front", HairFront::ALL.iter().map(|c| c.as_str())),
        one_of("hair_back", HairBack::ALL.iter().map(|c| c.as_str())),
        one_of("hair_colour", HairColour::ALL.iter().map(|c| c.as_str())),
        one_of("skin", Skin::ALL.iter().map(|c| c.as_str())),
        one_of("eyes", Eyes::ALL.iter().map(|c| c.as_str())),
        one_of("cheeks", Cheeks::ALL.iter().map(|c| c.as_str())),
        one_of("wear", Wear::ALL.iter().map(|c| c.as_str())),
        one_of("tile", Tile::ALL.iter().map(|c| c.as_str())),
    ];
    transaction.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS host_agent_looks (
            agent_id TEXT PRIMARY KEY REFERENCES host_agents(agent_id),
            {},
            tuft INTEGER NOT NULL CHECK (tuft IN (0, 1))
        ) STRICT;
        CREATE UNIQUE INDEX IF NOT EXISTS host_agent_looks_unique ON host_agent_looks(
            hair_front, hair_back, hair_colour, skin, eyes, cheeks, wear, tile, tuft
        );",
        columns.join(",\n            ")
    ))?;
    Ok(())
}

/// Gives `agent` the first look no other agent wears on its walk, and returns
/// it. Runs inside the transaction that makes the agent.
pub(in crate::host) fn assign(
    transaction: &Transaction<'_>,
    agent: AgentId,
) -> Result<Look, HostCatalogError> {
    let taken = taken(transaction)?;
    let digest = Sha256::digest(format!("{START_DOMAIN}{agent}"));
    let mut start = [0; 8];
    start.copy_from_slice(&digest[..8]);
    let start = u64::from_be_bytes(start) % LOOKS;
    let look = (0..LOOKS)
        .map(|step| Look::at((start + step * STRIDE) % LOOKS))
        .find(|look| !taken.contains(look))
        .ok_or_else(|| {
            HostCatalogError::Invalid(format!("all {LOOKS} looks are worn; no agent can be made"))
        })?;
    transaction.execute(
        "INSERT INTO host_agent_looks(
            agent_id, hair_front, hair_back, hair_colour, skin, eyes, cheeks, wear, tile, tuft
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            agent.to_string(),
            look.hair_front.as_str(),
            look.hair_back.as_str(),
            look.hair_colour.as_str(),
            look.skin.as_str(),
            look.eyes.as_str(),
            look.cheeks.as_str(),
            look.wear.as_str(),
            look.tile.as_str(),
            look.tuft,
        ],
    )?;
    Ok(look)
}

/// `agent`'s look, or `None` for an agent the Host gave none.
pub(super) fn read(
    connection: &Connection,
    agent: AgentId,
) -> Result<Option<Look>, HostCatalogError> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM host_agent_looks WHERE agent_id = ?1"),
            [agent.to_string()],
            row,
        )
        .optional()?
        .transpose()
}

const COLUMNS: &str = "hair_front, hair_back, hair_colour, skin, eyes, cheeks, wear, tile, tuft";

fn taken(connection: &Connection) -> Result<HashSet<Look>, HostCatalogError> {
    let mut statement = connection.prepare(&format!("SELECT {COLUMNS} FROM host_agent_looks"))?;
    let looks = statement.query_map([], row)?;
    let mut taken = HashSet::new();
    for look in looks {
        taken.insert(look??);
    }
    Ok(taken)
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Look, HostCatalogError>> {
    let names: [String; 8] = [
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ];
    let tuft: bool = row.get(8)?;
    Ok((|| {
        Ok(Look {
            hair_front: HairFront::parse(&names[0])?,
            hair_back: HairBack::parse(&names[1])?,
            hair_colour: HairColour::parse(&names[2])?,
            skin: Skin::parse(&names[3])?,
            eyes: Eyes::parse(&names[4])?,
            cheeks: Cheeks::parse(&names[5])?,
            wear: Wear::parse(&names[6])?,
            tile: Tile::parse(&names[7])?,
            tuft,
        })
    })())
}

/// Schema 54 for a catalog made before it: the look table, and a look for
/// every agent, oldest first, so the same catalog always upgrades the same way.
pub(super) fn give_every_agent_a_look(
    transaction: &Transaction<'_>,
) -> Result<(), HostCatalogError> {
    initialize(transaction)?;
    let agents: Vec<String> = transaction
        .prepare(
            "SELECT agent_id FROM host_agents
             WHERE agent_id NOT IN (SELECT agent_id FROM host_agent_looks)
             ORDER BY created_at_ms, agent_id",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for agent in agents {
        let agent = uuid::Uuid::parse_str(&agent).map_err(|error| {
            HostCatalogError::Invalid(format!("stored agent id `{agent}` is not a UUID: {error}"))
        })?;
        assign(transaction, AgentId::from_uuid(agent))?;
    }
    Ok(())
}
