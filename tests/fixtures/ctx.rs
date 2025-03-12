#![allow(unused)]
#![allow(clippy::indexing_slicing)]

use std::time::Duration;

use anyhow::{Context, Result};
use api::APIContract;
use assertables::assert_ok;
use client_sdk::transaction_builder::ProvableBlobTx;
use reqwest::{Client, Url};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt},
};
use tracing::info;

use hyle::{
    model::*,
    rest::client::{IndexerApiHttpClient, NodeApiHttpClient},
    utils::conf::Conf,
};
use hyle_contract_sdk::{
    flatten_blobs, BlobIndex, ContractName, HyleOutput, Identity, ProgramId, StateCommitment,
    TxHash, Verifier,
};

use crate::fixtures::test_helpers::wait_height_timeout;

use super::test_helpers::{self, wait_height, ConfMaker};

pub trait E2EContract {
    fn verifier() -> Verifier;
    fn program_id() -> ProgramId;
    fn state_commitment() -> StateCommitment;
}

pub struct E2ECtx {
    pg: Option<ContainerAsync<Postgres>>,
    nodes: Vec<test_helpers::TestProcess>,
    clients: Vec<NodeApiHttpClient>,
    client_index: usize,
    indexer_client: Option<IndexerApiHttpClient>,
    slot_duration: u64,
}

impl E2ECtx {
    async fn init() -> ContainerAsync<Postgres> {
        // Start postgres DB with default settings for the indexer.
        Postgres::default()
            .with_tag("17-alpine")
            .with_cmd(["postgres", "-c", "log_statement=all"])
            .start()
            .await
            .unwrap()
    }

    fn build_nodes(
        count: usize,
        conf_maker: &mut ConfMaker,
    ) -> (Vec<test_helpers::TestProcess>, Vec<NodeApiHttpClient>) {
        let mut nodes = Vec::new();
        let mut clients = Vec::new();
        let mut peers = Vec::new();
        let mut confs = Vec::new();
        let mut genesis_stakers = std::collections::HashMap::new();

        for i in 0..count {
            let mut node_conf = conf_maker.build("node");
            node_conf.p2p.peers = peers.clone();
            genesis_stakers.insert(node_conf.id.clone(), 100);
            peers.push(node_conf.p2p.address.clone());
            confs.push(node_conf);
        }

        for node_conf in confs.iter_mut() {
            node_conf.genesis.stakers = genesis_stakers.clone();
            let node = test_helpers::TestProcess::new("hyle", node_conf.clone())
                //.log("hyle=info,tower_http=error")
                .start();

            // Request something on node1 to be sure it's alive and working
            let client = NodeApiHttpClient {
                url: Url::parse(&format!("http://{}", &node.conf.rest_address)).unwrap(),
                reqwest_client: Client::new(),
                api_key: None,
            };
            nodes.push(node);
            clients.push(client);
        }
        (nodes, clients)
    }

    pub async fn new_single(slot_duration: u64) -> Result<E2ECtx> {
        std::env::set_var("RISC0_DEV_MODE", "1");

        let mut conf_maker = ConfMaker::default();
        conf_maker.default.consensus.slot_duration = slot_duration;
        conf_maker.default.consensus.solo = true;
        conf_maker.default.genesis.stakers =
            vec![("single-node".to_string(), 100)].into_iter().collect();

        let node_conf = conf_maker.build("single-node");
        let node = test_helpers::TestProcess::new("hyle", node_conf)
            //.log("hyle=info,tower_http=error")
            .start();

        // Request something on node1 to be sure it's alive and working
        let client = NodeApiHttpClient {
            url: Url::parse(&format!("http://{}", &node.conf.rest_address)).unwrap(),
            reqwest_client: Client::new(),
            api_key: None,
        };

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        info!("🚀 E2E test environment is ready!");
        Ok(E2ECtx {
            pg: None,
            nodes: vec![node],
            clients: vec![client],
            client_index: 0,
            indexer_client: None,
            slot_duration,
        })
    }

    pub async fn new_single_with_indexer(slot_duration: u64) -> Result<E2ECtx> {
        std::env::set_var("RISC0_DEV_MODE", "1");

        let pg = Self::init().await;

        let mut conf_maker = ConfMaker::default();
        conf_maker.default.consensus.slot_duration = slot_duration;
        conf_maker.default.consensus.solo = true;
        conf_maker.default.genesis.stakers =
            vec![("single-node".to_string(), 100)].into_iter().collect();
        conf_maker.default.database_url = format!(
            "postgres://postgres:postgres@localhost:{}/postgres",
            pg.get_host_port_ipv4(5432).await.unwrap()
        );

        let node_conf = conf_maker.build("single-node");
        let node = test_helpers::TestProcess::new("hyle", node_conf.clone())
            //.log("hyle=info,tower_http=error")
            .start();

        // Request something on node1 to be sure it's alive and working
        let client = NodeApiHttpClient {
            url: Url::parse(&format!("http://{}", &node.conf.rest_address)).unwrap(),
            reqwest_client: Client::new(),
            api_key: None,
        };

        // Start indexer
        let mut indexer_conf = conf_maker.build("indexer");
        indexer_conf.da_address = node_conf.da_address.clone();
        let indexer = test_helpers::TestProcess::new("indexer", indexer_conf.clone()).start();

        let url = format!("http://{}", &indexer_conf.rest_address);
        let indexer_client = Some(IndexerApiHttpClient::new(url).unwrap());

        // Wait for node2 to properly spin up
        wait_height(&client, 1).await?;

        info!("🚀 E2E test environment is ready!");
        Ok(E2ECtx {
            pg: Some(pg),
            nodes: vec![node, indexer],
            clients: vec![client],
            client_index: 0,
            indexer_client,
            slot_duration,
        })
    }
    pub async fn new_multi(count: usize, slot_duration: u64) -> Result<E2ECtx> {
        std::env::set_var("RISC0_DEV_MODE", "1");

        let mut conf_maker = ConfMaker::default();
        conf_maker.default.consensus.slot_duration = slot_duration;

        let (nodes, clients) = Self::build_nodes(count, &mut conf_maker);
        wait_height_timeout(clients.first().unwrap(), 1, 120).await?;

        info!("🚀 E2E test environment is ready!");
        Ok(E2ECtx {
            pg: None,
            nodes,
            clients,
            client_index: 0,
            indexer_client: None,
            slot_duration,
        })
    }

