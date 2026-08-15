use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use renoa_local::PiReasoningLevel;
use serde::{Deserialize, Serialize};

use crate::ServerError;

pub(crate) const SELECTION_FILE: &str = "runtime.jsonl";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RuntimeSelection {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) reasoning: PiReasoningLevel,
}

pub(crate) async fn create_selection_log(
    path: PathBuf,
    selection: RuntimeSelection,
) -> Result<(), ServerError> {
    tokio::task::spawn_blocking(move || {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        write_record(&mut file, &selection)?;
        file.sync_all()?;
        File::open(parent(&path)?)?.sync_all()?;
        Ok::<_, ServerError>(())
    })
    .await?
}

pub(crate) async fn append_selection(
    path: PathBuf,
    selection: RuntimeSelection,
) -> Result<(), ServerError> {
    tokio::task::spawn_blocking(move || {
        let mut file = OpenOptions::new().append(true).open(path)?;
        write_record(&mut file, &selection)?;
        file.sync_all()?;
        Ok::<_, ServerError>(())
    })
    .await?
}

pub(crate) async fn read_selection(path: PathBuf) -> Result<RuntimeSelection, ServerError> {
    let bytes = tokio::fs::read(path).await?;
    let mut latest = None;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        latest = Some(serde_json::from_slice(&line[..line.len() - 1])?);
    }
    latest.ok_or_else(|| {
        ServerError::InvalidRequest("session runtime metadata has no complete record".to_owned())
    })
}

fn write_record(file: &mut File, selection: &RuntimeSelection) -> Result<(), ServerError> {
    serde_json::to_writer(&mut *file, selection)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn parent(path: &Path) -> Result<&Path, ServerError> {
    path.parent()
        .ok_or_else(|| ServerError::Configuration("session runtime path has no parent".to_owned()))
}
