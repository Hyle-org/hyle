use anyhow::Result;
use clap::Parser;
use tracing::{error, info};

mod client;
mod config;
mod ctx;
mod model;
mod rest_endpoints;
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
        client::client(config.addr()).await?;
    }

    info!("server mode");
    // Start RPC server
    let rpc_server = tokio::spawn(async move {
        if let Err(e) = server::rpc_server(config.rpc_addr()).await {
            error!("RPC server failed: {:?}", e);
            Err(e)
        } else {
            Ok(())
        }
    });

    // Start REST server
    let rest_server = tokio::spawn(async move {
        if let Err(e) = server::rest_server(config.rpc_addr()).await {
            error!("REST server failed: {:?}", e);
            Err(e)
        } else {
            Ok(())
        }
    });

    rpc_server.await??;
    rest_server.await??;

    Ok(())
}
