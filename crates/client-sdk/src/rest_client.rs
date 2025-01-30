use anyhow::{Context, Result};
use reqwest::Url;

use sdk::{
    api::*, BlobIndex, BlobTransaction, BlockHash, BlockHeight, ConsensusInfo, Contract,
    ContractName, ProofTransaction, StateDigest, TxHash, UnsettledBlobTransaction,
};

pub struct NodeApiHttpClient {
    pub url: Url,
    pub reqwest_client: reqwest::Client,
}

pub struct IndexerApiHttpClient {
    pub url: Url,
    pub reqwest_client: reqwest::Client,
}

impl NodeApiHttpClient {
    pub fn new(url: String) -> Result<Self> {
        Ok(Self {
            url: Url::parse(&url)?,
            reqwest_client: reqwest::Client::new(),
        })
    }

    pub async fn register_contract(&self, tx: &APIRegisterContract) -> Result<TxHash> {
        self.post("v1/contract/register", tx, "Registering contract")
            .await
    }

    pub async fn send_tx_blob(&self, tx: &BlobTransaction) -> Result<TxHash> {
        self.post("v1/tx/send/blob", tx, "Sending tx blob").await
    }

    pub async fn send_tx_proof(&self, tx: &ProofTransaction) -> Result<TxHash> {
        self.post("v1/tx/send/proof", tx, "Sending tx proof").await
    }

    pub async fn get_consensus_info(&self) -> Result<ConsensusInfo> {
        self.get("v1/consensus/info", "getting consensus info")
            .await
    }

    pub async fn get_consensus_staking_state(&self) -> Result<APIStaking> {
        self.get(
            "v1/consensus/staking_state",
            "getting consensus staking state",
        )
        .await
    }

    pub async fn get_node_info(&self) -> Result<NodeInfo> {
        self.get("v1/info", "getting node info").await
    }

    pub async fn metrics(&self) -> Result<String> {
        self.reqwest_client
            .get(format!("{}v1/metrics", self.url))
            .header("Content-Type", "application/text")
            .send()
            .await
            .context("getting node metrics")?
            .text()
            .await
            .context("reading node metrics response")
    }

    pub async fn get_block_height(&self) -> Result<BlockHeight> {
        self.get("v1/da/block/height", "getting block height").await
    }

    pub async fn get_contract(&self, contract_name: &ContractName) -> Result<Contract> {
        self.get(
            &format!("v1/contract/{}", contract_name),
            &format!("getting contract {}", contract_name),
        )
        .await
    }

    pub async fn get_unsettled_tx(
        &self,
        blob_tx_hash: &TxHash,
    ) -> Result<UnsettledBlobTransaction> {
        self.get(
            &format!("v1/unsettled_tx/{blob_tx_hash}"),
            &format!("getting tx {}", blob_tx_hash),
        )
        .await
    }

    async fn get<T>(&self, endpoint: &str, context_msg: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        self.reqwest_client
            .get(format!("{}{}", self.url, endpoint))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context(format!("{} request failed", context_msg))?
            .json::<T>()
            .await
            .context(format!("Failed to deserialize {}", context_msg))
    }

    async fn post<T, R>(&self, endpoint: &str, body: &T, context_msg: &str) -> Result<R>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.reqwest_client
            .post(format!("{}{}", self.url, endpoint))
            .body(serde_json::to_string(body)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context(format!("{} request failed", context_msg))?
            .json::<R>()
            .await
            .context(format!("Failed to deserialize {}", context_msg))
    }
}

impl IndexerApiHttpClient {
    pub fn new(url: String) -> Result<Self> {
        Ok(Self {
            url: Url::parse(&url)?,
            reqwest_client: reqwest::Client::new(),
        })
    }

    pub async fn list_contracts(&self) -> Result<Vec<APIContract>> {
        self.get("v1/indexer/contracts", "listing contracts").await
    }

    pub async fn get_indexer_contract(&self, contract_name: &ContractName) -> Result<APIContract> {
        self.get(
            &format!("v1/indexer/contract/{contract_name}"),
            &format!("getting contract {contract_name}"),
        )
        .await
    }

    pub async fn fetch_current_state<State>(&self, contract_name: &ContractName) -> Result<State>
    where
        State: TryFrom<StateDigest, Error = anyhow::Error>,
    {
        let resp = self.get_indexer_contract(contract_name).await?;

        StateDigest(resp.state_digest).try_into()
    }

    pub async fn get_blocks(&self) -> Result<Vec<APIBlock>> {
        self.get("v1/indexer/blocks", "getting blocks").await
    }

    pub async fn get_last_block(&self) -> Result<APIBlock> {
        self.get("v1/indexer/block/last", "getting last block")
            .await
    }

    pub async fn get_block_by_height(&self, height: &BlockHeight) -> Result<APIBlock> {
        self.get(
            &format!("v1/indexer/block/height/{height}"),
            &format!("getting block with height {height}"),
        )
        .await
    }

    pub async fn get_block_by_hash(&self, hash: &BlockHash) -> Result<APIBlock> {
        self.get(
            &format!("v1/indexer/block/hash/{hash}"),
            &format!("getting block with hash {hash}"),
        )
        .await
    }

    pub async fn get_transactions(&self) -> Result<Vec<APITransaction>> {
        self.get("v1/indexer/transactions", "getting transactions")
            .await
    }

    pub async fn get_transactions_by_height(
        &self,
        height: &BlockHeight,
    ) -> Result<Vec<APITransaction>> {
        self.get(
            &format!("v1/indexer/transactions/block/{height}"),
            &format!("getting transactions for block height {height}"),
        )
        .await
    }

    pub async fn get_transactions_by_contract(
        &self,
        contract_name: &ContractName,
    ) -> Result<Vec<APITransaction>> {
        self.get(
            &format!("v1/indexer/transactions/contract/{contract_name}"),
            &format!("getting transactions for contract {contract_name}"),
        )
        .await
    }

    pub async fn get_transaction_with_hash(&self, tx_hash: &TxHash) -> Result<APITransaction> {
        self.get(
            &format!("v1/indexer/transaction/hash/{tx_hash}"),
            &format!("getting transaction with hash {tx_hash}"),
        )
        .await
    }

    pub async fn get_blob_transactions_by_contract(
        &self,
        contract_name: &ContractName,
    ) -> Result<Vec<TransactionWithBlobs>> {
        self.get(
            &format!("v1/indexer/blob_transactions/contract/{contract_name}"),
            &format!("getting blob transactions for contract {contract_name}"),
        )
        .await
    }

    pub async fn get_blob_by_tx_hash(&self, tx_hash: &TxHash) -> Result<APIBlob> {
        self.get(
            &format!("v1/indexer/blobs/hash/{tx_hash}"),
            &format!("getting blob by transaction hash {tx_hash}"),
        )
        .await
    }

    pub async fn get_blob(&self, tx_hash: &TxHash, blob_index: BlobIndex) -> Result<APIBlob> {
        self.get(
            &format!("v1/indexer/blob/hash/{tx_hash}/index/{blob_index}"),
            &format!("getting blob with hash {tx_hash} and index {blob_index}"),
        )
        .await
    }

    async fn get<T>(&self, endpoint: &str, context_msg: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        self.reqwest_client
            .get(format!("{}{}", self.url, endpoint))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context(format!("{} request failed", context_msg))?
            .json::<T>()
            .await
            .context(format!("Failed to deserialize {}", context_msg))
    }
}
