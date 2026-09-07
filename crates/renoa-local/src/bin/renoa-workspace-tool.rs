//! Container entry point for Renoa's existing read/search tools. No credentials,
//! agent loop, shell execution or repository-specific policy live here.
use std::{
    error::Error,
    io::{Read as _, Write as _},
};

use renoa_agent::ToolCall;
use renoa_local::LocalWorkspace;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("workspace directory required")?;
    let workspace = tokio::task::spawn_blocking(move || LocalWorkspace::open(root)).await??;
    let input = tokio::task::spawn_blocking(|| {
        let mut bytes = Vec::new();
        std::io::stdin().take(1_048_577).read_to_end(&mut bytes)?;
        if bytes.len() > 1_048_576 {
            return Err(std::io::Error::other("tool input exceeds 1 MiB"));
        }
        Ok::<_, std::io::Error>(bytes)
    })
    .await??;
    let output = if input.is_empty() {
        serde_json::to_vec(&workspace.inspection_specs())?
    } else {
        let call: ToolCall = serde_json::from_slice(&input)?;
        serde_json::to_vec(&workspace.inspect(call, CancellationToken::new()).await?)?
    };
    std::io::stdout().write_all(&output)?;
    Ok(())
}
