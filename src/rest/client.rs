use anyhow::{Context, Result};
use reqwest::{Response, Url};

use crate::{
    consensus::Slot,
    model::{
        BlobTransaction, BlockHeight, ContractName, ProofTransaction, RegisterContractTransaction,
    },
    tools::mock_workflow::RunScenario,
};

pub struct ApiHttpClient {
    pub url: Url,
    pub reqwest_client: reqwest::Client,
}

impl ApiHttpClient {
    pub async fn send_tx_blob(&self, tx: &BlobTransaction) -> Result<Response> {
        self.reqwest_client
            .post(format!("{}v1/tx/send/blob", self.url))
            .body(serde_json::to_string(tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx blob")
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

    pub async fn get_current_slot(&self) -> Result<BlockHeight> {
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

    pub async fn get_contract(&self, contract_name: &ContractName) -> Result<Response> {
        self.reqwest_client
            .get(format!("{}v1/contract/{}", self.url, contract_name))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting Contract")
    }

    pub async fn get_indexer_contract(&self, contract_name: &ContractName) -> Result<Response> {
        self.reqwest_client
            .get(format!("{}v1/indexer/contract/{}", self.url, contract_name))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting Contract")
    }

    pub async fn run_scenario_api_test(&self) -> Result<Response> {
        self.reqwest_client
            .post(format!("{}v1/tools/run_scenario", self.url))
            .body(serde_json::to_string(&RunScenario::ApiTest)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Starting api test scenario")
    }
}
