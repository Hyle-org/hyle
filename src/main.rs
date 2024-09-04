use anyhow::Result;
use clap::Parser;
use tracing::info;

mod client;
mod config;
mod ctx;
mod model;
mod server;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    client: Option<bool>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let config = config::read("config.ron").await?;

    if args.client.unwrap_or(false) {
        info!("client mode");
        client::client(&config.addr).await?;
    }

    info!("server mode");
    return server::server(&config.addr).await;
}
