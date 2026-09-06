use std::{env, path::Path};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let [action, path] = args.as_slice() else {
        return Err("usage: renoa-slack <run|inspect> /absolute/config.json".into());
    };
    if !matches!(action.as_str(), "run" | "inspect") {
        return Err("usage: renoa-slack <run|inspect> /absolute/config.json".into());
    }
    let config = renoa_slack::Config::read(Path::new(path))?;
    if action == "inspect" {
        println!(
            "{}",
            serde_json::to_string_pretty(&renoa_slack::inspect(&config)?)?
        );
        return Ok(());
    }
    let shutdown = CancellationToken::new();
    let service = renoa_slack::run(config, shutdown.clone());
    tokio::pin!(service);
    tokio::select! {
        result=&mut service=>result?,
        result=stop_signal()=>{ result?; shutdown.cancel(); service.await?; }
    }
    Ok(())
}

async fn stop_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {result=tokio::signal::ctrl_c()=>result, _=terminate.recv()=>Ok(())}
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
