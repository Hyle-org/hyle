use anyhow::Context;
use assert_cmd::prelude::*;
use client_sdk::transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutor};

use hyle::{
    model::BlobTransaction,
    rest::client::NodeApiHttpClient,
    utils::{
        conf::{Conf, P2pConf},
        crypto::BlstCrypto,
    },
};
use hyle_model::TxHash;
use rand::Rng;
use signal_child::signal;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::{io::AsyncBufReadExt, time::timeout};
use tracing::info;

pub struct ConfMaker {
    i: u32,
    random_port: u32,
    pub default: Conf,
}

impl ConfMaker {
    pub fn build(&mut self, prefix: &str) -> Conf {
        self.i += 1;
        Conf {
            id: if prefix == "single-node" {
                prefix.into()
            } else {
                format!("{}-{}", prefix, self.i)
            },
            p2p: P2pConf {
                address: format!("localhost:{}", self.random_port + self.i),
                ..self.default.p2p.clone()
            },
            da_address: format!("localhost:{}", self.random_port + 1000 + self.i),
            tcp_address: Some(format!("localhost:{}", self.random_port + 2000 + self.i)),
            rest_address: format!("localhost:{}", self.random_port + 3000 + self.i),
            ..self.default.clone()
        }
    }
}

impl Default for ConfMaker {
    fn default() -> Self {
        let mut default = Conf::new(None, None, None).unwrap();
        let mut rng = rand::rng();
        let random_port: u32 = rng.random_range(1024..(65536 - 4000));

        default.log_format = "node".to_string(); // Activate node name in logs for convenience in tests.
        default.p2p.address = format!("localhost:{}", random_port);
        default.p2p.mode = hyle::utils::conf::P2pMode::FullValidator;
        default.consensus.solo = false;
        default.genesis.stakers = {
            let mut stakers = std::collections::HashMap::new();
            stakers.insert("node-1".to_owned(), 100);
            stakers.insert("node-2".to_owned(), 100);
            stakers
        };
        default.genesis.faucet_password = "password".into();

        default.da_address = format!("localhost:{}", random_port + 1000);
        default.tcp_address = Some(format!("localhost:{}", random_port + 2000));
        default.rest_address = format!("localhost:{}", random_port + 3000);

        default.run_indexer = false; // disable indexer by default to avoid needed PG

        info!("Default conf: {:?}", default);

        Self {
            i: 0,
            random_port,
            default,
        }
    }
}

pub struct TestProcess {
    pub conf: Conf,
    #[allow(dead_code)]
    pub dir: TempDir,

    cmd: Command,
    process: Option<Child>,

    stdout: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    stderr: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

async fn stream_output<R: tokio::io::AsyncRead + Unpin>(output: R) -> anyhow::Result<()> {
    let mut reader = tokio::io::BufReader::new(output).lines();
    while let Some(line) = reader.next_line().await? {
        println!("{}", line);
    }
    Ok(())
}
impl TestProcess {
    pub fn new(command: &str, mut conf: Conf) -> Self {
        info!("🚀 Starting process with conf: {:?}", conf);
        let mut cargo_bin: Command = std::process::Command::cargo_bin(command).unwrap().into();

        // Create a temporary directory for the node
        let tmpdir = tempfile::Builder::new().prefix("hyle").tempdir().unwrap();
        let cmd = cargo_bin.current_dir(&tmpdir);
        cmd.kill_on_drop(true);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        conf.data_directory = tmpdir.path().to_path_buf();
        // Serialize the configuration to a file
        let conf_file = tmpdir.path().join("config.toml");

        std::fs::write(&conf_file, toml::to_string(&conf).unwrap()).unwrap();

        cmd.env("RISC0_DEV_MODE", "1");

        let secret = BlstCrypto::secret_from_name(&conf.id);
        cmd.env("HYLE_VALIDATOR_SECRET", hex::encode(secret));

        Self {
            conf,
            dir: tmpdir,
            cmd: cargo_bin,
            process: None,
            stdout: None,
            stderr: None,
        }
    }

    #[allow(dead_code)]
    pub fn log(mut self, level: &str) -> Self {
        self.cmd.env("RUST_LOG", level);
        self
    }

    pub fn start(mut self) -> Self {
        if let Some(process) = &self.process {
            panic!("Process already started: {:?}", process.id());
        }
        info!("Starting process: {:?}", self.cmd);
        self.process = Some({
            let mut process = self.cmd.spawn().unwrap();
            let stdout = process.stdout.take().expect("Failed to capture stdout");
            let stderr = process.stderr.take().expect("Failed to capture stderr");

            self.stdout = Some(tokio::task::spawn(stream_output(stdout)));
            self.stderr = Some(tokio::task::spawn(stream_output(stderr)));

            info!("Started process ID: {:?}", process.id());

            process
        });

        self
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(mut process) = self.process.take() {
            // TODO: support windows?
            signal(process.id().unwrap().try_into().unwrap(), signal::SIGQUIT)
                .context("Failed to stop child")?;
            process.wait().await.context("Failed to wait for process")?;
            Ok(())
        } else {
            anyhow::bail!("Process not started")
        }
    }
}
pub async fn wait_height(client: &NodeApiHttpClient, heights: u64) -> anyhow::Result<()> {
    wait_height_timeout(client, heights, 30).await
}

pub async fn wait_height_timeout(
    client: &NodeApiHttpClient,
    heights: u64,
    timeout_duration: u64,
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(timeout_duration), async {
        loop {
            if let Ok(mut current_height) = client.get_block_height().await {
                let target_height = current_height + heights;
                while current_height.0 < target_height.0 {
                    info!(
                        "⏰ Waiting for height {} to be reached. Current is {}",
                        target_height, current_height
                    );
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    current_height = client.get_block_height().await?;
                }
                return anyhow::Ok(());
            } else {
                info!("⏰ Waiting for node to be ready");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("Timeout reached while waiting for height: {e}"))?
}

#[allow(dead_code)]
pub async fn send_transaction<S: StateUpdater>(
    client: &NodeApiHttpClient,
    transaction: ProvableBlobTx,
    ctx: &mut TxExecutor<S>,
) -> TxHash {
    let identity = transaction.identity.clone();
    let blobs = transaction.blobs.clone();
    let tx_hash = client
        .send_tx_blob(&BlobTransaction::new(identity, blobs))
        .await
        .unwrap();

    let provable_tx = ctx.process(transaction).unwrap();
    for proof in provable_tx.iter_prove() {
        let tx = proof.await.unwrap();
        client.send_tx_proof(&tx).await.unwrap();
    }
    tx_hash
}
