use anyhow::{anyhow, bail, Error, Result};
use futures::{SinkExt, StreamExt};
use hyle_contract_sdk::{BlobIndex, ContractName, TxHash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, info, warn};

use crate::{
    bus::BusMessage,
    model::{
        Blob, BlobTransaction, Block, BlockHeight, Hashable, RegisterContractTransaction,
        Transaction, TransactionData,
    },
    node_state::NodeState,
    utils::modules::Module,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProverEvent {
    NewTx(Transaction),
}
impl BusMessage for ProverEvent {}

pub struct ContractStateIndexer<State> {
    da_stream: Framed<TcpStream, LengthDelimitedCodec>,
    states: BTreeMap<ContractName, State>,
    unsettled_blobs: BTreeMap<TxHash, BlobTransaction>,
    node_state: NodeState,
    program_id: String,
    handler: Box<dyn Fn(&BlobTransaction, BlobIndex, State) -> Result<State> + Send + Sync>,
}

pub struct ContractStateIndexerCtx<State> {
    pub da_address: String,
    pub program_id: String,
    pub handler: Box<dyn Fn(&BlobTransaction, BlobIndex, State) -> Result<State> + Send + Sync>,
}

impl<State> Module for ContractStateIndexer<State>
where
    State: TryFrom<hyle_contract_sdk::StateDigest, Error = Error> + Clone + Sync + Send,
{
    type Context = ContractStateIndexerCtx<State>;
    fn name() -> &'static str {
        "HyllarIndexer"
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        info!("Fetching current block height");
        let da_stream = connect_to(&ctx.da_address, BlockHeight(0)).await?;

        Ok(ContractStateIndexer {
            da_stream,
            states: BTreeMap::new(),
            unsettled_blobs: BTreeMap::new(),
            node_state: NodeState::default(),
            program_id: ctx.program_id,
            handler: ctx.handler,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl<State> ContractStateIndexer<State>
where
    State: TryFrom<hyle_contract_sdk::StateDigest, Error = Error> + Clone,
{
    pub async fn start(&mut self) -> Result<(), Error> {
        loop {
            let frame = self.da_stream.next().await;
            if let Some(Ok(cmd)) = frame {
                let bytes = cmd;
                let block: Block =
                    bincode::decode_from_slice(&bytes, bincode::config::standard())?.0;
                if let Err(e) = self.handle_block(block) {
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

    fn handle_block(&mut self, block: Block) -> Result<()> {
        info!(
            "📦 Handling block #{} with {} txs",
            block.height,
            block.txs.len()
        );
        let handled = self.node_state.handle_new_block(block);
        debug!("📦 Handled {:?}", handled);

        for c_tx in handled.new_contract_txs {
            if let TransactionData::RegisterContract(tx) = c_tx.transaction_data {
                self.handle_register_contract(tx)?;
            }
        }

        for b_tx in handled.new_blob_txs {
            if let TransactionData::Blob(tx) = b_tx.transaction_data {
                self.handle_blob(tx)?;
            }
        }

        for s_tx in handled.settled_blob_tx_hashes {
            self.settle_tx(s_tx)?;
        }
        Ok(())
    }

    fn handle_register_contract(&mut self, tx: RegisterContractTransaction) -> Result<()> {
        let program_id = hex::encode(tx.program_id.as_slice());
        if program_id != self.program_id {
            return Ok(());
        }
        info!("📝 Registering supported contract '{}'", tx.contract_name);
        let state = tx.state_digest.try_into()?;
        self.states.insert(tx.contract_name.clone(), state);
        Ok(())
    }

    fn handle_blob(&mut self, tx: BlobTransaction) -> Result<()> {
        let tx_hash = tx.hash();
        if tx
            .blobs
            .iter()
            .any(|b| self.states.contains_key(&b.contract_name))
        {
            info!("⚒️  Found supported blob in transaction: {}", tx_hash);
            self.unsettled_blobs.insert(tx_hash.clone(), tx);
        }
        Ok(())
    }

    fn settle_tx(&mut self, tx: TxHash) -> Result<()> {
        let Some(tx) = self.unsettled_blobs.get(&tx) else {
            return Ok(());
        };

        debug!("🔨 Settling transaction: {}", tx.hash());

        for (index, Blob { contract_name, .. }) in tx.blobs.iter().enumerate() {
            if !self.states.contains_key(contract_name) {
                continue;
            }

            let state = self
                .states
                .get(contract_name)
                .cloned()
                .ok_or(anyhow!("No state found for {contract_name}"))?;

            let new_state = (self.handler)(tx, BlobIndex(index as u32), state)?;

            info!("📈 Updated state for {contract_name}");

            *self.states.get_mut(contract_name).unwrap() = new_state;
        }
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
