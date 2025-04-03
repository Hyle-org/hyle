#![allow(dead_code)]
#![cfg(feature = "turmoil")]
#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use client_sdk::rest_client::NodeApiHttpClient;
use hyle::{
    entrypoint::main_process,
    utils::{conf::Conf, crypto::BlstCrypto},
};
use hyle_net::net::Sim;
use rand::{rngs::StdRng, RngCore, SeedableRng};
use sha3::digest::consts::U67108864;
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

impl ConfMaker {
    pub fn build_turmoil(&mut self, prefix: &str, seed: u64) -> Conf {
        self.i += 1;

        let mut node_conf = Conf {
            id: if prefix == "single-node" {
                prefix.into()
            } else {
                format!("{}-{}", prefix, self.i)
            },
            ..self.default.clone()
        };
        node_conf.hostname = node_conf.id.clone();
        node_conf.data_directory.pop();
        node_conf
            .data_directory
            .push(format!("data_{}_{}", seed, node_conf.id));
        node_conf
    }
}

#[derive(Clone)]
pub struct TurmoilCtx {
    pub nodes: Vec<TurmoilNodeProcess>,
    slot_duration: Duration,
    pub rng: StdRng,
}

impl TurmoilCtx {
    fn build_nodes(count: usize, conf_maker: &mut ConfMaker, seed: u64) -> Vec<TurmoilNodeProcess> {
        let mut nodes = Vec::new();
        let mut peers = Vec::new();
        let mut confs = Vec::new();
        let mut genesis_stakers = std::collections::HashMap::new();

        for _ in 0..count {
            let mut node_conf = conf_maker.build_turmoil("node", seed);
            _ = std::fs::remove_dir_all(&node_conf.data_directory);
            node_conf.p2p.peers = peers.clone();
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

            nodes.push(node);
        }
        nodes
    }
    pub fn new_multi(count: usize, slot_duration_ms: u64, seed: u64) -> Result<TurmoilCtx> {
        std::env::set_var("RISC0_DEV_MODE", "1");

        let mut conf_maker = ConfMaker::default();
        conf_maker.default.consensus.slot_duration = Duration::from_millis(slot_duration_ms);

        let nodes = Self::build_nodes(count, &mut conf_maker, seed);

        info!("🚀 E2E test environment is ready!");
        Ok(TurmoilCtx {
            nodes,
            slot_duration: Duration::from_millis(slot_duration_ms),
            rng: StdRng::seed_from_u64(seed),
        })
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

    pub fn conf(&self, n: u64) -> Conf {
        self.nodes.get((n - 1) as usize).unwrap().clone().conf
    }

    pub fn random(&mut self, from: u64, to: u64) -> u64 {
        from + (self.rng.next_u64() % (to - from))
    }
}
