//! `renoa-local` owns the rule that decides which text is the `/compact`
//! control. The crates that carry plain text to a Host session call it, so a
//! second spelling of the rule in them would let two surfaces disagree.
//!
//! This is a tripwire, not a parser: it catches a copy that spells the command
//! as the literal `"/compact"`, the way the rule itself does. A copy spelled
//! some other way (a trailing space, `concat!`) passes it.

use std::{fs, path::Path};

/// Crates that receive text for a Host session and must ask the Host what it
/// means. Slack and Telegram spell the control in their own platform syntax.
const CALLERS: [&str; 2] = ["renoa-acp", "renoa-node"];

fn rust_files(directory: &Path, found: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("read source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn only_the_host_spells_the_compact_rule() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for name in CALLERS {
        let mut files = Vec::new();
        rust_files(&crates.join(name).join("src"), &mut files);
        assert!(!files.is_empty(), "{name} has no sources to check");
        for file in files {
            let source = fs::read_to_string(&file).expect("read source file");
            // A trailing unit-test module may spell the command it exercises;
            // anything before it is production code.
            let production = source
                .rfind("#[cfg(test)]\nmod tests")
                .map_or(source.as_str(), |start| &source[..start]);
            if production.contains("\"/compact\"") {
                offenders.push(file.display().to_string());
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these files spell the /compact rule; call renoa_local::compact_invocation instead: {offenders:?}"
    );
}
