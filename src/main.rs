use clap::Parser;
use tracing::info;

mod client;
mod ctx;
mod model;
mod server;

use anyhow::Result;

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

    let addr = "127.0.0.1:1234";

    if args.client.unwrap_or(false) {
        info!("client mode");
        client::client(addr).await?;
    }

    info!("server mode");
    return server::server(addr).await;
}
