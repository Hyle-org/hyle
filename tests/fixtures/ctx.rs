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
    pg: Option<ContainerAsync<Postgres>>,
    nodes: Vec<test_helpers::TestProcess>,
    clients: Vec<ApiHttpClient>,
    client_index: usize,
}

impl E2ECtx {
    async fn init() -> ContainerAsync<Postgres> {
        // Start postgres DB with default settings for the indexer.
        Postgres::default().start().await.unwrap()
    }

    fn build_nodes(
        count: u16,
        conf_maker: &mut ConfMaker,
    ) -> (Vec<test_helpers::TestProcess>, Vec<ApiHttpClient>) {
        let mut nodes = Vec::new();
        let mut clients = Vec::new();
        let mut peers = Vec::new();
        for i in 0..count {
            let prefix = if i == 0 { "leader" } else { "node" };
            // Start 2 nodes
            let mut node_conf = conf_maker.build(prefix);
            node_conf.peers = peers.clone();
            peers.push(node_conf.host.clone());
            let node = test_helpers::TestProcess::new("node", node_conf).start();

            // Request something on node1 to be sure it's alive and working
            let client = ApiHttpClient {
                url: Url::parse(&format!("http://{}", &node.conf.rest)).unwrap(),
                reqwest_client: Client::new(),
            };
            nodes.push(node);
            clients.push(client);
        }
        (nodes, clients)
    }

    pub async fn new_single(slot_duration: u64) -> Result<E2ECtx> {
        let mut conf_maker = CONF_MAKER.lock().await;
        conf_maker.reset_default();
        conf_maker.default.consensus.slot_duration = slot_duration;

        let node_conf = conf_maker.build("single-node");
        let node = test_helpers::TestProcess::new("node", node_conf).start();

        // Request something on node1 to be sure it's alive and working
        let client = ApiHttpClient {
            url: Url::parse(&format!("http://{}", &node.conf.rest)).unwrap(),
            reqwest_client: Client::new(),
        };

        Ok(E2ECtx {
            pg: None,
            nodes: vec![node],
            clients: vec![client],
            client_index: 0,
        })
    }

    pub async fn new_multi(count: u16, slot_duration: u64) -> Result<E2ECtx> {
        let mut conf_maker = CONF_MAKER.lock().await;
        conf_maker.reset_default();
        conf_maker.default.consensus.slot_duration = slot_duration;

        let (nodes, clients) = Self::build_nodes(count, &mut conf_maker);
        wait_height(clients.first().unwrap(), 1).await?;

        Ok(E2ECtx {
            pg: None,
            nodes,
            clients,
            client_index: 0,
        })
    }

    pub async fn new_multi_with_indexer(count: u16, slot_duration: u64) -> Result<E2ECtx> {
        let pg = Self::init().await;

        let mut conf_maker = CONF_MAKER.lock().await;
        conf_maker.reset_default();
        conf_maker.default.consensus.slot_duration = slot_duration;
        conf_maker.default.database_url = format!(
            "postgres://postgres:postgres@localhost:{}/postgres",
            pg.get_host_port_ipv4(5432).await.unwrap()
        );

        let (mut nodes, mut clients) = Self::build_nodes(count, &mut conf_maker);

        // Start indexer
        let mut indexer_conf = conf_maker.build("indexer");
        indexer_conf.da_address = nodes.last().unwrap().conf.da_address.clone();
        let indexer = test_helpers::TestProcess::new("indexer", indexer_conf.clone()).start();

        nodes.push(indexer);
        clients.push(ApiHttpClient {
            url: Url::parse(&format!("http://{}", &indexer_conf.rest)).unwrap(),
            reqwest_client: Client::new(),
        });

        // Wait for node2 to properly spin up
        let client = clients.first().unwrap();
        wait_height(client, 1).await?;

        let pg = Some(pg);

        Ok(E2ECtx {
            pg,
            nodes,
            clients,
            client_index: 0,
        })
    }

    #[track_caller]
    pub fn on_indexer(self) -> E2ECtx {
        assert!(self.pg.is_some(), "Indexer is not started");

        Self {
            client_index: self.clients.len() - 1,
            ..self
        }
    }

    pub fn client(&self) -> &ApiHttpClient {
        &self.clients[self.client_index]
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
            .client()
            .send_tx_register_contract(tx)
            .await
            .and_then(|response| response.error_for_status().context("registering contract")));

        assert_ok!(self
            .client()
            .send_tx_register_contract(tx)
            .await
            .and_then(|response| response.error_for_status().context("registering contract")));

        Ok(())
    }

    pub async fn send_blob(&self, blobs: Vec<Blob>) -> Result<TxHash> {
        let blob_response = self
            .client()
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
            .client()
            .send_tx_proof(&ProofTransaction {
                blobs_references,
                proof
            })
            .await
            .and_then(|response| response.error_for_status().context("sending tx")));

        Ok(())
    }

    pub async fn wait_height(&self, height: u64) -> Result<()> {
        wait_height(&self.client(), height).await
    }

    pub async fn get_contract(&self, name: &str) -> Result<Contract> {
        let response = self
            .client()
            .get_contract(&name.into())
            .await
            .and_then(|response| response.error_for_status().context("Getting contract"));
        assert_ok!(response);

        let contract = response.unwrap().json::<Contract>().await?;
        Ok(contract)
    }
}
