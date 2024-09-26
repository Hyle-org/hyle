use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result};
use clap::{command, Parser, Subcommand};
use config::{Config, ConfigError, Environment, File};
use hyle::model::{BlobTransaction, ProofTransaction, RegisterContractTransaction};
use rand::Rng;
use reqwest::Url;
use serde::de::Deserialize;
use tokio::time::sleep;
use tracing::{error, info};

fn load<'de, T: Deserialize<'de>>(file: String) -> Result<T, ConfigError> {
    let s = Config::builder()
        .add_source(File::with_name(file.as_str()))
        .add_source(Environment::with_prefix("hyle"))
        .build()?;

    s.try_deserialize::<T>()
}

struct ApiHttpClient {
    url: Url,
    reqwest_client: reqwest::Client,
}

impl ApiHttpClient {
    pub async fn send_tx_blob(&self, tx: &BlobTransaction) -> Result<String> {
        let res = self
            .reqwest_client
            .post(format!("{}v1/tx/send/blob", self.url))
            .body(serde_json::to_string(tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx blob")?;

        res.text().await.context("test")
    }
    pub async fn send_tx_proof(&self, tx: &ProofTransaction) -> Result<String> {
        let res = self
            .reqwest_client
            .post(format!("{}v1/tx/send/proof", self.url))
            .body(serde_json::to_string(&tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx proof")?;

        res.text().await.context("test")
    }
    pub async fn send_tx_register_contract(
        &self,
        tx: &RegisterContractTransaction,
    ) -> Result<String> {
        let res = self
            .reqwest_client
            .post(format!("{}v1/contract/register", self.url))
            .body(serde_json::to_string(&tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx register contract")?;

        res.text().await.context("test")
    }
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
    Blob {
        file: String,
    },
    /// Send proof transaction
    Proof {
        file: String,
    },
    /// Register contract
    Contract {
        file: String,
    },
    Auto,
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
        SendCommands::Auto => {
            info!("Starting auto mode");

            let tx_blob = load::<BlobTransaction>("./data/tx1_blob.ron".to_string())
                .context("loading blob tx from file")?;
            let tx_proof = load::<ProofTransaction>("./data/tx1_proof.ron".to_string())
                .context("loading proof tx from file")?;
            let tx_contract =
                load::<RegisterContractTransaction>("./data/contract_c1.ron".to_string())
                    .context("loading contract tx from file")?;

            let mut rand = rand::thread_rng();

            loop {
                match rand.gen_range(1..4) {
                    1 => {
                        info!("Sending tx blob");
                        _ = api_client.send_tx_blob(&tx_blob).await;
                    }
                    2 => {
                        info!("Sending tx proof");
                        _ = api_client.send_tx_proof(&tx_proof).await;
                    }
                    3 => {
                        info!("Sending contract");
                        _ = api_client.send_tx_register_contract(&tx_contract).await;
                    }
                    _ => {
                        error!("unknown random choice");
                    }
                }

                sleep(Duration::from_millis(500)).await;
            }
        }
    }

    info!("Done");

    Ok(())
}
