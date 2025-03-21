use anyhow::{Context, Result};
use clap::Parser;
use hyle::{
    entrypoint::RunPg,
    log_error,
    utils::{
        conf::{self, P2pMode},
        logger::setup_tracing,
    },
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(long, default_value = "config.toml")]
    pub config_file: Option<String>,

    #[clap(long, action)]
    pub pg: bool,
}

#[cfg(feature = "dhat")]
#[global_allocator]
/// Use dhat to profile memory usage
static ALLOC: dhat::Alloc = dhat::Alloc;

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(feature = "dhat")]
    let _profiler = {
        tracing::info!("Running with dhat memory profiler");
        dhat::Profiler::new_heap()
    };

    let args = Args::parse();
    let mut config =
        conf::Conf::new(args.config_file, None, Some(true)).context("reading config file")?;
    // The indexer binary runs none of the consensus/p2p layer
    config.p2p.mode = P2pMode::None;
    // The indexer binary skips the TCP server
    config.tcp_server_port = None;

    setup_tracing(&config, format!("{}(nopkey)", config.id.clone()))?;

    let _pg = if args.pg {
        Some(RunPg::new(&mut config).await?)
    } else {
        None
    };

    log_error!(
        hyle::entrypoint::main_process(config, None).await,
        "Error running hyle indexer"
    )?;

    Ok(())
}