    pub fn make_conf(&self, prefix: &str) -> Conf {
        let mut conf_maker = ConfMaker::default();
        let mut node_conf = conf_maker.build(prefix);
        node_conf.consensus.slot_duration = self.slot_duration;
        node_conf.p2p.peers = self
            .nodes
            .iter()
            .map(|node| node.conf.p2p.address.clone())
            .collect();
        node_conf
    }

    pub async fn add_node(&mut self) -> Result<&NodeApiHttpClient> {
        self.add_node_with_conf(self.make_conf("new_node")).await
    }

    pub async fn add_node_with_conf(&mut self, node_conf: Conf) -> Result<&NodeApiHttpClient> {
        let node = test_helpers::TestProcess::new("hyle", node_conf)
            //.log("hyle=info,tower_http=error")
            .start();
        // Request something on node1 to be sure it's alive and working
        let client = NodeApiHttpClient {
            url: Url::parse(&format!("http://{}", &node.conf.rest_address)).unwrap(),
            reqwest_client: Client::new(),
            api_key: None,
        };
        wait_height(&client, 1).await?;
        self.nodes.push(node);
        self.clients.push(client);
        Ok(self.clients.last().unwrap())
    }

    pub async fn new_multi_with_indexer(count: usize, slot_duration: u64) -> Result<E2ECtx> {
        std::env::set_var("RISC0_DEV_MODE", "1");

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
        let url = format!("http://{}", &indexer_conf.rest_address);
        clients.push(NodeApiHttpClient::new(url.clone()).unwrap());
        let indexer_client = Some(IndexerApiHttpClient::new(url).unwrap());

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
            indexer_client,
            slot_duration,
        })
    }

    pub async fn stop_node(&mut self, index: usize) -> Result<()> {
        self.nodes[index].stop().await
    }

    pub fn restart_node(&mut self, index: usize) -> Result<()> {
        // Stupid but it works
        let node = self.nodes.remove(index).start();
        self.nodes.insert(index, node);
        Ok(())
    }

    pub fn client(&self) -> &NodeApiHttpClient {
        &self.clients[self.client_index]
    }

    pub fn indexer_client(&self) -> &IndexerApiHttpClient {
        self.indexer_client.as_ref().unwrap()
    }

    pub fn has_indexer(&self) -> bool {
        self.pg.is_some()
    }

    pub async fn metrics(&self) -> Result<String> {
        let metrics = self.client().metrics().await?;

        Ok(metrics)
    }

    pub async fn register_contract<Contract>(&self, sender: Identity, name: &str) -> Result<()>
    where
        Contract: E2EContract,
    {
        let blobs = vec![RegisterContractAction {
            verifier: Contract::verifier(),
            program_id: Contract::program_id(),
            state_commitment: Contract::state_commitment(),
            contract_name: name.into(),
        }
        .as_blob("hyle".into(), None, None)];

        let tx = &BlobTransaction::new(sender.clone(), blobs.clone());
        assert_ok!(self.client().send_tx_blob(tx).await);

        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let resp = self.client().get_contract(&name.into()).await;
                if resp.is_err() || resp.is_err() {
                    info!("⏰ Waiting for contract {name} state to be ready");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                } else {
                    return;
                }
            }
        })
        .await?;
        Ok(())
    }

    pub async fn send_blob(&self, identity: Identity, blobs: Vec<Blob>) -> Result<TxHash> {
        self.client()
            .send_tx_blob(&BlobTransaction::new(identity, blobs))
            .await
    }

    pub async fn send_provable_blob_tx(&self, tx: &ProvableBlobTx) -> Result<TxHash> {
        self.client()
            .send_tx_blob(&BlobTransaction::new(tx.identity.clone(), tx.blobs.clone()))
            .await
    }

    pub async fn send_proof_single(&self, proof: ProofTransaction) -> Result<()> {
        assert_ok!(self.client().send_tx_proof(&proof).await);
        Ok(())
    }

    pub async fn send_proof(
        &self,
        contract_name: ContractName,
        proof: ProofData,
        verifies: Vec<TxHash>,
    ) -> Result<()> {
        assert_ok!(
            self.client()
                .send_tx_proof(&ProofTransaction {
                    contract_name: contract_name.clone(),
                    proof: proof.clone(),
                })
                .await
        );

        Ok(())
    }

    pub async fn wait_height(&self, height: u64) -> Result<()> {
        wait_height(self.client(), height).await
    }

    pub async fn get_contract(&self, name: &str) -> Result<Contract> {
        self.client().get_contract(&name.into()).await
    }
}
