use std::ops::Deref;

use anyhow::Result;
use hyper::Uri;
use sdk::{
    api::{APIBlob, APIBlock, APIContract, APITransaction, TransactionWithBlobs},
    BlobIndex, BlockHash, BlockHeight, ContractName, TxHash,
};

use crate::http::HttpClient;

struct IndexerApiHttpClient {
    pub client: HttpClient,
}

impl IndexerApiHttpClient {
    pub fn new(url: String) -> Result<Self> {
        Ok(IndexerApiHttpClient {
            client: HttpClient {
                url: url.parse()?,
                api_key: None,
            },
        })
    }
    async fn list_contracts(&self) -> Result<Vec<APIContract>> {
        self.get("v1/indexer/contracts", "listing contracts").await
    }

    async fn get_indexer_contract(&self, contract_name: &ContractName) -> Result<APIContract> {
        self.get(
            &format!("v1/indexer/contract/{contract_name}"),
            &format!("getting contract {contract_name}"),
        )
        .await
    }

    async fn fetch_current_state<State>(&self, contract_name: &ContractName) -> Result<State>
    where
        State: serde::de::DeserializeOwned,
    {
        self.get::<State>(
            &format!("v1/indexer/contract/{contract_name}/state"),
            &format!("getting contract {contract_name} state"),
        )
        .await
    }

    async fn get_blocks(&self) -> Result<Vec<APIBlock>> {
        self.get("v1/indexer/blocks", "getting blocks").await
    }

    async fn get_last_block(&self) -> Result<APIBlock> {
        self.get("v1/indexer/block/last", "getting last block")
            .await
    }

    async fn get_block_by_height(&self, height: &BlockHeight) -> Result<APIBlock> {
        self.get(
            &format!("v1/indexer/block/height/{height}"),
            &format!("getting block with height {height}"),
        )
        .await
    }

    async fn get_block_by_hash(&self, hash: &BlockHash) -> Result<APIBlock> {
        self.get(
            &format!("v1/indexer/block/hash/{hash}"),
            &format!("getting block with hash {hash}"),
        )
        .await
    }

    async fn get_transactions(&self) -> Result<Vec<APITransaction>> {
        self.get("v1/indexer/transactions", "getting transactions")
            .await
    }

    async fn get_transactions_by_height(
        &self,
        height: &BlockHeight,
    ) -> Result<Vec<APITransaction>> {
        self.get(
            &format!("v1/indexer/transactions/block/{height}"),
            &format!("getting transactions for block height {height}"),
        )
        .await
    }

    async fn get_transactions_by_contract(
        &self,
        contract_name: &ContractName,
    ) -> Result<Vec<APITransaction>> {
        self.get(
            &format!("v1/indexer/transactions/contract/{contract_name}"),
            &format!("getting transactions for contract {contract_name}"),
        )
        .await
    }

    async fn get_transaction_with_hash(&self, tx_hash: &TxHash) -> Result<APITransaction> {
        self.get(
            &format!("v1/indexer/transaction/hash/{tx_hash}"),
            &format!("getting transaction with hash {tx_hash}"),
        )
        .await
    }

    async fn get_blob_transactions_by_contract(
        &self,
        contract_name: &ContractName,
    ) -> Result<Vec<TransactionWithBlobs>> {
        self.get(
            &format!("v1/indexer/blob_transactions/contract/{contract_name}"),
            &format!("getting blob transactions for contract {contract_name}"),
        )
        .await
    }

    async fn get_blobs_by_tx_hash(&self, tx_hash: &TxHash) -> Result<Vec<APIBlob>> {
        self.get(
            &format!("v1/indexer/blobs/hash/{tx_hash}"),
            &format!("getting blob by transaction hash {tx_hash}"),
        )
        .await
    }

    async fn get_blob(&self, tx_hash: &TxHash, blob_index: BlobIndex) -> Result<APIBlob> {
        self.get(
            &format!("v1/indexer/blob/hash/{tx_hash}/index/{blob_index}"),
            &format!("getting blob with hash {tx_hash} and index {blob_index}"),
        )
        .await
    }
}

impl Deref for IndexerApiHttpClient {
    type Target = HttpClient;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}
