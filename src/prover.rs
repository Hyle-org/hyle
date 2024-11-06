use crate::{
    bus::BusMessage,
    model::{
        Blob, BlobReference, BlobTransaction, Block, BlockHeight, Hashable, ProofData,
        ProofTransaction, SharedRunContext, Transaction, TransactionData,
    },
    node_state::model::Contract,
    utils::modules::Module,
};
use anyhow::{bail, Error, Result};
use borsh::to_vec;
use futures::{SinkExt, StreamExt};
use hyle_contract_sdk::{BlobIndex, ContractInput, Digestable};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProverEvent {
    NewTx(Transaction),
}
impl BusMessage for ProverEvent {}

pub struct Prover {
    da_stream: Framed<TcpStream, LengthDelimitedCodec>,
    rest: String,
}

impl Module for Prover {
    type Context = SharedRunContext;
    fn name() -> &'static str {
        "Prover"
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        info!("Fetching current block height");
        let url = format!("http://{}", ctx.common.config.rest);
        let resp = reqwest::get(format!("{}/v1/da/block/height", url))
            .await
            .expect("Failed to get block height");
        let body = resp.text().await.expect("Failed to read response body");

        let height = serde_json::from_str::<BlockHeight>(&body).unwrap();
        let da_stream = connect_to(&ctx.common.config.da_address, height).await?;
        Ok(Prover {
            da_stream,
            rest: ctx.common.config.rest.clone(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}
pub const HYLLAR_BIN: &[u8] = include_bytes!("../contracts/hyllar/hyllar.img");
pub const HYDENTITY_BIN: &[u8] = include_bytes!("../contracts/hydentity/hydentity.img");

impl Prover {
    pub async fn start(&mut self) -> Result<(), Error> {
        loop {
            let frame = self.da_stream.next().await;
            if let Some(Ok(cmd)) = frame {
                let bytes = cmd;
                let block: Block =
                    bincode::decode_from_slice(&bytes, bincode::config::standard())?.0;
                if let Err(e) = self.handle_block(block).await {
                    error!("Error while handling block: {:#}", e);
                }
                SinkExt::<bytes::Bytes>::send(&mut self.da_stream, "ok".into()).await?;
            } else if frame.is_none() {
                bail!("DA stream closed");
            } else if let Some(Err(e)) = frame {
                bail!("Error while reading DA stream: {}", e);
            }
        }
    }

    async fn handle_block(&mut self, block: Block) -> Result<()> {
        for tx in block.txs {
            if let Err(e) = self.handle_tx(tx).await {
                error!("Error while handling tx: {:#}", e);
            }
        }
        Ok(())
    }

    async fn handle_tx(&mut self, tx: Transaction) -> Result<()> {
        if let TransactionData::Blob(tx) = tx.transaction_data {
            self.prove_blobs(tx).await?;
        }
        Ok(())
    }

    async fn prove_blobs(&mut self, tx: BlobTransaction) -> Result<()> {
        info!("Got a new transaction to prove: {}", tx.hash());

        for (blob_index, Blob { contract_name, .. }) in tx.blobs.iter().enumerate() {
            info!(
                "⚒️  Proving blob {} in transaction: {}",
                contract_name,
                tx.hash()
            );

            let tx_hash = tx.hash();

            let proof = match contract_name.0.as_str() {
                "hyllar" => {
                    let contract_inputs = self
                        .build_inputs::<hyllar::HyllarToken>("hyllar", &tx)
                        .await?;
                    Self::prove(contract_inputs, HYLLAR_BIN)?
                }
                "hydentity" => {
                    let contract_inputs = self
                        .build_inputs::<hydentity::Hydentity>("hydentity", &tx)
                        .await?;
                    Self::prove(contract_inputs, HYDENTITY_BIN)?
                }
                _ => continue,
            };

            let proof_tx = ProofTransaction {
                blobs_references: vec![BlobReference {
                    contract_name: contract_name.clone(),
                    blob_tx_hash: tx_hash.clone(),
                    blob_index: BlobIndex(blob_index as u32),
                }],
                proof: ProofData::Bytes(proof),
            };

            info!("🚀 Sending proof tx to mempool: {}", proof_tx.hash());
            self.send_proof(proof_tx).await?;
        }

        Ok(())
    }

    async fn build_inputs<State>(
        &self,
        contract_name: &str,
        tx: &BlobTransaction,
    ) -> Result<ContractInput<State>>
    where
        State: Digestable + TryFrom<hyle_contract_sdk::StateDigest, Error = Error>,
    {
        let blobs = tx
            .blobs
            .clone()
            .into_iter()
            .map(|b| b.data)
            .collect::<Vec<_>>();

        let tx_hash = tx.hash();

        let initial_state = self.fetch_current_state::<State>(contract_name).await?;
        let contract_inputs = hyle_contract_sdk::ContractInput {
            initial_state,
            tx_hash: tx_hash.0.clone(),
            blobs,
            index: 0,
            private_blob: hyle_contract_sdk::BlobData("password".as_bytes().to_vec()),
            identity: tx.identity.clone(),
        };
        Ok(contract_inputs)
    }

    fn prove<ContractInput>(contract_input: ContractInput, binary: &[u8]) -> Result<Vec<u8>>
    where
        ContractInput: serde::Serialize,
    {
        let env = risc0_zkvm::ExecutorEnv::builder()
            .write(&contract_input)?
            .build()?;

        let prover = risc0_zkvm::default_prover();
        let prove_info = prover.prove(env, binary)?;

        let receipt = prove_info.receipt;
        let encoded_receipt = to_vec(&receipt).expect("Unable to encode receipt");
        Ok(encoded_receipt)
    }

    pub async fn fetch_current_state<State>(&self, contract_name: &str) -> Result<State, Error>
    where
        State: TryFrom<hyle_contract_sdk::StateDigest, Error = Error>,
    {
        let url = format!("http://{}", self.rest);
        let resp = reqwest::get(format!("{}/v1/contract/{}", url, contract_name)).await?;

        let status = resp.status();
        let body = resp.text().await?;

        if let Ok(contract) = serde_json::from_str::<Contract>(&body) {
            info!("Fetched contract: {:?}", contract);
            Ok(contract.state.try_into()?)
        } else {
            bail!(
                "Failed to parse JSON response, status: {}, body: {}",
                status,
                body
            );
        }
    }

    async fn send_proof(&self, proof_tx: ProofTransaction) -> Result<()> {
        let url = format!("http://{}", self.rest);
        let client = reqwest::Client::new();
        let res = client
            .post(format!("{}/v1/tx/send/proof", url))
            .json(&proof_tx)
            .send()
            .await?;
        if res.status().is_success() {
            info!("Proof sent successfully");
            info!("Response: {}", res.text().await?);
            Ok(())
        } else {
            bail!("Failed to send proof: {:?}", res);
        }
    }
}

pub async fn connect_to(
    target: &str,
    height: BlockHeight,
) -> Result<Framed<TcpStream, LengthDelimitedCodec>> {
    info!(
        "Connecting to node for data availability stream on {}",
        &target
    );
    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();

    let stream = loop {
        debug!("Trying to connect to {}", target);
        match TcpStream::connect(&target).await {
            Ok(stream) => break stream,
            Err(e) => {
                if start.elapsed() >= timeout {
                    bail!("Failed to connect to {}: {}. Timeout reached.", target, e);
                }
                warn!(
                    "Failed to connect to {}: {}. Retrying in 1 second...",
                    target, e
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    };
    let addr = stream.local_addr()?;
    let mut da_stream = Framed::new(stream, LengthDelimitedCodec::new());
    info!(
        "Connected to data stream to {} on {}. Starting stream from height {}",
        &target, addr, height
    );
    // Send the start height
    let height = bincode::encode_to_vec(height.0, bincode::config::standard())?;
    da_stream.send(height.into()).await?;
    Ok(da_stream)
}
