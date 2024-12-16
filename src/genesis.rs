use std::collections::BTreeMap;

use crate::{
    bus::{bus_client, BusClientSender, BusMessage},
    handle_messages,
    model::{
        BlobTransaction, Hashable, ProofData, RegisterContractTransaction, SharedRunContext,
        Transaction, TransactionData, ValidatorPublicKey, VerifiedProofTransaction,
    },
    p2p::network::PeerEvent,
    tools::transactions_builder::{BuildResult, States, TransactionBuilder},
    utils::{conf::SharedConf, crypto::SharedBlstCrypto, modules::Module},
};
use anyhow::{bail, Error, Result};
use hyle_contract_sdk::Digestable;
use hyle_contract_sdk::{identity_provider::IdentityVerification, Identity};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

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

        let genesis_txs = match self.generate_genesis_txs().await {
            Ok(t) => t,
            Err(e) => {
                error!("🌱 Genesis block generation failed: {:?}", e);
                return Err(e);
            }
        };

        // At this point, we can setup the genesis block.
        _ = self.bus.send(GenesisEvent::GenesisBlock {
            initial_validators,
            genesis_txs,
        });

        Ok(())
    }

    async fn generate_genesis_txs(&self) -> Result<Vec<Transaction>> {
        let (mut genesis_txs, mut states) = Self::genesis_contracts_txs();

        let register_txs = self.generate_register_txs(&mut states).await?;
        let faucet_txs = self.generate_faucet_txs(&mut states).await?;
        let stake_txs = self.generate_stake_txs(&mut states).await?;
        let delegate_txs = self.generate_delegate_txs(&mut states).await?;

        let builders = register_txs
            .into_iter()
            .chain(faucet_txs.into_iter())
            .chain(stake_txs.into_iter())
            .chain(delegate_txs.into_iter())
            .collect::<Vec<_>>();

        for BuildResult {
            identity,
            blobs,
            outputs,
        } in builders
        {
            // On genesis we don't need an actual zkproof as the txs are not going through data
            // dissemnitation. We can create the same VerifiedProofTransaction on each genesis
            // validator, and assume it's the same.

            let tx = BlobTransaction { identity, blobs };
            let blob_tx_hash = tx.hash();

            genesis_txs.push(Transaction::wrap(TransactionData::Blob(tx)));

            for (contract_name, out) in outputs {
                genesis_txs.push(Transaction::wrap(TransactionData::VerifiedProof(
                    VerifiedProofTransaction {
                        blob_tx_hash: blob_tx_hash.clone(),
                        contract_name,
                        proof_hash: ProofData::default().hash(),
                        hyle_output: out,
                        proof: None,
                    },
                )));
            }
        }

        Ok(genesis_txs)
    }

    pub async fn generate_register_txs(&self, states: &mut States) -> Result<Vec<BuildResult>> {
        // TODO: use an identity provider that checks BLST signature on a pubkey instead of
        // hydentity that checks password
        // The validator will send the signature for the register transaction in the handshake
        // in order to let all genesis validators to create the genesis register

        let mut txs = vec![];
        for peer in self.peer_pubkey.values() {
            info!("🌱  Registering identity {peer}");

            let identity = Identity(format!("{peer}.hydentity"));
            let mut transaction = TransactionBuilder::new(identity.clone());

            transaction.register_identity("password".to_string());
            txs.push(transaction.build(states).await?);
        }

        Ok(txs)
    }

    pub async fn generate_faucet_txs(&self, states: &mut States) -> Result<Vec<BuildResult>> {
        let genesis_faucet = 100;

        let mut txs = vec![];
        for peer in self.peer_pubkey.values() {
            info!("🌱  Fauceting {genesis_faucet} hyllar to {peer}");

            let identity = Identity("faucet.hydentity".to_string());
            let mut transaction = TransactionBuilder::new(identity.clone());

            transaction
                .verify_identity(&states.hydentity, "password".to_string())
                .await?;
            transaction.transfer("hyllar".into(), format!("{peer}.hydentity"), genesis_faucet);

            txs.push(transaction.build(states).await?);
        }

        Ok(txs)
    }

    pub async fn generate_stake_txs(&self, states: &mut States) -> Result<Vec<BuildResult>> {
        let genesis_stake = 100;

        let mut txs = vec![];
        for peer in self.peer_pubkey.values() {
            info!("🌱  Staking {genesis_stake} hyllar from {peer}");

            let identity = Identity(format!("{peer}.hydentity").to_string());
            let mut transaction = TransactionBuilder::new(identity.clone());

            transaction
                .verify_identity(&states.hydentity, "password".to_string())
                .await?;
            transaction.stake("hyllar".into(), "staking".into(), genesis_stake)?;

            txs.push(transaction.build(states).await?);
        }

        Ok(txs)
    }

    pub async fn generate_delegate_txs(&self, states: &mut States) -> Result<Vec<BuildResult>> {
        let mut txs = vec![];
        for peer in self.peer_pubkey.values().cloned() {
            info!("🌱  Delegating to {peer}");

            let identity = Identity(format!("{peer}.hydentity").to_string());
            let mut transaction = TransactionBuilder::new(identity.clone());

            transaction
                .verify_identity(&states.hydentity, "password".to_string())
                .await?;
            transaction.delegate(peer)?;

            txs.push(transaction.build(states).await?);
        }

        Ok(txs)
    }

    pub fn genesis_contracts_txs() -> (Vec<Transaction>, States) {
        let staking_program_id = hyle_contracts::STAKING_ID.to_vec();
        let hyllar_program_id = hyle_contracts::HYLLAR_ID.to_vec();
        let hydentity_program_id = hyle_contracts::HYDENTITY_ID.to_vec();

        let mut hydentity_state = hydentity::Hydentity::new();
        hydentity_state
            .register_identity("faucet.hydentity", "password")
            .unwrap();

        let staking_state = staking::state::Staking::new();

        let states = States {
            hyllar: hyllar::HyllarToken::new(100_000_000_000, "faucet.hydentity".to_string()),
            hydentity: hydentity_state,
            staking: staking_state,
        };

        (
            vec![
                Transaction::wrap(TransactionData::RegisterContract(
                    RegisterContractTransaction {
                        owner: "hyle".into(),
                        verifier: "risc0".into(),
                        program_id: staking_program_id.into(),
                        state_digest: states.staking.on_chain_state().as_digest(),
                        contract_name: "staking".into(),
                    },
                )),
                Transaction::wrap(TransactionData::RegisterContract(
                    RegisterContractTransaction {
                        owner: "hyle".into(),
                        verifier: "risc0".into(),
                        program_id: hyllar_program_id.into(),
                        state_digest: states.hyllar.as_digest(),
                        contract_name: "hyllar".into(),
                    },
                )),
                Transaction::wrap(TransactionData::RegisterContract(
                    RegisterContractTransaction {
                        owner: "hyle".into(),
                        verifier: "risc0".into(),
                        program_id: hydentity_program_id.into(),
                        state_digest: states.hydentity.as_digest(),
                        contract_name: "hydentity".into(),
                    },
                )),
            ],
            states,
        )
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
                pubkey: BlstCrypto::new("node-2".into()).validator_pubkey().clone(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-3".into(),
                pubkey: BlstCrypto::new("node-3".into()).validator_pubkey().clone(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-4".into(),
                pubkey: BlstCrypto::new("node-4".into()).validator_pubkey().clone(),
            })
            .expect("send");
            let _ = genesis.start().await;
            bus.try_recv().expect("recv")
        };
        let rec2 = {
            let (mut genesis, mut bus) = new(config).await;
            bus.send(PeerEvent::NewPeer {
                name: "node-4".into(),
                pubkey: BlstCrypto::new("node-4".into()).validator_pubkey().clone(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-2".into(),
                pubkey: BlstCrypto::new("node-2".into()).validator_pubkey().clone(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-3".into(),
                pubkey: BlstCrypto::new("node-3".into()).validator_pubkey().clone(),
            })
            .expect("send");
            let _ = genesis.start().await;
            bus.try_recv().expect("recv")
        };

        assert_eq!(rec1, rec2);
    }
}
