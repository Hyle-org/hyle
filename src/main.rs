use clap::Parser;
use tracing::{error, info};

mod client;
mod ctx;
mod model;
mod rest_endpoints;
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

    let rpc_addr = "127.0.0.1:1234";
    let rest_addr = "127.0.0.1:4321";

    if args.client.unwrap_or(false) {
        info!("client mode");
        client::client(rpc_addr).await?;
    }

    info!("server mode");
    // Start RPC server
    let rpc_server = tokio::spawn(async move {
        if let Err(e) = server::rpc_server(rpc_addr).await {
            error!("RPC server failed: {:?}", e);
            Err(e)
        } else {
            Ok(())
        }
    });

    // Start REST server
    let rest_server = tokio::spawn(async move {
        if let Err(e) = server::rest_server(rest_addr).await {
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
