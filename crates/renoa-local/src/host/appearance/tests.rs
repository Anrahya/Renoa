use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use renoa_kernel::AgentId;
use rusqlite::Connection;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    Look, NewPicture, PictureChange, PictureError, look,
    look::{LOOKS, STRIDE},
    restore_schema_53,
};
use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalHost, ModelProvider,
    host::{HostInitialization, catalog},
    presets::GENERAL_PRESET_ID,
};

fn host(root: &Path) -> LocalHost {
    fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::Xai],
        initial_provider: ModelProvider::Xai,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        code_mode: None,
    })
    .expect("Host")
}

async fn make(host: &LocalHost, operation: Uuid, name: &str) -> AgentId {
    let request = AgentCreateRequest::from_preset(
        operation,
        AgentPresetId::new(GENERAL_PRESET_ID).expect("preset id"),
        name,
    )
    .with_instructions("Do the assigned job.");
    host.create_agent(
        AgentCreator::System {
            component: "test".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        request,
        CancellationToken::new(),
    )
    .await
    .expect("create agent")
    .id
}

async fn look_of(host: &LocalHost, agent: AgentId) -> Look {
    host.agent_appearance(agent)
        .await
        .expect("read appearance")
        .expect("the agent exists")
        .look
}

fn count(database: &Path, table: &str) -> i64 {
    Connection::open(database)
        .expect("catalog")
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count")
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nthe rest of a picture";

fn png(operation: Uuid, expected: Option<String>) -> PictureChange {
    PictureChange {
        operation_id: operation,
        expected_revision: expected,
        picture: Some(NewPicture {
            content_type: "image/png".to_owned(),
            bytes: PNG.to_vec(),
        }),
    }
}

#[test]
fn the_picker_can_reach_every_look_and_each_index_is_a_different_one() {
    fn gcd(a: u64, b: u64) -> u64 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    assert_eq!(gcd(STRIDE, LOOKS), 1, "the stride walks every look");
    let looks: HashSet<Look> = (0..LOOKS).map(Look::at).collect();
    assert_eq!(looks.len() as u64, LOOKS);
}

#[tokio::test]
async fn agents_never_share_a_look_and_a_replayed_creation_keeps_its_own() {
    let directory = tempdir().expect("fixture");
    let host = host(directory.path());
    let mut agents = Vec::new();
    for n in 0..40u128 {
        agents.push(make(&host, Uuid::from_u128(n + 1), &format!("Agent {n}")).await);
    }
    let mut looks = HashSet::new();
    for agent in &agents {
        assert!(
            looks.insert(look_of(&host, *agent).await),
            "no look is worn twice"
        );
    }
    let first = look_of(&host, agents[0]).await;
    assert_eq!(make(&host, Uuid::from_u128(1), "Agent 0").await, agents[0]);
    assert_eq!(
        look_of(&host, agents[0]).await,
        first,
        "a replay changes nothing"
    );
    assert_eq!(count(&host.config.database, "host_agent_looks"), 40);
}

#[tokio::test]
async fn an_agent_whose_first_look_is_worn_gets_the_next_one_on_its_walk() {
    let directory = tempdir().expect("fixture");
    let host = host(directory.path());
    let wearer = make(&host, Uuid::from_u128(1), "Wearer").await;
    let newcomer = make(&host, Uuid::from_u128(2), "Newcomer").await;
    let database = host.config.database.clone();
    let mut db = Connection::open(&database).expect("catalog");
    // What the newcomer would pick on an empty Host: its first look.
    let first = {
        let transaction = db.transaction().expect("transaction");
        transaction
            .execute("DELETE FROM host_agent_looks", [])
            .expect("clear looks");
        let first = look::assign(&transaction, newcomer).expect("pick");
        transaction.rollback().expect("rollback");
        first
    };
    // The wearer wears it; the newcomer must walk past it.
    let transaction = db.transaction().expect("transaction");
    transaction
        .execute("DELETE FROM host_agent_looks", [])
        .expect("clear looks");
    transaction
        .execute(
            "INSERT INTO host_agent_looks VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                wearer.to_string(),
                first.hair_front.as_str(),
                first.hair_back.as_str(),
                first.hair_colour.as_str(),
                first.skin.as_str(),
                first.eyes.as_str(),
                first.cheeks.as_str(),
                first.wear.as_str(),
                first.tile.as_str(),
                first.tuft,
            ],
        )
        .expect("the wearer wears it");
    let next = look::assign(&transaction, newcomer).expect("pick past it");
    assert_eq!(first, Look::at(look::start(newcomer)));
    assert_eq!(
        next,
        Look::at((look::start(newcomer) + STRIDE) % LOOKS),
        "the newcomer takes the next look on its walk"
    );
    // And the table itself refuses a second wearer.
    let second = transaction.execute(
        "UPDATE host_agent_looks SET hair_front = ?1, hair_back = ?2, hair_colour = ?3,
            skin = ?4, eyes = ?5, cheeks = ?6, wear = ?7, tile = ?8, tuft = ?9
         WHERE agent_id = ?10",
        rusqlite::params![
            first.hair_front.as_str(),
            first.hair_back.as_str(),
            first.hair_colour.as_str(),
            first.skin.as_str(),
            first.eyes.as_str(),
            first.cheeks.as_str(),
            first.wear.as_str(),
            first.tile.as_str(),
            first.tuft,
            newcomer.to_string(),
        ],
    );
    assert!(second.is_err(), "two agents cannot share a look");
}

#[tokio::test]
async fn schema_54_gives_every_agent_of_a_schema_53_catalog_its_own_look() {
    let directory = tempdir().expect("fixture");
    let host = host(directory.path());
    let mut agents = Vec::new();
    for n in 0..5u128 {
        agents.push(make(&host, Uuid::from_u128(n + 1), &format!("Agent {n}")).await);
    }
    let database = host.config.database.clone();
    let db = Connection::open(&database).expect("catalog");
    restore_schema_53(&db);
    db.execute_batch(
        "UPDATE host_metadata SET schema_version = 53 WHERE singleton = 1;
         PRAGMA user_version = 53;",
    )
    .expect("a schema 53 catalog");
    drop(db);
    catalog::initialize(&database).expect("upgrade schema 53");
    catalog::initialize(&database).expect("reopen the upgraded catalog");
    let mut looks = HashSet::new();
    for agent in &agents {
        assert!(looks.insert(look_of(&host, *agent).await));
    }
    assert_eq!(count(&database, "host_agent_pictures"), 0);
}

#[tokio::test]
async fn a_picture_is_set_once_per_operation_and_only_over_the_one_expected() {
    let directory = tempdir().expect("fixture");
    let host = host(directory.path());
    let agent = make(&host, Uuid::from_u128(1), "Chef").await;
    let set = Uuid::from_u128(10);

    let revision = host
        .set_agent_picture(agent, png(set, None))
        .await
        .expect("set")
        .expect("the agent exists")
        .expect("a revision");
    assert_eq!(revision, crate::workspace::hex_sha256(PNG));
    let stored = host.agent_picture(agent).await.expect("read").expect("set");
    assert_eq!(
        (stored.content_type.as_str(), stored.bytes.as_slice()),
        ("image/png", PNG)
    );
    let appearance = host
        .agent_appearance(agent)
        .await
        .expect("read")
        .expect("agent");
    assert_eq!(
        appearance.picture_revision.as_deref(),
        Some(revision.as_str())
    );

    // The same change again answers as the first time.
    assert_eq!(
        host.set_agent_picture(agent, png(set, None))
            .await
            .expect("replay"),
        Some(Some(revision.clone()))
    );
    // The same id for another change is refused.
    assert!(matches!(
        host.set_agent_picture(agent, png(set, Some(revision.clone()))).await,
        Err(PictureError::OperationReused(id)) if id == set
    ));
    // A change that expected no picture, now there is one, is stale.
    assert!(matches!(
        host.set_agent_picture(agent, png(Uuid::from_u128(11), None)).await,
        Err(PictureError::Stale { current: Some(current) }) if current == revision
    ));
    assert_eq!(
        count(&host.config.database, "host_agent_picture_operations"),
        1
    );
    let receipt: String = Connection::open(&host.config.database)
        .expect("catalog")
        .query_row(
            "SELECT request_json FROM host_agent_picture_operations",
            [],
            |row| row.get(0),
        )
        .expect("receipt");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&receipt).expect("receipt json"),
        serde_json::json!({
            "agent_id": agent.to_string(),
            "expected_revision": null,
            "revision": revision,
            "content_type": "image/png",
        }),
        "a receipt keeps the revision, never the bytes"
    );

    let cleared = PictureChange {
        operation_id: Uuid::from_u128(12),
        expected_revision: Some(revision),
        picture: None,
    };
    assert_eq!(
        host.set_agent_picture(agent, cleared.clone())
            .await
            .expect("clear"),
        Some(None)
    );
    assert_eq!(host.agent_picture(agent).await.expect("read"), None);
    assert_eq!(count(&host.config.database, "host_agent_pictures"), 0);
    assert_eq!(
        host.set_agent_picture(agent, cleared)
            .await
            .expect("replay"),
        Some(None),
        "a replayed clear answers as the first time"
    );
}

