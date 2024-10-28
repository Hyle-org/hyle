use anyhow::{Context, Result};
use assertables::assert_ok;
use reqwest::{Client, Url};
use std::sync::LazyLock;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use tokio::sync::Mutex;

use hyle::{
    model::{
        Blob, BlobReference, BlobTransaction, ProofData, ProofTransaction,
        RegisterContractTransaction,
    },
    node_state::model::Contract,
    rest::client::ApiHttpClient,
};
use hyle_contract_sdk::{Identity, StateDigest, TxHash};

use super::test_helpers::{self, wait_height, ConfMaker};

static CONF_MAKER: LazyLock<Mutex<ConfMaker>> = LazyLock::new(|| Mutex::new(ConfMaker::default()));

pub trait E2EContract {
    fn verifier() -> String;
    fn program_id() -> Vec<u8>;
    fn state_digest() -> StateDigest;
}

pub struct E2ECtx {
    pg: ContainerAsync<Postgres>,
    node1: test_helpers::TestProcess,
    node2: test_helpers::TestProcess,
    client: ApiHttpClient,
    indexer: test_helpers::TestProcess,
}

impl E2ECtx {
    pub async fn new() -> Result<E2ECtx> {
        // Start postgres DB with default settings for the indexer.
        let pg = Postgres::default().start().await.unwrap();

        let mut conf_maker = CONF_MAKER.lock().await;
        conf_maker.default.consensus.slot_duration = 500;
        conf_maker.default.database_url = format!(
            "postgres://postgres:postgres@localhost:{}/postgres",
            pg.get_host_port_ipv4(5432).await.unwrap()
        );

        // Start 2 nodes
        let node1 = test_helpers::TestProcess::new("node", conf_maker.build("leader")).start();

        // Request something on node1 to be sure it's alive and working
        let client = ApiHttpClient {
            url: Url::parse(&format!("http://{}", &node1.conf.rest)).unwrap(),
            reqwest_client: Client::new(),
        };

        let mut node2_conf = conf_maker.build("node");
        node2_conf.peers = vec![node1.conf.host.clone()];
        let node2 = test_helpers::TestProcess::new("node", node2_conf).start();

        // Start indexer
        let mut indexer_conf = conf_maker.build("indexer");
        indexer_conf.da_address = node2.conf.da_address.clone();
        let indexer = test_helpers::TestProcess::new("indexer", indexer_conf).start();

        // Wait for node2 to properly spin up
        wait_height(&client, 1).await?;

        Ok(E2ECtx {
            pg,
            node1,
            node2,
            client,
            indexer,
        })
    }

    pub fn on_indexer(self) -> E2ECtx {
        // Check that the indexer did index things
        let client_indexer = ApiHttpClient {
            url: Url::parse(&format!("http://{}", &self.indexer.conf.rest)).unwrap(),
            reqwest_client: Client::new(),
        };
        Self {
            pg: self.pg,
            node1: self.node1,
            node2: self.node2,
            client: client_indexer,
            indexer: self.indexer,
        }
    }

    pub async fn register_contract<Contract>(&self, name: &str) -> Result<()>
    where
        Contract: E2EContract,
    {
        let tx = &RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: Contract::verifier(),
            program_id: Contract::program_id(),
            state_digest: Contract::state_digest(),
            contract_name: name.into(),
        };
        assert_ok!(self
            .client
            .send_tx_register_contract(tx)
            .await
            .and_then(|response| response.error_for_status().context("registering contract")));

        assert_ok!(self
            .client
            .send_tx_register_contract(tx)
            .await
            .and_then(|response| response.error_for_status().context("registering contract")));

        Ok(())
    }

    pub async fn send_blob(&self, blobs: Vec<Blob>) -> Result<TxHash> {
        let blob_response = self
            .client
            .send_tx_blob(&BlobTransaction {
                identity: Identity("client".to_string()),
                blobs,
            })
            .await
            .and_then(|response| response.error_for_status().context("sending tx"));

        assert_ok!(blob_response);

        blob_response
            .unwrap()
            .json::<TxHash>()
            .await
            .map_err(|e| e.into())
    }

    pub async fn send_proof(
        &self,
        blobs_references: Vec<BlobReference>,
        proof: ProofData,
    ) -> Result<()> {
        assert_ok!(self
            .client
            .send_tx_proof(&ProofTransaction {
                blobs_references,
                proof
            })
            .await
            .and_then(|response| response.error_for_status().context("sending tx")));

        Ok(())
    }

    pub async fn wait_height(&self, height: u64) -> Result<()> {
        wait_height(&self.client, height).await
    }

    pub async fn get_contract(&self, name: &str) -> Result<Contract> {
        let response = self
            .client
            .get_contract(&name.into())
            .await
            .and_then(|response| response.error_for_status().context("Getting contract"));
        assert_ok!(response);

        let contract = response.unwrap().json::<Contract>().await?;
        Ok(contract)
    }
}
