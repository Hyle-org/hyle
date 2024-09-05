use anyhow::{Context, Result};
use clap::Parser;
use tracing::{error, info};

mod client;
mod config;
mod ctx;
mod model;
mod p2p_network;
mod rest_endpoints;
mod server;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    client: Option<bool>,

    #[arg(short, long)]
    id: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let config = config::read("config.ron").await?;
    info!("Config: {:?}", config);

    let rpc_addr = config.addr(args.id).context("peer id")?.to_string();
    let rest_addr = config.rest_addr().to_string();

    if args.client.unwrap_or(false) {
        info!("client mode");
        client::client(&rpc_addr).await?;
    }

    info!("server mode");
    // Start RPC server
    let p2p_server = tokio::spawn(async move {
        if let Err(e) = server::p2p_server(&rpc_addr, &config).await {
            error!("RPC server failed: {:?}", e);
            Err(e)
        } else {
            Ok(())
        }
    });

    // Start REST server
    let rest_server = tokio::spawn(async move {
        if let Err(e) = server::rest_server(&rest_addr).await {
            error!("REST server failed: {:?}", e);
            Err(e)
        } else {
            Ok(())
        }
    });

    p2p_server.await??;
    rest_server.await??;

    Ok(())
}
