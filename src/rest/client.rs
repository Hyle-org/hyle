use anyhow::{Context, Result};
use hyle_contract_sdk::TxHash;
use reqwest::{Response, Url};

use crate::{
    consensus::{staking::Staker, ConsensusInfo},
    model::{
        BlobTransaction, BlockHeight, ContractName, ProofTransaction, RegisterContractTransaction,
    },
    node_state::model::Contract,
    tools::mock_workflow::RunScenario,
};

use super::NodeInfo;

pub struct ApiHttpClient {
    pub url: Url,
    pub reqwest_client: reqwest::Client,
}

impl ApiHttpClient {
    pub async fn send_tx_blob(&self, tx: &BlobTransaction) -> Result<TxHash> {
        self.reqwest_client
            .post(format!("{}v1/tx/send/blob", self.url))
            .body(serde_json::to_string(tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx blob")?
            .json::<TxHash>()
            .await
            .context("reading tx hash response")
    }

    pub async fn send_tx_proof(&self, tx: &ProofTransaction) -> Result<Response> {
        self.reqwest_client
            .post(format!("{}v1/tx/send/proof", self.url))
            .body(serde_json::to_string(&tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx proof")
    }

    pub async fn send_tx_register_contract(
        &self,
        tx: &RegisterContractTransaction,
    ) -> Result<Response> {
        self.reqwest_client
            .post(format!("{}v1/contract/register", self.url))
            .body(serde_json::to_string(&tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx register contract")
    }

    pub async fn send_stake_tx(&self, tx: &Staker) -> Result<Response> {
        self.reqwest_client
            .post(format!("{}v1/tx/send/stake", self.url))
            .body(serde_json::to_string(&tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx stake")
    }

    pub async fn get_consensus_info(&self) -> Result<ConsensusInfo> {
        self.reqwest_client
            .get(format!("{}v1/consensus/info", self.url))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting consensus info")?
            .json::<ConsensusInfo>()
            .await
            .context("reading consensus info response")
    }

    pub async fn get_node_info(&self) -> Result<NodeInfo> {
        self.reqwest_client
            .get(format!("{}v1/info", self.url))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting node info")?
            .json::<NodeInfo>()
            .await
            .context("reading node info response")
    }

    pub async fn get_block_height(&self) -> Result<BlockHeight> {
        self.reqwest_client
            .get(format!("{}v1/da/block/height", self.url))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting block height")?
            .json::<BlockHeight>()
            .await
            .context("reading block height response")
    }

    pub async fn get_contract(&self, contract_name: &ContractName) -> Result<Contract> {
        self.reqwest_client
            .get(format!("{}v1/contract/{}", self.url, contract_name))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting Contract")?
            .json::<Contract>()
            .await
            .context("reading contract response")
    }

    pub async fn get_indexer_contract(&self, contract_name: &ContractName) -> Result<Response> {
        self.reqwest_client
            .get(format!("{}v1/indexer/contract/{}", self.url, contract_name))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting Contract")
    }

    pub async fn run_scenario_api_test(&self, qps: u64) -> Result<Response> {
        self.reqwest_client
            .post(format!("{}v1/tools/run_scenario", self.url))
            .body(serde_json::to_string(&RunScenario::ApiTest { qps })?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Starting api test scenario")
    }
}
