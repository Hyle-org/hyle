use anyhow::{Context, Result};
use reqwest::{Response, Url};

#[cfg(feature = "node")]
use crate::tools::mock_workflow::RunScenario;
use crate::{
    model::consensus::ConsensusInfo,
    model::data_availability::Contract,
    model::indexer::ContractDb,
    model::rest::NodeInfo,
    model::{
        BlobTransaction, BlockHeight, ContractName, ProofTransaction, RecursiveProofTransaction,
        RegisterContractTransaction,
    },
};
use hyle_contract_sdk::{StateDigest, TxHash};
use staking::state::Staking;

pub struct ApiHttpClient {
    pub url: Url,
    pub reqwest_client: reqwest::Client,
}

impl ApiHttpClient {
    pub fn new(url: String) -> Self {
        Self {
            url: Url::parse(&url).expect("Invalid url"),
            reqwest_client: reqwest::Client::new(),
        }
    }

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

    pub async fn send_tx_recursive_proof(
        &self,
        tx: &RecursiveProofTransaction,
    ) -> Result<Response> {
        self.reqwest_client
            .post(format!("{}v1/tx/send/recursive_proof", self.url))
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

    pub async fn get_consensus_staking_state(&self) -> Result<Staking> {
        self.reqwest_client
            .get(format!("{}v1/consensus/staking_state", self.url))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting consensus staking state")?
            .json::<Staking>()
            .await
            .context("reading consensus staking state response")
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
            .context(format!("getting Contract {}", contract_name))?
            .json::<Contract>()
            .await
            .context("reading contract response")
    }

    pub async fn list_contracts(&self) -> Result<Vec<ContractDb>> {
        self.reqwest_client
            .get(format!("{}v1/indexer/contracts", self.url))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting Contract")?
            .json::<Vec<ContractDb>>()
            .await
            .context("reading contract response")
    }

    pub async fn get_indexer_contract(&self, contract_name: &ContractName) -> Result<Response> {
        self.reqwest_client
            .get(format!("{}v1/indexer/contract/{}", self.url, contract_name))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context(format!("getting Contract {}", contract_name))
    }

    pub async fn fetch_current_state<State>(&self, contract_name: &ContractName) -> Result<State>
    where
        State: TryFrom<hyle_contract_sdk::StateDigest, Error = anyhow::Error>,
    {
        let resp = self
            .get_indexer_contract(contract_name)
            .await?
            .json::<ContractDb>()
            .await?;

        StateDigest(resp.state_digest).try_into()
    }

    #[cfg(feature = "node")]
    pub async fn run_scenario_api_test(
        &self,
        qps: u64,
        injection_duration_seconds: u64,
    ) -> Result<Response> {
        self.reqwest_client
            .post(format!("{}v1/tools/run_scenario", self.url))
            .body(serde_json::to_string(&RunScenario::ApiTest {
                qps,
                injection_duration_seconds,
            })?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Starting api test scenario")
    }
}
