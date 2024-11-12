use std::collections::HashMap;

use crate::{
    bus::{bus_client, BusMessage, SharedMessageBus},
    consensus::staking::{Stake, Staker},
    handle_messages,
    model::{SharedRunContext, Transaction, TransactionData, ValidatorPublicKey},
    p2p::network::PeerEvent,
    utils::{conf::SharedConf, crypto::SharedBlstCrypto, modules::Module},
};
use anyhow::{Error, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum GenesisEvent {
    Ready {
        first_round_leader: ValidatorPublicKey,
    },
    GenesisBlock {
        stake_txs: Vec<Transaction>,
        initial_validators: Vec<ValidatorPublicKey>,
    },
}
impl BusMessage for GenesisEvent {}

bus_client! {
struct GenesisBusClient {
    sender(GenesisEvent),
    receiver(PeerEvent),
}
}

pub struct Genesis {
    config: SharedConf,
    bus: GenesisBusClient,
    peer_pubkey: HashMap<String, ValidatorPublicKey>,
    crypto: SharedBlstCrypto,
}

impl Module for Genesis {
    type Context = SharedRunContext;
    fn name() -> &'static str {
        "Genesis"
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = GenesisBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        Ok(Genesis {
            config: ctx.common.config.clone(),
            bus,
            peer_pubkey: HashMap::new(),
            crypto: ctx.node.crypto.clone(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl Genesis {
    pub async fn start(&mut self) -> Result<(), Error> {
        if !self
            .config
            .consensus
            .genesis_stakers
            .contains_key(&self.config.id)
        {
            return Ok(());
        }

        // We will start from the genesis block.
        self.peer_pubkey.insert(
            self.config.id.clone(),
            self.crypto.validator_pubkey().clone(),
        );

        // Wait until we've connected with all other genesis peers.
        if self.config.id != "single-node" {
            handle_messages! {
                on_bus self.bus,
                listen<PeerEvent> msg => {
                    match msg {
                        PeerEvent::NewPeer { name, pubkey } => {
                            info!("🌱 New peer {}({}) added to genesis", &name, &pubkey);
                            self.peer_pubkey
                                .insert(name.clone(), pubkey.clone());

                            // Once we know everyone in the initial quorum, craft & process the genesis block.
                            if self.peer_pubkey.len()
                                == self.config.consensus.genesis_stakers.len() {
                                break
                            } else {
                                info!("🌱 Waiting for {} more peers to join genesis", self.config.consensus.genesis_stakers.len() - self.peer_pubkey.len());
                            }
                        }
                    }
                }
            }
        }

        let mut initial_validators = self.peer_pubkey.values().cloned().collect::<Vec<_>>();
        initial_validators.sort();

        // At this point, we can setup the genesis block.
        _ = self.bus.send(GenesisEvent::GenesisBlock {
            initial_validators,
            stake_txs: self
                .peer_pubkey
                .iter()
                .map(|(k, v)| {
                    Transaction::wrap(TransactionData::Stake(Staker {
                        pubkey: v.clone(),
                        stake: Stake {
                            amount: *self.config.consensus.genesis_stakers.get(k).unwrap_or(&100),
                        },
                    }))
                })
                .collect(),
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use assertables::assert_matches;

    use super::*;
    use crate::bus::SharedMessageBus;
    use crate::utils::conf::Conf;
    use crate::utils::crypto::BlstCrypto;
    use std::collections::HashMap;
    use std::sync::Arc;

    bus_client! {
    struct TestGenesisBusClient {
        sender(PeerEvent),
        receiver(GenesisEvent),
    }
    }

    async fn new(config: Conf) -> (Genesis, TestGenesisBusClient) {
        let shared_bus = SharedMessageBus::default();
        let bus = GenesisBusClient::new_from_bus(shared_bus.new_handle()).await;
        let test_bus = TestGenesisBusClient::new_from_bus(shared_bus.new_handle()).await;
        let crypto = Arc::new(BlstCrypto::new_random());
        (
            Genesis {
                config: Arc::new(config),
                bus,
                peer_pubkey: HashMap::new(),
                crypto,
            },
            test_bus,
        )
    }

    #[test_log::test(tokio::test)]
    async fn test_not_part_of_genesis() {
        let config = Conf {
            id: "node-4".to_string(),
            consensus: crate::utils::conf::Consensus {
                genesis_stakers: vec![("node-1".into(), 100)].into_iter().collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let (mut genesis, _) = new(config).await;

        // Start the Genesis module
        let result = genesis.start().await;

        // Verify the start method executed correctly
        assert!(result.is_ok());
    }

    #[test_log::test(tokio::test)]
    async fn test_genesis_single() {
        let config = Conf {
            id: "single-node".to_string(),
            consensus: crate::utils::conf::Consensus {
                genesis_stakers: vec![("single-node".into(), 100)].into_iter().collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let (mut genesis, _) = new(config).await;

        // Start the Genesis module
        let result = genesis.start().await;

        // Verify the start method executed correctly
        assert!(result.is_ok());
    }

    #[test_log::test(tokio::test)]
    async fn test_genesis_as_leader() {
        let config = Conf {
            id: "node-1".to_string(),
            consensus: crate::utils::conf::Consensus {
                genesis_stakers: vec![("node-1".into(), 100), ("node-2".into(), 100)]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let (mut genesis, mut bus) = new(config).await;

        bus.send(PeerEvent::NewPeer {
            name: "node-2".into(),
            pubkey: ValidatorPublicKey("aaa".into()),
        })
        .expect("send");

        // Start the Genesis module
        let result = genesis.start().await;

        assert!(result.is_ok());

        let rec = bus.recv().await.expect("recv");
        assert_matches!(rec, GenesisEvent::GenesisBlock { .. });
        if let GenesisEvent::GenesisBlock {
            stake_txs,
            initial_validators,
        } = rec
        {
            assert_eq!(stake_txs.len(), 2);
            assert_eq!(initial_validators.len(), 2);
        }

        let rec = bus.recv().await.expect("recv");
        assert_matches!(rec, GenesisEvent::Ready { .. });
        if let GenesisEvent::Ready { first_round_leader } = rec {
            assert_eq!(first_round_leader, *genesis.crypto.validator_pubkey());
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_genesis_as_follower() {
        let config = Conf {
            id: "node-2".to_string(),
            consensus: crate::utils::conf::Consensus {
                genesis_stakers: vec![("node-1".into(), 100), ("node-2".into(), 100)]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let (mut genesis, mut bus) = new(config).await;

        let node_1_pubkey = ValidatorPublicKey("bbb".into());

        bus.send(PeerEvent::NewPeer {
            name: "node-1".into(),
            pubkey: node_1_pubkey.clone(),
        })
        .expect("send");

        // Start the Genesis module
        let result = genesis.start().await;

        assert!(result.is_ok());

        let rec = bus.recv().await.expect("recv");
        assert_matches!(rec, GenesisEvent::GenesisBlock { .. });
        if let GenesisEvent::GenesisBlock {
            stake_txs,
            initial_validators,
        } = rec
        {
            assert_eq!(stake_txs.len(), 2);
            assert_eq!(initial_validators.len(), 2);
        }

        let rec = bus.recv().await.expect("recv");
        assert_matches!(rec, GenesisEvent::Ready { .. });
        if let GenesisEvent::Ready { first_round_leader } = rec {
            assert_eq!(first_round_leader, node_1_pubkey);
        }
    }
}
