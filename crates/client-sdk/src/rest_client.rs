use std::{
    ops::{Deref, DerefMut},
    time::Duration,
};

use anyhow::{Context, Result};
use hyle_net::http::HttpClient;
use sdk::{
    api::{
        APIBlob, APIBlock, APIContract, APIRegisterContract, APIStaking, APITransaction, NodeInfo,
        TransactionWithBlobs,
    },
    BlobIndex, BlobTransaction, BlockHash, BlockHeight, ConsensusInfo, Contract, ContractName,
    ProofTransaction, TxHash, UnsettledBlobTransaction,
};

#[derive(Clone)]
pub struct IndexerApiHttpClient {
    pub client: HttpClient,
}

impl IndexerApiHttpClient {
    pub fn new(url: String) -> Result<Self> {
        Ok(IndexerApiHttpClient {
            client: HttpClient {
                url: url.parse()?,
                api_key: None,
                retry: None,
            },
        })
    }
    pub async fn list_contracts(&self) -> Result<Vec<APIContract>> {
        self.get("v1/indexer/contracts")
            .await
            .context("listing contracts")
    }

    pub async fn get_indexer_contract(&self, contract_name: &ContractName) -> Result<APIContract> {
        self.get(&format!("v1/indexer/contract/{contract_name}"))
            .await
            .context(format!("getting contract {contract_name}"))
    }

    pub async fn fetch_current_state<State>(&self, contract_name: &ContractName) -> Result<State>
    where
        State: serde::de::DeserializeOwned,
    {
        self.get::<State>(&format!("v1/indexer/contract/{contract_name}/state"))
            .await
            .context(format!("getting contract {contract_name} state"))
    }

    pub async fn get_block_height(&self) -> Result<BlockHeight> {
        let block: APIBlock = self.get_last_block().await?;
        Ok(BlockHeight(block.height))
    }

    pub async fn get_blocks(&self) -> Result<Vec<APIBlock>> {
        self.get("v1/indexer/blocks")
            .await
            .context("getting blocks")
    }

    pub async fn get_last_block(&self) -> Result<APIBlock> {
        self.get("v1/indexer/block/last")
            .await
            .context("getting last block")
    }

    pub async fn get_block_by_height(&self, height: &BlockHeight) -> Result<APIBlock> {
        self.get(&format!("v1/indexer/block/height/{height}"))
            .await
            .context(format!("getting block with height {height}"))
    }

    pub async fn get_block_by_hash(&self, hash: &BlockHash) -> Result<APIBlock> {
        self.get(&format!("v1/indexer/block/hash/{hash}"))
            .await
            .context(format!("getting block with hash {hash}"))
    }

    pub async fn get_transactions(&self) -> Result<Vec<APITransaction>> {
        self.get("v1/indexer/transactions")
            .await
            .context("getting transactions")
    }

    pub async fn get_transactions_by_height(
        &self,
        height: &BlockHeight,
    ) -> Result<Vec<APITransaction>> {
        self.get(&format!("v1/indexer/transactions/block/{height}"))
            .await
            .context(format!("getting transactions for block height {height}"))
    }

    pub async fn get_transactions_by_contract(
        &self,
        contract_name: &ContractName,
    ) -> Result<Vec<APITransaction>> {
        self.get(&format!("v1/indexer/transactions/contract/{contract_name}"))
            .await
            .context(format!("getting transactions for contract {contract_name}"))
    }

    pub async fn get_transaction_with_hash(&self, tx_hash: &TxHash) -> Result<APITransaction> {
        self.get(&format!("v1/indexer/transaction/hash/{tx_hash}"))
            .await
            .context(format!("getting transaction with hash {tx_hash}"))
    }

    pub async fn get_blob_transactions_by_contract(
        &self,
        contract_name: &ContractName,
    ) -> Result<Vec<TransactionWithBlobs>> {
        self.get(&format!(
            "v1/indexer/blob_transactions/contract/{contract_name}"
        ))
        .await
        .context(format!(
            "getting blob transactions for contract {contract_name}"
        ))
    }

    pub async fn get_blobs_by_tx_hash(&self, tx_hash: &TxHash) -> Result<Vec<APIBlob>> {
        self.get(&format!("v1/indexer/blobs/hash/{tx_hash}"))
            .await
            .context(format!("getting blob by transaction hash {tx_hash}"))
    }

    pub async fn get_blob(&self, tx_hash: &TxHash, blob_index: BlobIndex) -> Result<APIBlob> {
        self.get(&format!(
            "v1/indexer/blob/hash/{tx_hash}/index/{blob_index}"
        ))
        .await
        .context(format!(
            "getting blob with hash {tx_hash} and index {blob_index}"
        ))
    }
}

impl Deref for IndexerApiHttpClient {
    type Target = HttpClient;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

#[derive(Clone)]
pub struct NodeApiHttpClient {
    pub client: HttpClient,
}

impl NodeApiHttpClient {
    pub fn new(url: String) -> Result<Self> {
        Ok(NodeApiHttpClient {
            client: HttpClient {
                url: url.parse()?,
                api_key: None,
                retry: None,
            },
        })
    }
    /// Create a new client with a retry configuration (retrying `n` times, waiting `duration` before each retry)
    pub fn with_retry(&self, n: usize, duration: Duration) -> Self {
        let mut cloned = self.clone();
        cloned.retry = Some((n, duration));
        cloned
    }

    /// Create a client with the retry configuration (n=3, duration=1000ms)
    pub fn retry_3times_1000ms(&self) -> Self {
        self.with_retry(3, Duration::from_millis(1000))
    }

    pub async fn register_contract(&self, tx: &APIRegisterContract) -> Result<TxHash> {
        self.post_json("v1/contract/register", tx)
            .await
            .context("Registering contract")
    }

    pub async fn send_tx_blob(&self, tx: &BlobTransaction) -> Result<TxHash> {
        self.post_json("v1/tx/send/blob", tx)
            .await
            .context("Sending tx blob")
    }

    pub async fn send_tx_proof(&self, tx: &ProofTransaction) -> Result<TxHash> {
        self.post_json("v1/tx/send/proof", tx)
            .await
            .context("Sending tx proof")
    }

    pub async fn get_consensus_info(&self) -> Result<ConsensusInfo> {
        self.get("v1/consensus/info")
            .await
            .context("getting consensus info")
    }

    pub async fn get_consensus_staking_state(&self) -> Result<APIStaking> {
        self.get("v1/consensus/staking_state")
            .await
            .context("getting consensus staking state")
    }

    pub async fn get_node_info(&self) -> Result<NodeInfo> {
        self.get("v1/info").await.context("getting node info")
    }

    pub async fn metrics(&self) -> Result<String> {
        self.get_str("v1/metrics")
            .await
            .context("getting node metrics")
    }

    pub async fn get_block_height(&self) -> Result<BlockHeight> {
        self.get("v1/da/block/height")
            .await
            .context("getting block height")
    }

    pub async fn get_contract(&self, contract_name: &ContractName) -> Result<Contract> {
        self.get(&format!("v1/contract/{}", contract_name))
            .await
            .context(format!("getting contract {}", contract_name))
    }

    pub async fn get_unsettled_tx(
        &self,
        blob_tx_hash: &TxHash,
    ) -> Result<UnsettledBlobTransaction> {
        self.get(&format!("v1/unsettled_tx/{blob_tx_hash}"))
            .await
            .context(format!("getting tx {}", blob_tx_hash))
    }
}
impl Deref for NodeApiHttpClient {
    type Target = HttpClient;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}
impl DerefMut for NodeApiHttpClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.client
    }
}
