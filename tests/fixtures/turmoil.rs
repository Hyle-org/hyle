#![allow(dead_code)]
#![cfg(feature = "turmoil")]
#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use client_sdk::rest_client::NodeApiHttpClient;
use hyle::{entrypoint::main_process, utils::conf::Conf};
use hyle_crypto::BlstCrypto;
use hyle_net::net::Sim;
use rand::{rngs::StdRng, RngCore, SeedableRng};
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

    pub fn from(conf: &Conf) -> TurmoilNodeProcess {
        let client =
            NodeApiHttpClient::new(format!("http://{}:{}", conf.id, &conf.rest_server_port))
                .expect("Creating client");
        TurmoilNodeProcess {
            conf: conf.clone(),
            client: client.with_retry(3, Duration::from_millis(1000)),
        }
    }
}

#[derive(Clone)]
pub struct TurmoilCtx {
    pub nodes: Vec<TurmoilNodeProcess>,
    slot_duration: Duration,
    seed: u64,
    pub rng: StdRng,
}

impl TurmoilCtx {
    pub fn build_conf(seed: u64, i: usize) -> Conf {
        let mut node_conf = Conf {
            id: format!("node-{}", i),
            ..ConfMaker::default().default
        };
        node_conf.data_directory.pop();
        node_conf
            .data_directory
            .push(format!("data_{}_{}", seed, node_conf.id));
        node_conf
    }

    fn build_nodes(count: usize, slot_duration: Duration, seed: u64) -> Vec<TurmoilNodeProcess> {
        let mut nodes = Vec::new();
        let mut peers = Vec::new();
        let mut confs = Vec::new();
        let mut genesis_stakers = std::collections::HashMap::new();

        for i in 0..count {
            let mut node_conf = Self::build_conf(seed, i + 1);
            _ = std::fs::remove_dir_all(&node_conf.data_directory);
            node_conf.consensus.slot_duration = slot_duration;
            node_conf.p2p.peers = peers.clone();
            genesis_stakers.insert(node_conf.id.clone(), 100);
            peers.push(format!("{}:{}", node_conf.id, node_conf.p2p.server_port));
            confs.push(node_conf);
        }

        for node_conf in confs.iter_mut() {
            node_conf.genesis.stakers = genesis_stakers.clone();
            let node = TurmoilNodeProcess::from(node_conf);
            nodes.push(node);
        }
        nodes
    }
    pub fn new_multi(
        count: usize,
        slot_duration_ms: u64,
        seed: u64,
        sim: &mut Sim<'_>,
    ) -> Result<TurmoilCtx> {
        std::env::set_var("RISC0_DEV_MODE", "1");

        let rng = StdRng::seed_from_u64(seed);

        let slot_duration = Duration::from_millis(slot_duration_ms);

        let nodes = Self::build_nodes(count, slot_duration, seed);

        _ = Self::setup_simulation(nodes.as_slice(), sim);

        info!("🚀 E2E test environment is ready!");

        Ok(TurmoilCtx {
            nodes,
            slot_duration: Duration::from_millis(slot_duration_ms),
            seed,
            rng,
        })
    }

    pub fn add_node_to_simulation(&mut self, sim: &mut Sim<'_>) -> Result<NodeApiHttpClient> {
        let mut node_conf = Self::build_conf(self.seed, self.nodes.len() + 1);
        node_conf.consensus.slot_duration = self.slot_duration;
        node_conf.p2p.peers = self
            .nodes
            .iter()
            .map(|node| format!("{}:{}", node.conf.id, node.conf.p2p.server_port))
            .collect();

        let node = TurmoilNodeProcess::from(&node_conf);

        _ = Self::setup_simulation(&[node.clone()], sim);

        self.nodes.push(node);
        Ok(self.nodes.last().unwrap().client.clone())
    }

    fn setup_simulation(nodes: &[TurmoilNodeProcess], sim: &mut Sim<'_>) -> anyhow::Result<()> {
        let mut nodes = nodes.to_vec();
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

    pub fn clean(&self) -> Result<()> {
        // Cleaning
        for node in self.nodes.iter() {
            _ = std::fs::remove_dir_all(node.conf.data_directory.clone());
        }

        Ok(())
    }

    pub fn client(&self) -> NodeApiHttpClient {
        self.nodes.first().unwrap().client.clone()
    }

    pub fn conf(&self, n: u64) -> Conf {
        self.nodes.get((n - 1) as usize).unwrap().clone().conf
    }

    /// Generate a random number between specified `from` and `to`
    pub fn random_between(&mut self, from: u64, to: u64) -> u64 {
        from + (self.rng.next_u64() % (to - from + 1))
    }

    /// Pick randomly a node id of the current context
    pub fn random_id(&mut self) -> String {
        let random_n = self.random_between(1, self.nodes.len() as u64);
        self.conf(random_n).id
    }

    /// Pick randomly two **different** node ids of the current context
    pub fn random_id_pair(&mut self) -> (String, String) {
        let from = self.random_id();
        let to = loop {
            let candidate = self.random_id();
            if candidate != from {
                break candidate;
            }
        };

        (from, to)
    }
}
