#![allow(unused)]

use anyhow::{Context, Result};
use assertables::assert_ok;
use reqwest::{Client, Url};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use tracing::info;

use hyle::{
    indexer::model::ContractDb,
    model::{
        Blob, BlobReference, BlobTransaction, ProofData, ProofTransaction,
        RegisterContractTransaction,
    },
    node_state::model::Contract,
    rest::client::ApiHttpClient,
};
use hyle_contract_sdk::{Identity, StateDigest, TxHash};

use super::test_helpers::{self, wait_height, ConfMaker};

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
    indexer_client_index: usize,
}

impl E2ECtx {
    async fn init() -> ContainerAsync<Postgres> {
        // Start postgres DB with default settings for the indexer.
        Postgres::default().start().await.unwrap()
    }

    fn build_nodes(
        count: usize,
        conf_maker: &mut ConfMaker,
    ) -> (Vec<test_helpers::TestProcess>, Vec<ApiHttpClient>) {
        let mut nodes = Vec::new();
        let mut clients = Vec::new();
        let mut peers = Vec::new();
        let mut confs = Vec::new();
        let mut genesis_stakers = std::collections::HashMap::new();

        for i in 0..count {
            let mut node_conf = conf_maker.build("node");
            node_conf.peers = peers.clone();
            genesis_stakers.insert(node_conf.id.clone(), 100);
            peers.push(node_conf.host.clone());
            confs.push(node_conf);
        }

        for node_conf in confs.iter_mut() {
            node_conf.consensus.genesis_stakers = genesis_stakers.clone();
            let node = test_helpers::TestProcess::new("node", node_conf.clone())
                //.log("hyle=info,tower_http=error")
                .start();

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
        let mut conf_maker = ConfMaker::default();
        conf_maker.default.consensus.slot_duration = slot_duration;
        conf_maker.default.single_node = Some(true);
        conf_maker.default.consensus.genesis_stakers =
            vec![("single-node".to_string(), 100)].into_iter().collect();

        let node_conf = conf_maker.build("single-node");
        let node = test_helpers::TestProcess::new("node", node_conf)
            //.log("hyle=info,tower_http=error")
            .start();

        // Request something on node1 to be sure it's alive and working
        let client = ApiHttpClient {
            url: Url::parse(&format!("http://{}", &node.conf.rest)).unwrap(),
            reqwest_client: Client::new(),
        };

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        info!("🚀 E2E test environment is ready!");
        Ok(E2ECtx {
            pg: None,
            nodes: vec![node],
            clients: vec![client],
            client_index: 0,
            indexer_client_index: 0,
        })
    }

    pub async fn new_multi(count: usize, slot_duration: u64) -> Result<E2ECtx> {
        let mut conf_maker = ConfMaker::default();
        conf_maker.default.consensus.slot_duration = slot_duration;

        let (nodes, clients) = Self::build_nodes(count, &mut conf_maker);
        wait_height(clients.first().unwrap(), 1).await?;

        info!("🚀 E2E test environment is ready!");
        Ok(E2ECtx {
            pg: None,
            nodes,
            clients,
            client_index: 0,
            indexer_client_index: 0,
        })
    }

    pub async fn add_node(&mut self) -> Result<&ApiHttpClient> {
        let mut conf_maker = ConfMaker::default();
        let mut node_conf = conf_maker.build("new-node");
        //node_conf.peers = vec![self.nodes[0].conf.host.clone()];
        node_conf.peers = self
            .nodes
            .iter()
            .map(|node| node.conf.host.clone())
            .collect();
        let node = test_helpers::TestProcess::new("node", node_conf)
            //.log("hyle=info,tower_http=error")
            .start();
        // Request something on node1 to be sure it's alive and working
        let client = ApiHttpClient {
            url: Url::parse(&format!("http://{}", &node.conf.rest)).unwrap(),
            reqwest_client: Client::new(),
        };
        wait_height(&client, 1).await?;
        self.nodes.push(node);
        self.clients.push(client);
        Ok(self.clients.last().unwrap())
    }

    pub async fn new_multi_with_indexer(count: usize, slot_duration: u64) -> Result<E2ECtx> {
        let pg = Self::init().await;

        let mut conf_maker = ConfMaker::default();
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
        let indexer_client_index = clients.len() - 1;

        // Wait for node2 to properly spin up
        let client = clients.first().unwrap();
        wait_height(client, 1).await?;

        let pg = Some(pg);

        info!("🚀 E2E test environment is ready!");
        Ok(E2ECtx {
            pg,
            nodes,
            clients,
            client_index: 0,
            indexer_client_index,
        })
    }

    pub fn client(&self) -> &ApiHttpClient {
        &self.clients[self.client_index]
    }

    pub fn has_indexer(&self) -> bool {
        self.pg.is_some()
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

        Ok(())
    }

    pub async fn send_blob(&self, identity: Identity, blobs: Vec<Blob>) -> Result<TxHash> {
        let blob_response = self
            .client()
            .send_tx_blob(&BlobTransaction { identity, blobs })
            .await
            .and_then(|response| response.error_for_status().context("sending tx"));

        let response = match blob_response {
            Ok(response) => response,
            Err(e) => panic!("Error sending blob tx: {:?}", e),
        };

        response.json::<TxHash>().await.map_err(|e| e.into())
    }

    pub async fn send_proof(
        &self,
        blobs_references: Vec<BlobReference>,
        proof: ProofData,
    ) -> Result<()> {
        assert_ok!(self
            .client()
            .send_tx_proof(&ProofTransaction {
                blobs_references: blobs_references.clone(),
                proof: proof.clone()
            })
            .await
            .and_then(|response| response.error_for_status().context("sending tx")));

        Ok(())
    }

    pub async fn wait_height(&self, height: u64) -> Result<()> {
        wait_height(self.client(), height).await
    }

    pub async fn get_contract(&self, name: &str) -> Result<Contract> {
        let contract_response = self
            .client()
            .get_contract(&name.into())
            .await
            .and_then(|response| response.error_for_status().context("Getting contract"));

        let response = match contract_response {
            Ok(response) => response,
            Err(e) => panic!("Error getting contract: {:?}", e),
        };

        let contract = response.json::<Contract>().await?;
        Ok(contract)
    }

    pub async fn get_indexer_contract(&self, name: &str) -> Result<ContractDb> {
        let indexer_contract_response = self.clients[self.indexer_client_index]
            .get_indexer_contract(&name.into())
            .await
            .and_then(|response| response.error_for_status().context("Getting contract"));

        let response = match indexer_contract_response {
            Ok(response) => response,
            Err(e) => panic!("Error getting contract: {:?}", e),
        };

        let contract = response.json::<ContractDb>().await?;
        Ok(contract)
    }
}
