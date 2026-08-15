use std::{env, error::Error, io};

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
        _ => Err(io::Error::other("usage: renoa-agent <acp|--version>").into()),
    }
}
