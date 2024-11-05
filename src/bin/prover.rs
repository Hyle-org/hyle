use anyhow::{Context, Result};
use axum::Router;
use clap::Parser;
use hyle::{
    bus::{metrics::BusMetrics, SharedMessageBus},
    model::{CommonRunContext, NodeRunContext, SharedRunContext},
    p2p::P2P,
    prover::Prover,
    utils::{
        conf,
        crypto::BlstCrypto,
        logger::{setup_tracing, TracingMode},
        modules::ModulesHandler,
    },
};
use std::sync::{Arc, Mutex};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(long, default_value = "config.ron")]
    pub config_file: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = Arc::new(
        conf::Conf::new(args.config_file, None, Some(true)).context("reading config file")?,
    );

    setup_tracing(
        match config.log_format.as_str() {
            "json" => TracingMode::Json,
            "node" => TracingMode::NodeName,
            _ => TracingMode::Full,
        },
        config.id.clone(),
    )?;

    info!("Starting prover with config: {:?}", &config);

    let bus = SharedMessageBus::new(BusMetrics::global(config.id.clone()));
    let crypto = Arc::new(BlstCrypto::new(config.id.clone()));

    std::fs::create_dir_all(&config.data_directory).context("creating data directory")?;

    let mut handler = ModulesHandler::default();

    let ctx = SharedRunContext {
        common: CommonRunContext {
            bus: bus.new_handle(),
            config: config.clone(),
            router: Mutex::new(Some(Router::new())),
        }
        .into(),
        node: NodeRunContext { crypto }.into(),
    };

    handler.build_module::<P2P>(ctx.clone()).await?;
    handler.build_module::<Prover>(ctx.clone()).await?;

    let (running_modules, abort) = handler.start_modules()?;

    #[cfg(unix)]
    {
        use tokio::signal::unix;
        let mut terminate = unix::signal(unix::SignalKind::interrupt())?;
        tokio::select! {
            Err(e) = running_modules => {
                error!("Error running modules: {:?}", e);
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down");
                abort();
            }
            _ = terminate.recv() =>  {
                info!("SIGTERM received, shutting down");
                abort();
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            Err(e) = running_modules => {
                error!("Error running modules: {:?}", e);
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down");
                abort();
            }
        }
    }

    Ok(())
}
