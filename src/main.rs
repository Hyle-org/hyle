use anyhow::{Context, Result};
use clap::Parser;
use tracing::{error, info};

mod client;
mod conf;
mod ctx;
mod logger;
mod model;
mod p2p;
mod rest_endpoints;
mod server;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    client: Option<bool>,

    #[arg(long, default_value = "master.ron")]
    config_file: String,
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let config = conf::Conf::new(args.config_file)?;
    info!("Starting node with config: {:?}", config);

    let rpc_addr = config.addr();
    let rest_addr = config.rest_addr().to_string();

    if args.client.unwrap_or(false) {
        info!("client mode");
        client::client(&rpc_addr).await?;
    }

    info!("server mode");

    tokio::spawn(async move {
        if let Err(e) = server::p2p_server(&rpc_addr, &config).await {
            error!("RPC server failed: {:?}", e);
        }
    });

    // Start REST server
    server::rest_server(&rest_addr)
        .await
        .context("Starting REST server")?;

    Ok(())
}
