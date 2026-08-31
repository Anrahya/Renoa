#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("renoa-telegram: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), renoa_telegram::TelegramServiceError> {
    let config = renoa_telegram::Config::from_environment().await?;
    renoa_telegram::run(config).await
}
