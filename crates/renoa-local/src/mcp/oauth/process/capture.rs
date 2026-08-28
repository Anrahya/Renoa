use std::io;

use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    sync::oneshot,
    task::JoinHandle,
};

use crate::{mcp::McpAdapterError, process::stop_process_group_raw};

pub(super) struct Captured {
    pub(super) bytes: Vec<u8>,
    pub(super) truncated: bool,
}

impl Drop for Captured {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

pub(super) fn drain(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    limit: usize,
    mut first_record: Option<oneshot::Sender<()>>,
) -> JoinHandle<io::Result<Captured>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8_192];
        let mut truncated = false;
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                return Ok(Captured { bytes, truncated });
            }
            let previous = bytes.len();
            let retained = read.min(limit.saturating_sub(previous));
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;
            if !truncated
                && bytes[previous..].contains(&b'\n')
                && let Some(sender) = first_record.take()
            {
                let _receiver_closed = sender.send(()).is_err();
            }
        }
    })
}

pub(super) async fn stop_and_capture(
    child: &mut tokio::process::Child,
    pid: u32,
    stdout: JoinHandle<io::Result<Captured>>,
    stderr: JoinHandle<io::Result<Captured>>,
) -> Result<(Captured, Captured), McpAdapterError> {
    let cleanup = stop_process_group_raw(child, pid)
        .await
        .map_err(|error| McpAdapterError::Cleanup(error.to_string()));
    let stdout = join(stdout, "stdout").await;
    let stderr = join(stderr, "stderr").await;
    cleanup?;
    Ok((stdout?, stderr?))
}

pub(super) async fn stop_and_discard(
    child: &mut tokio::process::Child,
    pid: u32,
    stdout: JoinHandle<io::Result<Captured>>,
    stderr: JoinHandle<io::Result<Captured>>,
) -> Result<(), McpAdapterError> {
    let (mut stdout, mut stderr) = stop_and_capture(child, pid, stdout, stderr).await?;
    stdout.bytes.fill(0);
    stderr.bytes.fill(0);
    Ok(())
}

async fn join(
    task: JoinHandle<io::Result<Captured>>,
    stream: &'static str,
) -> Result<Captured, McpAdapterError> {
    task.await
        .map_err(|source| McpAdapterError::ReaderTask(stream, source))?
        .map_err(|source| McpAdapterError::Read { stream, source })
}
