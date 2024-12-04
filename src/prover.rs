use crate::{
    bus::BusMessage,
    model::{
        Blob, BlobTransaction, Block, BlockHeight, Hashable, ProofData, ProofTransaction,
        SharedRunContext, Transaction, TransactionData,
    },
    rest::client::ApiHttpClient,
    utils::modules::Module,
};
use anyhow::{bail, Error, Result};
use borsh::to_vec;
use futures::{SinkExt, StreamExt};
use hyle_contract_sdk::{BlobIndex, ContractInput, ContractName, Digestable};
use reqwest::{Client, Url};
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
    client: ApiHttpClient,
}

impl Module for Prover {
    type Context = SharedRunContext;
    fn name() -> &'static str {
        "Prover"
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        info!("Fetching current block height");
        let url = format!("http://{}", ctx.common.config.rest);
        let client = ApiHttpClient {
            url: Url::parse(url.as_str()).unwrap(),
            reqwest_client: Client::new(),
        };
        let height = client.get_block_height().await?;
        let da_stream = connect_to(&ctx.common.config.da_address, height).await?;
        Ok(Prover { da_stream, client })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}
pub const HYLLAR_BIN: &[u8] = include_bytes!("../contracts/hyllar/hyllar.img");
pub const AMM_BIN: &[u8] = include_bytes!("../contracts/amm/amm.img");

pub static HYLLAR_ID: &str = include_str!("../contracts/hyllar/hyllar.txt");
pub static AMM_ID: &str = include_str!("../contracts/hyllar/hyllar.txt");

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
                "⚒️  Found blob to proove for '{}' in transaction: {}",
                contract_name,
                tx.hash()
            );

            let tx_hash = tx.hash();

            let contract = self.client.get_contract(contract_name).await?;

            let program_id = hex::encode(contract.program_id.as_slice());
            let proof = if program_id == HYLLAR_ID.trim() {
                let contract_inputs = self
                    .build_inputs::<hyllar::HyllarToken>(
                        contract_name,
                        &tx,
                        BlobIndex(blob_index as u32),
                    )
                    .await?;
                Self::prove(contract_inputs, HYLLAR_BIN)?
            } else if program_id == AMM_ID.trim() {
                let contract_inputs = self
                    .build_inputs::<amm::AmmState>(contract_name, &tx, BlobIndex(blob_index as u32))
                    .await?;
                Self::prove(contract_inputs, AMM_BIN)?
            } else {
                continue;
            };

            let proof_tx = ProofTransaction {
                contract_name: contract_name.clone(),
                blob_tx_hash: tx_hash.clone(),
                proof: ProofData::Bytes(proof),
            };

            info!("🚀 Sending proof tx to mempool: {}", proof_tx.hash());
            self.send_proof(proof_tx).await?;
        }

        Ok(())
    }

    async fn build_inputs<State>(
        &self,
        contract_name: &ContractName,
        tx: &BlobTransaction,
        index: BlobIndex,
    ) -> Result<ContractInput<State>>
    where
        State: Digestable + TryFrom<hyle_contract_sdk::StateDigest, Error = Error>,
    {
        let blobs = tx.blobs.clone();
        let tx_hash = tx.hash();

        let initial_state = self.fetch_current_state::<State>(contract_name).await?;
        let contract_inputs = hyle_contract_sdk::ContractInput {
            initial_state,
            tx_hash,
            blobs,
            index,
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

    pub async fn fetch_current_state<State>(
        &self,
        contract_name: &ContractName,
    ) -> Result<State, Error>
    where
        State: TryFrom<hyle_contract_sdk::StateDigest, Error = Error>,
    {
        let contract = self.client.get_contract(contract_name).await?;
        contract.state.try_into()
    }

    async fn send_proof(&self, proof_tx: ProofTransaction) -> Result<()> {
        let res = self.client.send_tx_proof(&proof_tx).await?;

        info!("Proof sent successfully");
        info!("Response: {}", res.text().await?);

        Ok(())
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
