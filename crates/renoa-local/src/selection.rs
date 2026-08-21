use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{LocalHostError, PiReasoningLevel};

pub(crate) const SELECTION_FILE: &str = "runtime.jsonl";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RuntimeSelection {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) reasoning: PiReasoningLevel,
}

pub(crate) fn create_selection_log(
    directory: &Path,
    selection: &RuntimeSelection,
) -> Result<(), LocalHostError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(SELECTION_FILE))?;
    write_record(&mut file, selection)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) async fn append_selection(
    path: PathBuf,
    selection: RuntimeSelection,
) -> Result<(), LocalHostError> {
    tokio::task::spawn_blocking(move || {
        let mut file = OpenOptions::new().append(true).open(path)?;
        write_record(&mut file, &selection)?;
        file.sync_all()?;
        Ok(())
    })
    .await?
}

pub(crate) async fn read_selection(path: PathBuf) -> Result<RuntimeSelection, LocalHostError> {
    tokio::task::spawn_blocking(move || {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let complete_length = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        if complete_length != bytes.len() {
            file.set_len(u64::try_from(complete_length).map_err(|error| {
                LocalHostError::InvalidRequest(format!(
                    "session runtime metadata length is invalid: {error}"
                ))
            })?)?;
            file.sync_all()?;
        }
        let mut latest = None;
        for line in bytes[..complete_length].split(|byte| *byte == b'\n') {
            if !line.is_empty() {
                latest = Some(serde_json::from_slice(line)?);
            }
        }
        latest.ok_or_else(|| {
            LocalHostError::InvalidRequest(
                "session runtime metadata has no complete record".to_owned(),
            )
        })
    })
    .await?
}

fn write_record(file: &mut File, selection: &RuntimeSelection) -> Result<(), LocalHostError> {
    serde_json::to_writer(&mut *file, selection)?;
    file.write_all(b"\n")?;
    Ok(())
}
