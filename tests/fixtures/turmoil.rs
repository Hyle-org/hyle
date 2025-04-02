#![allow(dead_code)]
#![cfg(feature = "turmoil")]
#![cfg(test)]

use std::sync::Arc;

use anyhow::Context;
use client_sdk::rest_client::NodeApiHttpClient;
use hyle::{
    entrypoint::main_process,
    utils::{conf::Conf, crypto::BlstCrypto},
};
use hyle_model::Contract;
use hyle_net::net::Sim;
use tokio::sync::Mutex;
use tracing::info;

use anyhow::Result;

use crate::fixtures::test_helpers::ConfMaker;
#[derive(Clone)]
pub struct TurmoilNodeProcess {
    pub conf: Conf,
    pub client: NodeApiHttpClient,
}

impl TurmoilNodeProcess {
    pub async fn start(&self) -> anyhow::Result<()> {
        let crypto = Arc::new(BlstCrypto::new(&self.conf.id).context("Creating crypto")?);

        main_process(self.conf.clone(), Some(crypto)).await?;

        Ok(())
    }
}

pub struct TurmoilCtx {
    pub nodes: Vec<TurmoilNodeProcess>,
    slot_duration: u64,
}

impl TurmoilCtx {
    fn build_nodes(count: usize, conf_maker: &mut ConfMaker) -> Vec<TurmoilNodeProcess> {
        let mut nodes = Vec::new();
        let mut peers = Vec::new();
        let mut confs = Vec::new();
        let mut genesis_stakers = std::collections::HashMap::new();

        for _ in 0..count {
            let mut node_conf = conf_maker.build("node");
            node_conf.p2p.peers = peers.clone();
            node_conf.hostname = node_conf.id.clone();
            node_conf.data_directory.pop();
            node_conf
                .data_directory
                .push(format!("data_{}", node_conf.id));
            genesis_stakers.insert(node_conf.id.clone(), 100);
            peers.push(format!("{}:{}", node_conf.id, node_conf.p2p.server_port));
            confs.push(node_conf);
        }

        for node_conf in confs.iter_mut() {
            node_conf.genesis.stakers = genesis_stakers.clone();
            let conf_clone = node_conf.clone();
            let client = NodeApiHttpClient::new(format!(
                "http://{}:{}",
                conf_clone.id, &conf_clone.rest_server_port
            ))
            .expect("Creating client");
            let node = TurmoilNodeProcess {
                conf: conf_clone.clone(),
                client: client.clone(),
            };

            // Request something on node1 to be sure it's alive and working
            nodes.push(node);
        }
        nodes
    }
    pub fn new_multi(count: usize, slot_duration: u64) -> Result<TurmoilCtx> {
        std::env::set_var("RISC0_DEV_MODE", "1");

        let mut conf_maker = ConfMaker::default();
        conf_maker.default.consensus.slot_duration = slot_duration;

        let nodes = Self::build_nodes(count, &mut conf_maker);

        info!("🚀 E2E test environment is ready!");
        Ok(TurmoilCtx {
            nodes,
            slot_duration,
        })
    }

    pub async fn new_single(slot_duration: u64) -> Result<TurmoilCtx> {
        std::env::set_var("RISC0_DEV_MODE", "1");

        let mut conf_maker = ConfMaker::default();
        conf_maker.default.consensus.slot_duration = slot_duration;
        conf_maker.default.consensus.solo = true;
        conf_maker.default.genesis.stakers =
            vec![("single-node".to_string(), 100)].into_iter().collect();

        let node_conf = conf_maker.build("single-node");
        let client = NodeApiHttpClient::new(format!(
            "http://{}:{}",
            node_conf.id, &node_conf.rest_server_port
        ))
        .expect("Creating client");
        let node = TurmoilNodeProcess {
            conf: node_conf.clone(),
            client: client.clone(),
        };

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        info!("🚀 E2E test environment is ready!");
        Ok(TurmoilCtx {
            nodes: vec![node],
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
            .map(|node| format!("{}:{}", node.conf.id, node.conf.p2p.server_port.clone()))
            .collect();
        node_conf
    }

    pub fn setup_simulation(&self, sim: &mut Sim<'_>) -> anyhow::Result<()> {
        let mut nodes = self.nodes.clone();
        nodes.reverse();

        let turmoil_node = nodes.pop().unwrap();
        {
            let id = turmoil_node.conf.id.clone();
            let cloned = Arc::new(Mutex::new(turmoil_node.clone())); // Permet de partager la variable

            let f = {
                let cloned = Arc::clone(&cloned); // Clonage pour éviter de déplacer
                move || {
                    let cloned = Arc::clone(&cloned);
                    async move {
                        let node = cloned.lock().await; // Accès mutable au nœud
                        _ = node.start().await;
                        Ok(())
                    }
                }
            };

            sim.host(id, f);
        }
        while let Some(turmoil_node) = nodes.pop() {
            let id = turmoil_node.conf.id.clone();
            let cloned = Arc::new(Mutex::new(turmoil_node.clone())); // Permet de partager la variable

            let f = {
                let cloned = Arc::clone(&cloned); // Clonage pour éviter de déplacer
                move || {
                    let cloned = Arc::clone(&cloned);
                    async move {
                        let node = cloned.lock().await; // Accès mutable au nœud
                        _ = node.start().await;
                        Ok(())
                    }
                }
            };

            sim.host(id, f);
        }
        Ok(())
    }
    pub fn client(&self) -> NodeApiHttpClient {
        self.nodes.first().unwrap().client.clone()
    }

    pub async fn get_contract(&self, name: &str) -> Result<Contract> {
        self.client().get_contract(&name.into()).await
    }
}
