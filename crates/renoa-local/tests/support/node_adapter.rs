//! Builds a Node adapter for the tests that run it. nextest runs every test
//! in its own process, and the build starts by deleting `dist/`, so each
//! process building for itself deletes the entry point under another test.
//! A file lock serializes the build and a freshness check skips it once
//! `dist/` is newer than the sources, so one build serves the whole run.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
};

/// The adapter's entry point, built from its current sources.
pub(crate) fn built(root: &Path) -> PathBuf {
    let entry = root.join("dist/src/main.js");
    let lock = File::create(root.join("node_modules/.renoa-test-build.lock"))
        .expect("open the adapter build lock; install the adapter's dependencies first");
    lock.lock().expect("lock the adapter build");
    if !fresh(root, &entry) {
        let status = Command::new("pnpm")
            .arg("--dir")
            .arg(root)
            .arg("build")
            .status()
            .expect("run the adapter build");
        assert!(
            status.success(),
            "{} build failed: {status}",
            root.display()
        );
        assert!(entry.is_file(), "the build produced no {}", entry.display());
    }
    entry
}

fn fresh(root: &Path, entry: &Path) -> bool {
    let Ok(built) = fs::metadata(entry).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let mut pending = vec![
        root.join("src"),
        root.join("package.json"),
        root.join("tsconfig.json"),
    ];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            let entries = fs::read_dir(&path).expect("read the adapter sources");
            pending.extend(entries.map(|entry| entry.expect("read a source entry").path()));
        } else if metadata.modified().expect("read a source time") > built {
            return false;
        }
    }
    true
}
