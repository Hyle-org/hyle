use assert_cmd::prelude::*;
use hyle::{
    model::{BlobTransaction, ProofData},
    rest::client::ApiHttpClient,
    tools::transactions_builder::{BuildResult, States, TransactionBuilder},
    utils::conf::{Conf, Consensus},
};
use rand::Rng;
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
            host: format!("localhost:{}", self.random_port + self.i),
            da_address: format!("localhost:{}", self.random_port + 1000 + self.i),
            rest: format!("localhost:{}", self.random_port + 2000 + self.i),
            ..self.default.clone()
        }
    }
}

impl Default for ConfMaker {
    fn default() -> Self {
        let mut default = Conf::new(None, None, None).unwrap();
        let mut rng = rand::thread_rng();
        let random_port: u32 = rng.gen_range(1024..(65536 - 3000));
        default.single_node = Some(false);
        default.host = format!("localhost:{}", random_port);
        default.da_address = format!("localhost:{}", random_port + 1000);
        default.rest = format!("localhost:{}", random_port + 2000);
        default.run_indexer = false; // disable indexer by default to avoid needed PG
        default.log_format = "node".to_string(); // Activate node name in logs for convenience in tests.
        info!("Default conf: {:?}", default);
        default.consensus = Consensus {
            slot_duration: 1,
            genesis_stakers: {
                let mut stakers = std::collections::HashMap::new();
                stakers.insert("node-1".to_owned(), 100);
                stakers.insert("node-2".to_owned(), 100);
                stakers
            },
        };
        info!("Default conf: {:?}", default);
        Self {
            i: 0,
            random_port,
            default,
        }
    }
}

enum TestProcessState {
    Command(Command),
    Child(Child),
}

pub struct TestProcess {
    pub conf: Conf,
    #[allow(dead_code)]
    pub dir: TempDir,
    state: TestProcessState,

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
        let conf_file = tmpdir.path().join("config.ron");
        ron::ser::to_writer(std::fs::File::create(&conf_file).unwrap(), &conf).unwrap();

        let console_port: u32 = conf.host.split(':').last().unwrap().parse().unwrap();
        let tokio_console_port: u16 = ((console_port + 10000) % u16::MAX as u32)
            .try_into()
            .unwrap();
        cmd.env(
            "TOKIO_CONSOLE_BIND",
            format!("127.0.0.1:{}", tokio_console_port),
        );
        cmd.env("RISC0_DEV_MODE", "1");
        Self {
            conf,
            dir: tmpdir,
            state: TestProcessState::Command(cargo_bin),
            stdout: None,
            stderr: None,
        }
    }

    #[allow(dead_code)]
    pub fn log(mut self, level: &str) -> Self {
        if let TestProcessState::Command(cmd) = &mut self.state {
            cmd.env("RUST_LOG", level);
        };
        self
    }

    pub fn start(mut self) -> Self {
        self.state = match &mut self.state {
            TestProcessState::Command(cmd) => {
                println!("Starting process: {:?}", cmd);
                TestProcessState::Child(cmd.spawn().unwrap())
            }
            TestProcessState::Child(child) => {
                panic!("Process already started: {:?}", child.id());
            }
        };
        if let TestProcessState::Child(child) = &mut self.state {
            let stdout = child.stdout.take().expect("Failed to capture stdout");
            let stderr = child.stderr.take().expect("Failed to capture stderr");

            self.stdout = Some(tokio::task::spawn(stream_output(stdout)));
            self.stderr = Some(tokio::task::spawn(stream_output(stderr)));
        }
        self
    }
}

pub async fn wait_height(client: &ApiHttpClient, heights: u64) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
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
    .await?

    //result.map_err(|_| anyhow::anyhow!("Timeout reached while waiting for height"))
}

pub async fn send_transaction(
    client: &ApiHttpClient,
    mut transaction: TransactionBuilder,
    states: &mut States,
) {
    let BuildResult {
        identity, blobs, ..
    } = transaction.build(states).await.unwrap();

    let blob_tx_hash = client
        .send_tx_blob(&BlobTransaction { identity, blobs })
        .await
        .unwrap();

    for (proof, contract_name) in transaction.iter_prove() {
        let proof: ProofData = proof.await.unwrap();
        client
            .send_tx_proof(&hyle::model::ProofTransaction {
                blob_tx_hash: blob_tx_hash.clone(),
                proof,
                contract_name,
            })
            .await
            .unwrap();
    }
}
