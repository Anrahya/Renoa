//! Attaching a completed upload to a turn: the file is copied into the
//! agent's workspace under `attachments/`, the agent is told where it is, and
//! an image is also shown to the model.

use std::{
    fs::{self, File},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use renoa_agent::ContentBlock;
use uuid::Uuid;

use super::{FILE, UploadError, Uploads, read_record};
use crate::workspace::hex_sha256;

/// The folder of an agent's workspace that attached files are copied into.
const FOLDER: &str = "attachments";

/// The most bytes a file name may have on Linux file systems: the copy's name,
/// mark included, must fit it whatever name the command carried.
const MAX_NAME_BYTES: usize = 255;

/// The longest base64 image shown to the model: Anthropic's API refuses an
/// image over 5 MiB. A larger one is still copied into the workspace.
const MAX_INLINE_IMAGE_BASE64_BYTES: usize = 5 * 1024 * 1024;

impl Uploads {
    /// What a turn receives for upload `id`, attached under `name`: a line
    /// naming the copy in `workspace`, and the image when it is one. An
    /// upload this principal did not complete becomes a line saying so.
    /// Attaching the same upload again reuses the copy.
    ///
    /// # Errors
    ///
    /// Returns storage failures, and a workspace whose `attachments` is not a
    /// folder.
    pub async fn attach(
        &self,
        principal: Uuid,
        id: Uuid,
        name: String,
        workspace: PathBuf,
    ) -> Result<Vec<ContentBlock>, UploadError> {
        let directory = self.root.join(id.to_string());
        tokio::task::spawn_blocking(move || attach(&directory, principal, &name, &workspace))
            .await?
    }
}

fn attach(
    directory: &Path,
    principal: Uuid,
    name: &str,
    workspace: &Path,
) -> Result<Vec<ContentBlock>, UploadError> {
    let uploaded = read_record(directory)?.is_some_and(|record| record.principal == principal);
    // A completed upload's `file` is never changed or removed, so it is read
    // without the uploads lock.
    let bytes = match fs::read(directory.join(FILE)) {
        Ok(bytes) if uploaded => bytes,
        Ok(_) => return Ok(vec![missing(name)]),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![missing(name)]),
        Err(error) => return Err(error.into()),
    };
    let Some(path) = copy_in(workspace, name, &bytes)? else {
        return Ok(vec![ContentBlock::text(format!(
            "The attached file {name:?} arrived, but every name it could take in {FOLDER}/ holds another file."
        ))]);
    };
    let mut blocks = vec![ContentBlock::text(format!(
        "Attached file: {path} ({}).",
        shown_size(bytes.len())
    ))];
    if let Some(mime_type) = image_type(&bytes)
        && bytes.len().div_ceil(3) * 4 <= MAX_INLINE_IMAGE_BASE64_BYTES
    {
        blocks.push(ContentBlock::image(STANDARD.encode(&bytes), mime_type));
    }
    Ok(blocks)
}

fn missing(name: &str) -> ContentBlock {
    ContentBlock::text(format!(
        "The attached file {name:?} did not arrive, so it is not in your workspace."
    ))
}

/// Copies `bytes` into the workspace's attachments folder under `name`, or
/// under `name` marked with the start of its hash when another file has it.
/// A copy already there is reused. Returns the copy's path in the workspace,
/// or `None` when both names hold other files.
fn copy_in(workspace: &Path, name: &str, bytes: &[u8]) -> io::Result<Option<String>> {
    let folder = workspace.join(FOLDER);
    match fs::create_dir(&folder) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
        _ => {}
    }
    if !fs::symlink_metadata(&folder)?.is_dir() {
        return Err(io::Error::other(format!(
            "{FOLDER} in the agent's workspace is not a folder"
        )));
    }
    let digest = hex_sha256(bytes);
    let mark = &digest[..8];
    let (stem, extension) = file_name(name, mark.len() + 3);
    for candidate in [
        format!("{stem}{extension}"),
        format!("{stem} ({mark}){extension}"),
    ] {
        let target = folder.join(&candidate);
        // Checked twice: another turn may place the same name between the
        // first check and the copy.
        for _ in 0..2 {
            match fs::symlink_metadata(&target) {
                Ok(metadata) => {
                    if metadata.is_file() && hex_sha256(&fs::read(&target)?) == digest {
                        return Ok(Some(format!("{FOLDER}/{candidate}")));
                    }
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if place(&folder, &target, bytes)? {
                        return Ok(Some(format!("{FOLDER}/{candidate}")));
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(None)
}

/// Writes `bytes` to `target` unless something is already there: the copy is
/// written and synced aside, then linked into place, which never replaces.
/// Returns whether it was placed.
fn place(folder: &Path, target: &Path, bytes: &[u8]) -> io::Result<bool> {
    let aside = folder.join(format!(".{}.part", Uuid::new_v4()));
    let placed = (|| {
        let mut file = File::create_new(&aside)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::hard_link(&aside, target) {
            Ok(()) => File::open(folder)?.sync_all().map(|()| true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error),
        }
    })();
    let removed = fs::remove_file(&aside);
    let placed = placed?;
    removed?;
    Ok(placed)
}

/// `name` as a file name that stays in the attachments folder, split into a
/// stem and an extension, with room left for a mark of `reserve` bytes.
fn file_name(name: &str, reserve: usize) -> (String, String) {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    let cleaned = if cleaned.is_empty() { "file" } else { cleaned };
    let (stem, extension) = match cleaned.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty() && extension.len() + 1 + reserve < MAX_NAME_BYTES =>
        {
            (stem, format!(".{extension}"))
        }
        _ => (cleaned, String::new()),
    };
    let mut end = stem.len().min(MAX_NAME_BYTES - reserve - extension.len());
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    (stem[..end].to_owned(), extension)
}

/// The media type of an image a model can be shown, from its first bytes.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn shown_size(bytes: usize) -> String {
    const KIB: usize = 1024;
    match bytes {
        0..KIB => format!("{bytes} bytes"),
        KIB..1_048_576 => format!("{} KB", bytes.div_ceil(KIB)),
        _ => format!(
            "{}.{} MB",
            bytes / 1_048_576,
            bytes % 1_048_576 * 10 / 1_048_576
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{file_name, image_type, shown_size};

    #[test]
    fn a_name_stays_in_the_folder_and_fits_with_its_mark() {
        assert_eq!(
            file_name("fridge.jpg", 11),
            ("fridge".into(), ".jpg".into())
        );
        let (stem, extension) = file_name("../../etc/passwd", 11);
        assert_eq!(format!("{stem}{extension}"), "_.._etc_passwd");
        assert_eq!(file_name(".env", 11), ("env".into(), String::new()));
        assert_eq!(file_name(" .. ", 11), ("file".into(), String::new()));
        assert_eq!(file_name("a\nb.txt", 11), ("a_b".into(), ".txt".into()));
        let (stem, extension) = file_name(&format!("{}.pdf", "é".repeat(200)), 11);
        assert!(stem.len() + extension.len() + 11 <= 255);
        assert_eq!(extension, ".pdf");
    }

    #[test]
    fn images_are_known_by_their_bytes() {
        assert_eq!(image_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_type(b"GIF89a.."), Some("image/gif"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(b"%PDF-1.7"), None);
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(shown_size(900), "900 bytes");
        assert_eq!(shown_size(312 * 1024 - 5), "312 KB");
        assert_eq!(shown_size(3 * 1_048_576 + 524_288), "3.5 MB");
    }
}
