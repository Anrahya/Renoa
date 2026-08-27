use std::{
    env,
    error::Error,
    io::{self, Write as _},
};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [argument] if argument == "--version" => {
            println!("renoa-agent {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [argument] if argument == "acp" => {
            renoa_acp::serve_stdio(renoa_acp::Config::from_environment()?).await?;
            Ok(())
        }
        [command, format] if command == "models" && format == "--json" => {
            let catalog = renoa_acp::configured_model_catalog().await?;
            let mut stdout = io::stdout().lock();
            serde_json::to_writer(&mut stdout, &catalog)?;
            stdout.write_all(b"\n")?;
            Ok(())
        }
        [mcp, github, install, account_flag, account]
            if mcp == "mcp"
                && github == "github"
                && install == "install"
                && account_flag == "--account" =>
        {
            let installed = renoa_acp::install_github_mcp(account).await?;
            let mut stdout = io::stdout().lock();
            serde_json::to_writer(&mut stdout, &installed)?;
            stdout.write_all(b"\n")?;
            Ok(())
        }
        _ => Err(io::Error::other(
            "usage: renoa-agent <acp|models --json|mcp github install --account ACCOUNT|--version>",
        )
        .into()),
    }
}
