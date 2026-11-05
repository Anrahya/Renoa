//! Files a principal sends its agents.
//!
//! A surface uploads a file in parts through the owner's Host calls, then a
//! command attaches it ([`Uploads::attach`]). Each upload is a directory named
//! by its id under `state/uploads`: `upload.json` records who uploaded it, its
//! size and its SHA-256; `partial` grows by whole parts, in order; once every
//! byte has arrived and the hash matches, `partial` is renamed `file`. A part
//! is on disk before its call is answered, and a part sent again is written
//! again, so a caller retries any part it is unsure of and resumes from the
//! `received` an answer reports.

mod attach;

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use renoa_home::RenoaHome;

use crate::workspace::hex_sha256;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// The size of every part but the last: the largest power of two whose base64
/// body, inside the frame that carries it to the node, fits one 1 MiB RCP
/// message.
pub const UPLOAD_PART_BYTES: usize = 512 * 1024;

/// [`UPLOAD_PART_BYTES`] as a file offset.
const PART: u64 = UPLOAD_PART_BYTES as u64;

/// The largest file a principal may upload. A product limit, not a transport
/// one: fifty parts.
pub const MAX_UPLOAD_BYTES: u64 = 50 * PART;

const RECORD: &str = "upload.json";
const PARTIAL: &str = "partial";
const FILE: &str = "file";
const LOCK: &str = ".lock";

/// Where an upload stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UploadStatus {
    pub size: u64,
    /// Bytes stored so far: whole parts until the last one arrives.
    pub received: u64,
    pub complete: bool,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UploadError {
    #[error("{0}")]
    Invalid(String),
    #[error("No upload has this id; start it first.")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("upload storage failed: {0}")]
    Io(#[from] io::Error),
    #[error("upload storage task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    principal: Uuid,
    size: u64,
    sha256: String,
}

/// The uploads of one Renoa home.
#[derive(Clone, Debug)]
pub struct Uploads {
    root: PathBuf,
}

impl Uploads {
    #[must_use]
    pub fn open(home: &RenoaHome) -> Self {
        Self {
            root: home.uploads(),
        }
    }

    /// Starts upload `id` of `size` bytes whose SHA-256 is `sha256`, or
    /// reports where it stands when the same principal already started it
    /// with the same size and hash.
    ///
    /// # Errors
    ///
    /// Refuses a size outside 1 byte to [`MAX_UPLOAD_BYTES`], a hash that is
    /// not 64 lowercase hex digits, and an id already started otherwise;
    /// returns storage failures. A refused start stores nothing.
    pub async fn start(
        &self,
        principal: Uuid,
        id: Uuid,
        size: u64,
        sha256: String,
    ) -> Result<UploadStatus, UploadError> {
        if size == 0 || size > MAX_UPLOAD_BYTES {
            return Err(UploadError::Invalid(format!(
                "An upload is 1 byte to {} MiB.",
                MAX_UPLOAD_BYTES / (1024 * 1024)
            )));
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(UploadError::Invalid(
                "sha256 is the file's SHA-256 as 64 lowercase hex digits.".to_owned(),
            ));
        }
        let root = self.root.clone();
        let record = Record {
            principal,
            size,
            sha256,
        };
        tokio::task::spawn_blocking(move || {
            let _lock = lock(&root)?;
            start_locked(&root, id, &record)
        })
        .await?
    }

    /// Stores part `index` of upload `id`, then completes the upload once
    /// every byte has arrived and the hash matches.
    ///
    /// # Errors
    ///
    /// `NotFound` for an upload this principal did not start; `Invalid` for a
    /// part number past the end, a part of the wrong size, or a completed
    /// file whose hash does not match, which also removes the upload;
    /// `Conflict` for a part past the one expected next or a part of a
    /// completed upload sent again with other bytes; and storage failures.
    pub async fn put_part(
        &self,
        principal: Uuid,
        id: Uuid,
        index: u64,
        data: Vec<u8>,
    ) -> Result<UploadStatus, UploadError> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock(&root)?;
            put_locked(&root.join(id.to_string()), principal, index, &data)
        })
        .await?
    }
}

/// Serializes every change to the uploads of one home, across processes.
fn lock(root: &Path) -> io::Result<File> {
    fs::create_dir_all(root)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK))?;
    lock.lock()?;
    Ok(lock)
}

fn start_locked(root: &Path, id: Uuid, record: &Record) -> Result<UploadStatus, UploadError> {
    let directory = root.join(id.to_string());
    match read_record(&directory)? {
        Some(stored) if stored == *record => status(&directory, &stored),
        Some(_) => Err(UploadError::Conflict(
            "This upload id is already in use; start again with a new id.".to_owned(),
        )),
        None => {
            // A directory without a record is what a crash left while
            // removing an upload; nobody holds a status from it.
            remove_if_present(&directory)?;
            let staging = root.join(format!(".staging-{id}"));
            let created = create(&staging, record).and_then(|()| {
                fs::rename(&staging, &directory)?;
                File::open(root)?.sync_all()
            });
            if let Err(error) = created {
                let _ = fs::remove_dir_all(&staging);
                return Err(error.into());
            }
            Ok(UploadStatus {
                size: record.size,
                received: 0,
                complete: false,
            })
        }
    }
}

