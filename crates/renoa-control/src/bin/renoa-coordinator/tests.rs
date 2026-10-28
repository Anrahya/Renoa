use std::path::Path;

use super::*;

fn serve(control_room: Option<&str>) -> Result<Operation, String> {
    let home = tempfile::tempdir().expect("home directory");
    let mut arguments: Vec<OsString> = vec!["renoa-coordinator".into(), "serve".into()];
    arguments.push(home.path().into());
    arguments.extend(["7818", "localhost", "http://localhost"].map(OsString::from));
    arguments.extend(control_room.map(OsString::from));
    Operation::parse(arguments.into_iter())
}

#[test]
fn the_control_room_directory_is_optional_and_must_be_absolute() {
    assert!(matches!(
        serve(None),
        Ok(Operation::Serve {
            control_room: None,
            ..
        })
    ));
    assert!(matches!(
        serve(Some("/opt/renoa/control-room")),
        Ok(Operation::Serve { control_room: Some(path), .. })
            if path == Path::new("/opt/renoa/control-room")
    ));
    assert!(serve(Some("control-room")).is_err());
}