#[test]
fn the_stored_names_of_every_choice_stay_as_catalogs_hold_them() {
    fn names<T: Copy>(all: &[T], name: fn(T) -> &'static str) -> Vec<&'static str> {
        all.iter().copied().map(name).collect()
    }
    // A catalog checks these names; changing one is a schema change.
    assert_eq!(
        [
            names(super::HairFront::ALL, super::HairFront::as_str),
            names(super::HairBack::ALL, super::HairBack::as_str),
            names(super::HairColour::ALL, super::HairColour::as_str),
            names(super::Skin::ALL, super::Skin::as_str),
            names(super::Eyes::ALL, super::Eyes::as_str),
            names(super::Cheeks::ALL, super::Cheeks::as_str),
            names(super::Wear::ALL, super::Wear::as_str),
            names(super::Tile::ALL, super::Tile::as_str),
        ],
        [
            vec!["short", "fringe", "side_part", "curls", "princess"],
            vec!["none", "bun", "long", "twin_tails"],
            vec!["ink", "brown", "auburn", "blonde", "grey"],
            vec!["fair", "light", "medium", "deep"],
            vec!["round", "oval", "lashes", "sparkle", "cat"],
            vec!["none", "blush", "freckles", "lines"],
            vec![
                "none",
                "glasses",
                "bow",
                "clip",
                "beanie",
                "headphones",
                "cat_ears"
            ],
            vec!["clay", "cobalt", "ochre"],
        ]
    );
}

/// Two agent ids whose walks start at the same look, the first found first.
fn sharing_a_start() -> (AgentId, AgentId) {
    let mut starts = HashMap::new();
    (1..)
        .find_map(|n| {
            let agent = AgentId::from_uuid(Uuid::from_u128(n));
            starts
                .insert(look::start(agent), agent)
                .map(|other| (other, agent))
        })
        .expect("two walks share a start")
}

#[tokio::test]
async fn schema_54_walks_the_oldest_agent_first_and_breaks_a_tie_by_agent_id() {
    let (a, b) = sharing_a_start();
    let (low, high) = if a.to_string() < b.to_string() {
        (a, b)
    } else {
        (b, a)
    };
    // (created_at_ms of `low`, of `high`, the one that takes the shared start)
    for (low_at, high_at, first) in [(2, 1, high), (1, 1, low)] {
        let directory = tempdir().expect("fixture");
        let host = host(directory.path());
        let database = host.config.database.clone();
        let db = Connection::open(&database).expect("catalog");
        restore_schema_53(&db);
        for (agent, at) in [(low, low_at), (high, high_at)] {
            crate::test_agents::insert_agent(&database, &agent.to_string());
            db.execute(
                "UPDATE host_agents SET created_at_ms = ?1 WHERE agent_id = ?2",
                rusqlite::params![at, agent.to_string()],
            )
            .expect("creation time");
        }
        db.execute_batch(
            "UPDATE host_metadata SET schema_version = 53 WHERE singleton = 1;
             PRAGMA user_version = 53;",
        )
        .expect("a schema 53 catalog");
        drop(db);
        catalog::initialize(&database).expect("upgrade schema 53");
        let second = if first == low { high } else { low };
        let start = look::start(first);
        assert_eq!(look_of(&host, first).await, Look::at(start));
        assert_eq!(
            look_of(&host, second).await,
            Look::at((start + STRIDE) % LOOKS)
        );
    }
}

#[tokio::test]
async fn a_refused_picture_stores_nothing() {
    let directory = tempdir().expect("fixture");
    let host = host(directory.path());
    let agent = make(&host, Uuid::from_u128(1), "Chef").await;
    let with = |content_type: &str, bytes: Vec<u8>| PictureChange {
        operation_id: Uuid::from_u128(20),
        expected_revision: None,
        picture: Some(NewPicture {
            content_type: content_type.to_owned(),
            bytes,
        }),
    };
    let refused = [
        with("image/jpeg", PNG.to_vec()),
        with("image/gif", b"GIF89a".to_vec()),
        with("image/webp", b"RIFF\0\0\0\0WAVEfmt ".to_vec()),
        with("image/png", Vec::new()),
        with("image/png", {
            let mut big = PNG.to_vec();
            big.resize(super::MAX_PICTURE_BYTES + 1, 0);
            big
        }),
        PictureChange {
            operation_id: Uuid::nil(),
            ..png(Uuid::nil(), None)
        },
    ];
    for change in refused {
        let outcome = host.set_agent_picture(agent, change).await;
        assert!(
            matches!(outcome, Err(PictureError::Invalid(_))),
            "{outcome:?}"
        );
    }
    let webp = with("image/webp", b"RIFF\0\0\0\0WEBPVP8 ".to_vec());
    assert!(
        host.set_agent_picture(agent, webp).await.is_ok(),
        "a real WebP is kept"
    );
    assert_eq!(
        count(&host.config.database, "host_agent_picture_operations"),
        1
    );

    let stranger = AgentId::from_uuid(Uuid::from_u128(99));
    assert_eq!(
        host.set_agent_picture(stranger, png(Uuid::from_u128(21), None))
            .await
            .expect("an unknown agent is no error"),
        None
    );
    assert_eq!(host.agent_appearance(stranger).await.expect("read"), None);
    assert_eq!(
        count(&host.config.database, "host_agent_picture_operations"),
        1
    );
}
