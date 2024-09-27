use std::str::FromStr;

use anyhow::{Context, Result};
use clap::{command, Parser, Subcommand};
use config::{Config, ConfigError, Environment, File};
use hyle::{
    model::{BlobTransaction, ProofTransaction, RegisterContractTransaction},
    rest::client::ApiHttpClient,
};
use reqwest::Url;
use serde::de::Deserialize;
use tracing::info;

fn load<'de, T: Deserialize<'de>>(file: String) -> Result<T, ConfigError> {
    let s = Config::builder()
        .add_source(File::with_name(file.as_str()))
        .add_source(Environment::with_prefix("hyle"))
        .build()?;

    s.try_deserialize::<T>()
}

/// A cli to interact with hyle node
#[derive(Debug, Parser)] // requires `derive` feature
#[command(name = "hyle")]
#[command(about = "A CLI to interact with hyle node", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: SendCommands,
}

#[derive(Debug, Subcommand)]
enum SendCommands {
    /// Send blob transaction
    #[command(alias = "b")]
    Blob { file: String },
    /// Send proof transaction
    #[command(alias = "p")]
    Proof { file: String },
    /// Register contract
    #[command(alias = "c")]
    Contract { file: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let api_client = ApiHttpClient {
        url: Url::from_str("http://localhost:4321").unwrap(),
        reqwest_client: reqwest::Client::new(),
    };

    match cli.command {
        SendCommands::Blob { file } => {
            let blob = load::<BlobTransaction>(file).context("loading blob tx from file")?;
            let res = api_client.send_tx_blob(&blob).await?;
            info!("Received response {}", res);
        }
        SendCommands::Proof { file } => {
            let blob = load::<ProofTransaction>(file).context("loading proof tx from file")?;
            let res = api_client.send_tx_proof(&blob).await?;
            info!("Received response {}", res);
        }
        SendCommands::Contract { file } => {
            let blob = load::<RegisterContractTransaction>(file)
                .context("loading contract tx from file")?;
            let res = api_client.send_tx_register_contract(&blob).await?;
            info!("Received response {}", res);
        }
    }

    info!("Done");

    Ok(())
}
