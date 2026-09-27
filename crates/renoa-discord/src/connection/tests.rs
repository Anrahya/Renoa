use std::os::unix::fs::PermissionsExt as _;

use super::*;

fn request() -> DiscordConnectRequest {
    DiscordConnectRequest {
        operation_id: Uuid::new_v4(),
        bot_token: "bot.token-A_1".into(),
        guild_id: "10".into(),
        agent_id: Uuid::new_v4(),
    }
}

fn record(request: &DiscordConnectRequest) -> Connection {
    Connection {
        operation_id: request.operation_id,
        bot_name: "Renoa".into(),
        guild_id: Snowflake::parse(&request.guild_id).unwrap(),
        guild_name: "Home".into(),
        operator_user_id: Snowflake::parse("20").unwrap(),
        agent_id: request.agent_id,
        bot_token: request.bot_token.clone(),
    }
}

fn home() -> (tempfile::TempDir, RenoaHome) {
    let root = tempfile::tempdir().unwrap();
    let home = RenoaHome::at(root.path().join("home")).unwrap();
    home.initialize().unwrap();
    (root, home)
}

#[test]
fn publish_commits_once_privately_and_adopts_only_its_own_retry() {
    let (_root, home) = home();
    let first = request();
    assert!(Connection::read(&home).unwrap().is_none());
    record(&first).publish(&home, &first).unwrap();
    let path = home.discord_connection();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let saved = fs::read(&path).unwrap();

    let mut renamed = record(&first);
    renamed.guild_name = "Renamed since".into();
    let adopted = renamed.publish(&home, &first).unwrap();
    assert_eq!(
        adopted.guild_name, "Home",
        "a retry adopts the committed record"
    );

    let second = request();
    let error = record(&second).publish(&home, &second).err().unwrap();
    assert!(error.to_string().contains("already connected"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), saved);
    let names: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["discord.json"], "no temporary file survives");
}

#[test]
fn a_rejected_record_leaves_no_file() {
    let (_root, home) = home();
    let mut invalid = request();
    invalid.bot_token = "Bot token".into();
    assert!(record(&invalid).publish(&home, &invalid).is_err());
    assert_eq!(
        fs::read_dir(home.path().join("credentials"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn read_rejects_links_and_shared_files() {
    let (root, home) = home();
    let target = root.path().join("elsewhere.json");
    let first = request();
    fs::write(&target, serde_json::to_vec(&record(&first)).unwrap()).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&target, home.discord_connection()).unwrap();
    assert!(Connection::read(&home).is_err());
    fs::remove_file(home.discord_connection()).unwrap();

    record(&first).publish(&home, &first).unwrap();
    fs::set_permissions(home.discord_connection(), fs::Permissions::from_mode(0o640)).unwrap();
    let error = Connection::read(&home).err().unwrap();
    assert!(error.to_string().contains("group or other"), "{error}");
}
