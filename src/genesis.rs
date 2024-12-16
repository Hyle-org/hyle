use std::collections::BTreeMap;

use crate::{
    bus::{bus_client, BusClientSender, BusMessage},
    handle_messages,
    model::{
        RegisterContractTransaction, SharedRunContext, Transaction, TransactionData,
        ValidatorPublicKey,
    },
    p2p::network::PeerEvent,
    utils::{conf::SharedConf, crypto::SharedBlstCrypto, modules::Module},
};
use anyhow::{bail, Error, Result};
use hyle_contract_sdk::identity_provider::IdentityVerification;
use hyle_contract_sdk::Digestable;
use serde::{Deserialize, Serialize};
use staking::{Stake, Staker};
use tracing::info;

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub enum GenesisEvent {
    NoGenesis,
    GenesisBlock {
        genesis_txs: Vec<Transaction>,
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
    peer_pubkey: BTreeMap<String, ValidatorPublicKey>,
    crypto: SharedBlstCrypto,
}

impl Module for Genesis {
    type Context = SharedRunContext;
    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = GenesisBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        Ok(Genesis {
            config: ctx.common.config.clone(),
            bus,
            peer_pubkey: BTreeMap::new(),
            crypto: ctx.node.crypto.clone(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl Genesis {
    pub async fn start(&mut self) -> Result<(), Error> {
        if self.config.single_node.unwrap_or(false) {
            bail!("Single node mode, genesis module should not be enabled.");
        }
        if !self
            .config
            .consensus
            .genesis_stakers
            .contains_key(&self.config.id)
        {
            info!("📡 Not a genesis staker, need to catchup from peers.");
            _ = self.bus.send(GenesisEvent::NoGenesis {});
            return Ok(());
        }

        info!("🌱 Building genesis block");

        // We will start from the genesis block.
        self.peer_pubkey.insert(
            self.config.id.clone(),
            self.crypto.validator_pubkey().clone(),
        );

        // Wait until we've connected with all other genesis peers.
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

        let mut initial_validators = self.peer_pubkey.values().cloned().collect::<Vec<_>>();
        initial_validators.sort();

        info!("Genesis validators {:?}", initial_validators.clone());

        let stake_txs = self
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
            .collect::<Vec<_>>();

        let contracts_txs = Self::genesis_contracts_txs();

        let genesis_txs = stake_txs
            .into_iter()
            .chain(contracts_txs.into_iter())
            .collect();

        // At this point, we can setup the genesis block.
        _ = self.bus.send(GenesisEvent::GenesisBlock {
            initial_validators,
            genesis_txs,
        });

        Ok(())
    }

    pub fn genesis_contracts_txs() -> Vec<Transaction> {
        let hyllar_program_id = hyle_contracts::HYLLAR_ID.to_vec();
        let hydentity_program_id = hyle_contracts::HYDENTITY_ID.to_vec();

        let mut hydentity_state = hydentity::Hydentity::new();
        hydentity_state
            .register_identity("faucet.hydentity", "password")
            .unwrap();

        vec![
            Transaction::wrap(TransactionData::RegisterContract(
                RegisterContractTransaction {
                    owner: "hyle".into(),
                    verifier: "risc0".into(),
                    program_id: hyllar_program_id.clone().into(),
                    state_digest: hyllar::HyllarToken::new(
                        100_000_000_000,
                        "faucet.hydentity".to_string(),
                    )
                    .as_digest(),
                    contract_name: "hyllar".into(),
                },
            )),
            Transaction::wrap(TransactionData::RegisterContract(
                RegisterContractTransaction {
                    owner: "hyle".into(),
                    verifier: "risc0".into(),
                    program_id: hydentity_program_id.into(),
                    state_digest: hydentity_state.as_digest(),
                    contract_name: "hydentity".into(),
                },
            )),
            Transaction::wrap(TransactionData::RegisterContract(
                RegisterContractTransaction {
                    owner: "hyle".into(),
                    verifier: "risc0".into(),
                    program_id: hyle_contracts::RISC0_RECURSION_ID.to_vec().into(),
                    state_digest: hyle_contract_sdk::StateDigest(vec![]),
                    contract_name: "risc0-recursion".into(),
                },
            )),
        ]
    }
}

#[cfg(test)]
mod tests {
    use assertables::assert_matches;

    use super::*;
    use crate::bus::{BusClientReceiver, SharedMessageBus};
    use crate::utils::conf::Conf;
    use crate::utils::crypto::BlstCrypto;
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
        let crypto = Arc::new(BlstCrypto::new(config.id.clone()));
        (
            Genesis {
                config: Arc::new(config),
                bus,
                peer_pubkey: BTreeMap::new(),
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
            single_node: Some(true),
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
        assert!(result.is_err());
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

        let rec: GenesisEvent = bus.try_recv().expect("recv");
        assert_matches!(rec, GenesisEvent::GenesisBlock { .. });
        if let GenesisEvent::GenesisBlock {
            genesis_txs,
            initial_validators,
        } = rec
        {
            assert!(!genesis_txs.is_empty());
            assert_eq!(initial_validators.len(), 2);
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

        let rec = bus.try_recv().expect("recv");
        assert_matches!(rec, GenesisEvent::GenesisBlock { .. });
        if let GenesisEvent::GenesisBlock {
            genesis_txs,
            initial_validators,
        } = rec
        {
            assert!(!genesis_txs.is_empty());
            assert_eq!(initial_validators.len(), 2);
        }
    }

    // test that the order of nodes connecting doesn't matter on genesis block creation
    #[test_log::test(tokio::test)]
    async fn test_genesis_connect_order() {
        let config = Conf {
            id: "node-1".to_string(),
            consensus: crate::utils::conf::Consensus {
                genesis_stakers: vec![
                    ("node-1".into(), 100),
                    ("node-2".into(), 100),
                    ("node-3".into(), 100),
                    ("node-4".into(), 100),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let rec1 = {
            let (mut genesis, mut bus) = new(config.clone()).await;
            bus.send(PeerEvent::NewPeer {
                name: "node-2".into(),
                pubkey: ValidatorPublicKey("node-2".into()).clone(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-3".into(),
                pubkey: ValidatorPublicKey("node-3".into()).clone(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-4".into(),
                pubkey: ValidatorPublicKey("node-4".into()).clone(),
            })
            .expect("send");
            let _ = genesis.start().await;
            bus.try_recv().expect("recv")
        };
        let rec2 = {
            let (mut genesis, mut bus) = new(config).await;
            bus.send(PeerEvent::NewPeer {
                name: "node-4".into(),
                pubkey: ValidatorPublicKey("node-4".into()).clone(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-2".into(),
                pubkey: ValidatorPublicKey("node-2".into()).clone(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-3".into(),
                pubkey: ValidatorPublicKey("node-3".into()).clone(),
            })
            .expect("send");
            let _ = genesis.start().await;
            bus.try_recv().expect("recv")
        };

        assert_eq!(rec1, rec2);
    }
}
