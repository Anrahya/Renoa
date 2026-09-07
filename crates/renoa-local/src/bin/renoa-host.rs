use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, ReasoningLevel,
    arcee_profile,
};
use serde::Deserialize;
use std::{error::Error, path::PathBuf};
use tokio_util::sync::CancellationToken;

#[path = "renoa-host/github_review.rs"]
mod github_review;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    data_directory: PathBuf,
    model_bridge: PathBuf,
    providers: Vec<ModelProvider>,
    provider: ModelProvider,
    model: String,
    reasoning: Option<ReasoningLevel>,
    model_auth_store: PathBuf,
    mcp_adapter: Option<PathBuf>,
    mcp_registry_adapter: Option<PathBuf>,
    shared_plugin_registry: Option<String>,
    oauth_relay: Option<Relay>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Relay {
    origin: String,
    device_credential_file: PathBuf,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !(args.len() == 1
        || (args.len() == 6 && args[1] == "rename-bot")
        || (args.len() == 3
            && (args[1] == "github-review"
                || args[1] == "github-webhook"
                || args[1] == "github-execute")))
    {
        return Err(std::io::Error::other("usage: renoa-host <config.json> [rename-bot <agent-id> <expected-name> <name> <operation-id> | github-review <request.json> | github-webhook <envelope.json> | github-execute <execution.json>]").into());
    }
    let c: Config = serde_json::from_slice(&std::fs::read(&args[0])?)?;
    for path in [&c.data_directory, &c.model_bridge, &c.model_auth_store]
        .into_iter()
        .chain(c.mcp_adapter.iter())
        .chain(c.mcp_registry_adapter.iter())
        .chain(c.oauth_relay.iter().map(|r| &r.device_credential_file))
    {
        if !path.is_absolute() {
            return Err(std::io::Error::other("Host launch paths must be absolute").into());
        }
    }
    let mut models = LocalModelConfiguration::new(
        &c.model_bridge,
        c.providers,
        c.provider,
        c.model,
        &c.model_auth_store,
    );
    if let Some(reasoning) = c.reasoning {
        models = models.with_initial_reasoning(reasoning);
    }
    let mut adapters = LocalHostAdapters::new(c.mcp_adapter.as_deref())
        .with_mcp_registry(c.mcp_registry_adapter.as_deref())
        .with_shared_plugin_registry(c.shared_plugin_registry.as_deref());
    if let Some(relay) = &c.oauth_relay {
        adapters = adapters.with_oauth_relay(&relay.origin, &relay.device_credential_file);
    }
    let host = LocalHost::new(
        &c.data_directory,
        models,
        vec![arcee_profile(&c.data_directory)?],
        adapters,
    )?;
    if args.len() == 3 {
        return github_review::run(&host, &args[1], std::path::Path::new(&args[2])).await;
    }
    if args.len() == 6 {
        let text = |index: usize| {
            args[index]
                .to_str()
                .ok_or_else(|| std::io::Error::other("rename arguments must be UTF-8"))
        };
        let id = renoa_kernel::AgentId::from_uuid(uuid::Uuid::parse_str(text(2)?)?);
        let result = host
            .rename_bot(
                id,
                uuid::Uuid::parse_str(text(5)?)?,
                renoa_local::RenameBot {
                    id,
                    expected_name: text(3)?.to_owned(),
                    name: text(4)?.to_owned(),
                },
                CancellationToken::new(),
            )
            .await?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    let stop = CancellationToken::new();
    let runner = host.run_routines(stop.clone());
    tokio::pin!(runner);
    #[cfg(unix)]
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let signal = async {
        #[cfg(unix)]
        tokio::select! { result=tokio::signal::ctrl_c()=>result, _=termination.recv()=>Ok(()) }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await
    };
    eprintln!(
        "Renoa Host routine service starting: {}",
        host.host_id().await?
    );
    tokio::select! {
        result=&mut runner=>result?,
        result=signal=>{result?;stop.cancel();runner.await?;}
    }
    Ok(())
}
