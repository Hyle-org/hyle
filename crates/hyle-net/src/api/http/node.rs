use std::ops::Deref;

use anyhow::Result;
use sdk::{
    api::*, BlobTransaction, BlockHeight, ConsensusInfo, Contract, ContractName, ProofTransaction,
    TxHash, UnsettledBlobTransaction,
};

use crate::http::HttpClient;

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
            },
        })
    }
    pub async fn register_contract(&self, tx: &APIRegisterContract) -> Result<TxHash> {
        self.post_json("v1/contract/register", tx, "Registering contract")
            .await
    }

    pub async fn send_tx_blob(&self, tx: &BlobTransaction) -> Result<TxHash> {
        self.post_json("v1/tx/send/blob", tx, "Sending tx blob")
            .await
    }

    pub async fn send_tx_proof(&self, tx: &ProofTransaction) -> Result<TxHash> {
        self.post_json("v1/tx/send/proof", tx, "Sending tx proof")
            .await
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
        self.get("v1/metrics", "getting node metrics").await
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
}
impl Deref for NodeApiHttpClient {
    type Target = HttpClient;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}
