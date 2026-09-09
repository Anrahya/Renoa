use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;
use std::{io, process::Stdio};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _},
    process::{ChildStdout, Command},
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Serialize)]
pub(crate) struct GitPage {
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub encoding: &'static str,
    pub content: String,
}

pub(super) async fn page(
    command: Command,
    offset: u64,
    cancel: &CancellationToken,
) -> io::Result<GitPage> {
    read(command, cancel, async |mut stdout| {
        let skipped =
            tokio::io::copy(&mut (&mut stdout).take(offset), &mut tokio::io::sink()).await?;
        if skipped != offset {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "offset exceeds Git output",
            ));
        }
        // Reuse the workspace response size. This bounds a page, never the
        // underlying blob/diff, and the next byte remains addressable.
        let size = crate::output::MAX_TOOL_OUTPUT_BYTES;
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(u64::try_from(size).map_err(io::Error::other)? + 1)
            .read_to_end(&mut bytes)
            .await?;
        let more = bytes.len() > size;
        if more {
            bytes.truncate(size);
        }
        let (content, encoding) = match std::str::from_utf8(&bytes) {
            Ok(text) => (text.to_owned(), "utf8"),
            Err(error) if more && error.error_len().is_none() => {
                bytes.truncate(error.valid_up_to());
                (
                    String::from_utf8(bytes.clone()).map_err(io::Error::other)?,
                    "utf8",
                )
            }
            Err(_) => (STANDARD.encode(&bytes), "base64"),
        };
        let next = offset
            .checked_add(u64::try_from(bytes.len()).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("Git cursor overflow"))?;
        Ok((
            GitPage {
                offset,
                next_offset: more.then_some(next),
                encoding,
                content,
            },
            more,
        ))
    })
    .await
}

pub(super) async fn read<T, F, Fut>(
    mut command: Command,
    cancel: &CancellationToken,
    consume: F,
) -> io::Result<T>
where
    F: FnOnce(ChildStdout) -> Fut,
    Fut: std::future::Future<Output = io::Result<(T, bool)>>,
{
    if cancel.is_cancelled() {
        return Err(interrupted());
    }
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let pid = crate::process::child_pid_raw(&child).map_err(io::Error::other)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing Git stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing Git stderr"))?;
    let work = async {
        let errors = async {
            // Drain diagnostics without buffering source content or credentials.
            tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await
        };
        let reading = async {
            let (value, early) = consume(stdout).await?;
            if early {
                child.start_kill()?;
            }
            let status = child.wait().await?;
            if !early && !status.success() {
                return Err(io::Error::other(format!(
                    "Git inspection exited with {status}"
                )));
            }
            Ok(value)
        };
        let (value, _) = tokio::try_join!(reading, errors)?;
        Ok(value)
    };
    let result =
        tokio::select! { result = work => result, () = cancel.cancelled() => Err(interrupted()) };
    crate::process::stop_process_group_raw(&mut child, pid)
        .await
        .map_err(io::Error::other)?;
    result
}

// Git hunk headers contain two pairs of u64 line coordinates. Retain only their
// prefix; source lines of any length are streamed without allocating whole lines.
pub(super) async fn line_prefix(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> io::Result<Option<Vec<u8>>> {
    let mut prefix = Vec::new();
    let mut seen = false;
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            return Ok(seen.then_some(prefix));
        }
        seen = true;
        let end = buf.iter().position(|b| *b == b'\n');
        let n = end.map_or(buf.len(), |n| n + 1);
        prefix.extend_from_slice(&buf[..n.min(128_usize.saturating_sub(prefix.len()))]);
        reader.consume(n);
        if end.is_some() {
            return Ok(Some(prefix));
        }
    }
}

pub(super) async fn skip_line(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<bool> {
    Ok(line_prefix(reader).await?.is_some())
}

// Compare source lines without allocating a whole blob line. Normalize only
// LF/CRLF terminators, as Rust's str::lines does for the supplied quotation.
pub(super) async fn matches_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    expected: &[u8],
) -> io::Result<bool> {
    let mut remaining = expected;
    while !remaining.is_empty() {
        let bytes = reader.fill_buf().await?;
        let count = remaining.len().min(bytes.len());
        if count == 0 || bytes[..count] != remaining[..count] {
            return Ok(false);
        }
        reader.consume(count);
        remaining = &remaining[count..];
    }
    match reader.fill_buf().await?.first().copied() {
        None => Ok(!expected.is_empty()),
        Some(b'\n') => {
            reader.consume(1);
            Ok(true)
        }
        Some(b'\r') => {
            reader.consume(1);
            if reader.fill_buf().await?.first() != Some(&b'\n') {
                return Ok(false);
            }
            reader.consume(1);
            Ok(true)
        }
        Some(_) => Ok(false),
    }
}

fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Git inspection cancelled")
}
