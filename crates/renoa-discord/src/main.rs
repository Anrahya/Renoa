#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: renoa-discord <config.json>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: renoa-discord <config.json>");
        std::process::exit(2);
    }
    if let Err(error) = renoa_discord::run(std::path::Path::new(&path)).await {
        eprintln!("renoa-discord: {error}");
        std::process::exit(1);
    }
}
