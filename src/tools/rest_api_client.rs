use std::fmt::Display;

use anyhow::{Context, Result};
#[cfg(feature = "node")]
use futures::SinkExt;
use reqwest::{Response, Url};
#[cfg(feature = "node")]
use tokio::net::TcpStream;
#[cfg(feature = "node")]
use tokio_util::codec::{Framed, LengthDelimitedCodec};

#[cfg(feature = "node")]
use crate::model::Transaction;
use crate::model::{
    consensus::ConsensusInfo,
    data_availability::Contract,
    indexer::{ContractDb, TransactionDb},
    rest::NodeInfo,
    BlobTransaction, BlockHeight, ContractName, ProofTransaction, ProofTransactionB64,
    RegisterContractTransaction,
};
#[cfg(feature = "node")]
use crate::tcp_server::TcpServerNetMessage;
#[cfg(feature = "node")]
use crate::tools::mock_workflow::RunScenario;
use hyle_contract_sdk::{StateDigest, TxHash};
use staking::state::Staking;

pub struct NodeApiHttpClient {
    pub url: Url,
    pub reqwest_client: reqwest::Client,
}

#[cfg(feature = "node")]
pub struct NodeTcpClient {
    pub framed: Framed<TcpStream, LengthDelimitedCodec>,
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
        let tx: ProofTransactionB64 = tx.clone().into();
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

#[cfg(feature = "node")]
impl NodeTcpClient {
    pub async fn new(url: String) -> Result<Self> {
        tracing::info!("Connecting to {}", url);
        let stream = TcpStream::connect(url).await?;
        let framed = Framed::new(stream, LengthDelimitedCodec::new());
        Ok(Self { framed })
    }

    pub async fn send_transaction(&mut self, transaction: Transaction) -> Result<()> {
        let msg: TcpServerNetMessage = transaction.into();
        self.framed
            .send(msg.to_binary()?.into())
            .await
            .context("Failed to send NetMessage")?;
        Ok(())
    }

    pub async fn send_encoded_message_no_response(&mut self, encoded_msg: Vec<u8>) -> Result<()> {
        self.framed
            .send(encoded_msg.into())
            .await
            .context("Failed to send NetMessage")?;
        Ok(())
    }
}

impl IndexerApiHttpClient {
    pub fn new(url: String) -> Result<Self> {
        Ok(Self {
            url: Url::parse(&url)?,
            reqwest_client: reqwest::Client::new(),
        })
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

    pub async fn query_indexer<U: Display>(&self, route: U) -> Result<Response> {
        self.reqwest_client
            .get(format!("{}v1/{}", self.url, route))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Running custom query to {route}")
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

    pub async fn get_transaction_by_hash(&self, tx_hash: &TxHash) -> Result<TransactionDb> {
        self.reqwest_client
            .get(format!("{}v1/indexer/transaction/hash/{tx_hash}", self.url))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("getting transaction by hash")?
            .json::<TransactionDb>()
            .await
            .context("reading transaction response")
    }
}
