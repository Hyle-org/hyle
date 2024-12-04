use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Error, Result};
use futures::{SinkExt, StreamExt};
use hyle::{
    bus::BusMessage,
    model::{
        Blob, BlobTransaction, Block, BlockHeight, Hashable, RegisterContractTransaction,
        SharedRunContext, Transaction, TransactionData,
    },
    node_state::NodeState,
    utils::modules::Module,
};
use hyllar::{HyllarToken, HyllarTokenContract};
use sdk::{erc20::ERC20Action, ContractName, StructuredBlobData, TxHash};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProverEvent {
    NewTx(Transaction),
}
impl BusMessage for ProverEvent {}

pub struct Indexer {
    da_stream: Framed<TcpStream, LengthDelimitedCodec>,
    states: BTreeMap<ContractName, HyllarToken>,
    unsettled_blobs: BTreeMap<TxHash, BlobTransaction>,
    node_state: NodeState,
}

impl Module for Indexer {
    type Context = SharedRunContext;
    fn name() -> &'static str {
        "HyllarIndexer"
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        info!("Fetching current block height");
        let da_stream = connect_to(&ctx.common.config.da_address, BlockHeight(0)).await?;

        Ok(Indexer {
            da_stream,
            states: BTreeMap::new(),
            unsettled_blobs: BTreeMap::new(),
            node_state: NodeState::default(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}
pub static HYLLAR_ID: &str = include_str!("../../hyllar.txt");

impl Indexer {
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
        if program_id != HYLLAR_ID.trim() {
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

        for Blob {
            contract_name,
            data,
        } in tx.blobs.iter()
        {
            if !self.states.contains_key(contract_name) {
                continue;
            }

            let state = self
                .states
                .get(contract_name)
                .cloned()
                .ok_or(anyhow!("No state found for {contract_name}"))?;

            let data: StructuredBlobData<ERC20Action> = data.clone().try_into()?;

            let caller: sdk::Identity = data
                .caller
                .map(|i| {
                    tx.blobs
                        .get(i.0 as usize)
                        .unwrap()
                        .contract_name
                        .0
                        .clone()
                        .into()
                })
                .unwrap_or(tx.identity.clone());

            let mut contract = HyllarTokenContract::init(state, caller);
            let res = sdk::erc20::execute_action(&mut contract, data.parameters);
            info!("🚀 Executed {contract_name}: {res:?}");

            let new_state = contract.state();

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