/// Writes a new upload's record and empty `partial` into `staging`, synced.
fn create(staging: &Path, record: &Record) -> io::Result<()> {
    // A staging directory left by a crash holds nothing anyone was told of.
    remove_if_present(staging)?;
    fs::create_dir(staging)?;
    let mut file = File::create_new(staging.join(RECORD))?;
    file.write_all(&serde_json::to_vec(record)?)?;
    file.sync_all()?;
    File::create_new(staging.join(PARTIAL))?.sync_all()?;
    File::open(staging)?.sync_all()
}

fn remove_if_present(directory: &Path) -> io::Result<()> {
    match fs::remove_dir_all(directory) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn read_record(directory: &Path) -> Result<Option<Record>, UploadError> {
    match fs::read(directory.join(RECORD)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).map_err(io::Error::from)?,
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn status(directory: &Path, record: &Record) -> Result<UploadStatus, UploadError> {
    if directory.join(FILE).exists() {
        return Ok(complete(record));
    }
    let length = fs::metadata(directory.join(PARTIAL))?.len();
    Ok(UploadStatus {
        size: record.size,
        received: committed(length, record.size)?,
        complete: false,
    })
}

const fn complete(record: &Record) -> UploadStatus {
    UploadStatus {
        size: record.size,
        received: record.size,
        complete: true,
    }
}

/// Where the next part goes in `partial`: after its whole parts. A shorter
/// tail is a part a crash cut short, and a full `partial` not yet renamed
/// lacks only its check; either way the part it ends in is sent again.
fn committed(length: u64, size: u64) -> Result<u64, UploadError> {
    if length > size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "an upload holds more bytes than its size",
        )
        .into());
    }
    // A size is never 0, so the last part starts below it.
    let length = if length == size { size - 1 } else { length };
    Ok(length - length % PART)
}

fn put_locked(
    directory: &Path,
    principal: Uuid,
    index: u64,
    data: &[u8],
) -> Result<UploadStatus, UploadError> {
    let record = read_record(directory)?
        .filter(|record| record.principal == principal)
        .ok_or(UploadError::NotFound)?;
    let offset = index
        .checked_mul(PART)
        .filter(|offset| *offset < record.size)
        .ok_or_else(|| {
            UploadError::Invalid(format!(
                "This upload has {} parts, numbered from 0.",
                record.size.div_ceil(PART)
            ))
        })?;
    let expected = (record.size - offset).min(PART);
    if u64::try_from(data.len()).ok() != Some(expected) {
        return Err(UploadError::Invalid(format!(
            "Part {index} of this upload is {expected} bytes."
        )));
    }
    if directory.join(FILE).exists() {
        let mut file = File::open(directory.join(FILE))?;
        return if holds(&mut file, offset, data)? {
            Ok(complete(&record))
        } else {
            Err(UploadError::Conflict(format!(
                "Part {index} was already stored with other bytes; start again with a new id."
            )))
        };
    }
    let mut partial = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join(PARTIAL))?;
    let received = committed(partial.metadata()?.len(), record.size)?;
    if offset > received {
        return Err(UploadError::Conflict(format!(
            "Send part {} of this upload next.",
            received / PART
        )));
    }
    // A part sent again is written again: a crash may have left it torn
    // without changing its length, and the hash at the end catches a part
    // whose bytes really changed.
    partial.seek(SeekFrom::Start(offset))?;
    partial.write_all(data)?;
    partial.sync_data()?;
    let received = received.max(offset + expected);
    if received < record.size {
        return Ok(UploadStatus {
            size: record.size,
            received,
            complete: false,
        });
    }
    finish(directory, &record, partial)
}

/// Whether `file` holds `data` at `offset`.
fn holds(file: &mut File, offset: u64, data: &[u8]) -> io::Result<bool> {
    let mut stored = vec![0; data.len()];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut stored)?;
    Ok(stored == data)
}

/// Completes an upload whose bytes have all arrived: renames `partial` to
/// `file` when the hash matches, and removes the upload when it does not.
fn finish(
    directory: &Path,
    record: &Record,
    mut partial: File,
) -> Result<UploadStatus, UploadError> {
    let mut bytes = Vec::new();
    partial.seek(SeekFrom::Start(0))?;
    partial.read_to_end(&mut bytes)?;
    drop(partial);
    if hex_sha256(&bytes) != record.sha256 {
        fs::remove_dir_all(directory)?;
        return Err(UploadError::Invalid(
            "The file's SHA-256 does not match the one the upload started with; start again with a new id."
                .to_owned(),
        ));
    }
    fs::rename(directory.join(PARTIAL), directory.join(FILE))?;
    File::open(directory)?.sync_all()?;
    Ok(complete(record))
}

#[cfg(test)]
mod tests;
