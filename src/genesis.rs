use std::collections::{BTreeMap, HashMap};

use crate::{
    bus::{bus_client, BusClientSender, BusMessage},
    consensus::{
        ConsensusProposal, ConsensusProposalHash, NewValidatorCandidate, ValidatorCandidacy,
    },
    handle_messages,
    mempool::DataProposal,
    model::{
        BlobProofOutput, BlobTransaction, Hashable, ProofData, RegisterContractTransaction,
        SharedRunContext, SignedBlock, Transaction, TransactionData, ValidatorPublicKey,
        VerifiedProofTransaction,
    },
    p2p::network::PeerEvent,
    utils::{
        conf::SharedConf,
        crypto::{
            AggregateSignature, SharedBlstCrypto, Signature, SignedByValidator, ValidatorSignature,
        },
        modules::Module,
    },
};
use anyhow::{bail, Error, Result};
use client_sdk::transaction_builder::{BuildResult, StateUpdater, TransactionBuilder};
use hydentity::Hydentity;
use hyle_contract_sdk::{identity_provider::IdentityVerification, Identity};
use hyle_contract_sdk::{ContractName, Digestable, ProgramId};
use hyllar::HyllarToken;
use serde::{Deserialize, Serialize};
use staking::state::Staking;
use tracing::{error, info};

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub enum GenesisEvent {
    NoGenesis,
    GenesisBlock(SignedBlock),
}
impl BusMessage for GenesisEvent {}

bus_client! {
struct GenesisBusClient {
    sender(GenesisEvent),
    receiver(PeerEvent),
}
}

type PeerPublicKeyMap = BTreeMap<String, ValidatorPublicKey>;

pub struct Genesis {
    config: SharedConf,
    bus: GenesisBusClient,
    peer_pubkey: PeerPublicKeyMap,
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

#[derive(Debug, Clone)]
pub struct States {
    pub hyllar: HyllarToken,
    pub hydentity: Hydentity,
    pub staking: Staking,
}

impl StateUpdater for States {
    fn update(
        &mut self,
        contract_name: &hyle_contract_sdk::ContractName,
        new_state: hyle_contract_sdk::StateDigest,
    ) -> Result<()> {
        match contract_name.0.as_str() {
            "hyllar" => self.hyllar = new_state.try_into()?,
            "hydentity" => self.hydentity = new_state.try_into()?,
            "staking" => self.staking = new_state.try_into()?,
            _ => bail!("Unknown contract name"),
        }
        Ok(())
    }

    fn get(&self, contract_name: &ContractName) -> Result<hyle_contract_sdk::StateDigest> {
        Ok(match contract_name.0.as_str() {
            "hyllar" => self.hyllar.as_digest(),
            "hydentity" => self.hydentity.as_digest(),
            "staking" => self.staking.as_digest(),
            _ => bail!("Unknown contract name"),
        })
    }
}

impl Genesis {
    pub async fn start(&mut self) -> Result<(), Error> {
        let file = self.config.data_directory.clone().join("genesis.bin");
        let already_handled_genesis: bool = Self::load_from_disk_or_default(&file);
        if already_handled_genesis {
            info!("🌿 Genesis block already handled, skipping");
            // TODO: do we need a different message?
            _ = self.bus.send(GenesisEvent::NoGenesis {})?;
            return Ok(());
        }

        self.do_genesis().await?;

        // TODO: ideally we'd wait until everyone has processed it, as there's technically a data race.

        Self::save_on_disk(&file, &true)?;

        Ok(())
    }

    pub async fn do_genesis(&mut self) -> Result<()> {
        let single_node = self.config.single_node.unwrap_or(false);
        // Unless we're in single node mode, we must be a genesis staker to start the network.
        if !single_node
            && !self
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
        if !single_node {
            info!("🌱 Waiting on other genesis peers to join");
            handle_messages! {
                on_bus self.bus,
                listen<PeerEvent> msg => {
                    match msg {
                        PeerEvent::NewPeer { name, pubkey, .. } => {
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

        let genesis_txs = match Self::generate_genesis_txs(
            &self.peer_pubkey,
            &self.config.consensus.genesis_stakers,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                error!("🌱 Genesis block generation failed: {:?}", e);
                return Err(e);
            }
        };

        let signed_block = self.make_genesis_block(genesis_txs, initial_validators);

        // At this point, we can setup the genesis block.
        _ = self.bus.send(GenesisEvent::GenesisBlock(signed_block));

        Ok(())
    }

    pub async fn generate_genesis_txs(
        peer_pubkey: &PeerPublicKeyMap,
        genesis_stake: &HashMap<String, u64>,
    ) -> Result<Vec<Transaction>> {
        let (contract_program_ids, mut genesis_txs, mut states) = Self::genesis_contracts_txs();

        let register_txs = Self::generate_register_txs(peer_pubkey, &mut states).await?;
        let faucet_txs = Self::generate_faucet_txs(peer_pubkey, &mut states, genesis_stake).await?;
        let stake_txs = Self::generate_stake_txs(peer_pubkey, &mut states, genesis_stake).await?;

        let builders = register_txs
            .into_iter()
            .chain(faucet_txs.into_iter())
            .chain(stake_txs.into_iter())
            .collect::<Vec<_>>();

        for BuildResult {
            identity,
            blobs,
            mut outputs,
        } in builders
        {
            // On genesis we don't need an actual zkproof as the txs are not going through data
            // dissemination. We can create the same VerifiedProofTransaction on each genesis
            // validator, and assume it's the same.

            let tx = BlobTransaction { identity, blobs };
            let blob_tx_hash = tx.hash();

            genesis_txs.push(Transaction::wrap(TransactionData::Blob(tx)));

            // Pretend we're verifying a recursive proof
            genesis_txs.push(Transaction::wrap(TransactionData::VerifiedProof(
                VerifiedProofTransaction {
                    contract_name: "risc0-recursion".into(),
                    proven_blobs: outputs
                        .drain(..)
                        .map(|(contract_name, out)| BlobProofOutput {
                            original_proof_hash: ProofData::default().hash(),
                            program_id: contract_program_ids
                                .get(&contract_name)
                                .expect("Genesis TXes on unregistered contracts")
                                .clone(),
                            blob_tx_hash: blob_tx_hash.clone(),
                            hyle_output: out,
                        })
                        .collect(),
                    is_recursive: true,
                    proof_hash: ProofData::default().hash(),
                    proof: None,
                },
            )));
        }

        Ok(genesis_txs)
    }

    async fn generate_register_txs(
        peer_pubkey: &PeerPublicKeyMap,
        states: &mut States,
    ) -> Result<Vec<BuildResult>> {
        // TODO: use an identity provider that checks BLST signature on a pubkey instead of
        // hydentity that checks password
        // The validator will send the signature for the register transaction in the handshake
        // in order to let all genesis validators to create the genesis register

        let mut txs = vec![];
        for peer in peer_pubkey.values() {
            info!("🌱  Registering identity {peer}");

            let identity = Identity(format!("{peer}.hydentity"));
            let mut transaction = TransactionBuilder::new(identity.clone());

            // Register
            states
                .hydentity
                .default_builder(&mut transaction)
                .register_identity("password".to_string())?;

            txs.push(transaction.build(states)?);
        }

        Ok(txs)
    }

    async fn generate_faucet_txs(
        peer_pubkey: &PeerPublicKeyMap,
        states: &mut States,
        genesis_stakers: &HashMap<String, u64>,
    ) -> Result<Vec<BuildResult>> {
        let mut txs = vec![];
        for (id, peer) in peer_pubkey.iter() {
            let genesis_faucet = *genesis_stakers
                .get(id)
                .expect("Genesis stakers should be in the peer map")
                as u128;

            info!("🌱  Fauceting {genesis_faucet} hyllar to {peer}");

            let identity = Identity("faucet.hydentity".to_string());
            let mut transaction = TransactionBuilder::new(identity.clone());

            // Verify identity
            states
                .hydentity
                .default_builder(&mut transaction)
                .verify_identity(&states.hydentity, "password".to_string())?;

            // Transfer
            states
                .hyllar
                .default_builder(&mut transaction)
                .transfer(format!("{peer}.hydentity"), genesis_faucet)?;

            txs.push(transaction.build(states)?);
        }

        Ok(txs)
    }

    async fn generate_stake_txs(
        peer_pubkey: &PeerPublicKeyMap,
        states: &mut States,
        genesis_stakers: &HashMap<String, u64>,
    ) -> Result<Vec<BuildResult>> {
        let mut txs = vec![];
        for (id, peer) in peer_pubkey.iter() {
            let genesis_stake = *genesis_stakers
                .get(id)
                .expect("Genesis stakers should be in the peer map")
                as u128;

            info!("🌱  Staking {genesis_stake} hyllar from {peer}");

            let identity = Identity(format!("{peer}.hydentity").to_string());
            let mut transaction = TransactionBuilder::new(identity.clone());

            // Verify identity
            states
                .hydentity
                .default_builder(&mut transaction)
                .verify_identity(&states.hydentity, "password".to_string())?;

            // Stake
            states
                .staking
                .builder(&mut transaction)
                .stake(genesis_stake)?;

            // Transfer
            states
                .hyllar
                .default_builder(&mut transaction)
                .transfer("staking".to_string(), genesis_stake)?;

            // Delegate
            states
                .staking
                .builder(&mut transaction)
                .delegate(peer.clone())?;

            txs.push(transaction.build(states)?);
        }

        Ok(txs)
    }

    fn genesis_contracts_txs() -> (BTreeMap<ContractName, ProgramId>, Vec<Transaction>, States) {
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

        let mut map = BTreeMap::default();
        map.insert("hyllar".into(), ProgramId(hyllar_program_id.clone()));
        map.insert("hydentity".into(), ProgramId(hydentity_program_id.clone()));
        map.insert("staking".into(), ProgramId(staking_program_id.clone()));
        map.insert(
            "risc0-recursion".into(),
            ProgramId(hyle_contracts::RISC0_RECURSION_ID.to_vec()),
        );

        (
            map,
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
                Transaction::wrap(TransactionData::RegisterContract(
                    RegisterContractTransaction {
                        owner: "hyle".into(),
                        verifier: "risc0".into(),
                        program_id: hyle_contracts::RISC0_RECURSION_ID.to_vec().into(),
                        state_digest: hyle_contract_sdk::StateDigest(vec![]),
                        contract_name: "risc0-recursion".into(),
                    },
                )),
            ],
            states,
        )
    }

    fn make_genesis_block(
        &self,
        genesis_txs: Vec<Transaction>,
        initial_validators: Vec<ValidatorPublicKey>,
    ) -> SignedBlock {
        let dp = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: genesis_txs,
        };

        // TODO: do something better?
        let round_leader = initial_validators.first().unwrap().clone();

        SignedBlock {
            data_proposals: vec![(round_leader.clone(), vec![dp.clone()])],
            certificate: AggregateSignature {
                signature: Signature("fake".into()),
                validators: initial_validators.clone(),
            },
            consensus_proposal: ConsensusProposal {
                slot: 0,
                view: 0,
                round_leader: round_leader.clone(),
                // TODO: genesis block should have a consistent, up-to-date timestamp
                timestamp: 1735689600000, // 1st of Jan 25 for now
                // TODO: We aren't actually storing the data proposal above, so we cannot store it here,
                // or we might mistakenly request data from that cut, but mempool hasn't seen it.
                // This should be fixed by storing the data proposal in mempool or handling this whole thing differently.
                cut: vec![/*(
                    round_leader.clone(), dp.hash(), AggregateSignature {
                        signature: Signature("fake".into()),
                        validators: initial_validators.clone()
                    }
                )*/],
                new_validators_to_bond: initial_validators
                    .iter()
                    .map(|v| NewValidatorCandidate {
                        pubkey: v.clone(),
                        msg: SignedByValidator {
                            msg: crate::consensus::ConsensusNetMessage::ValidatorCandidacy(
                                ValidatorCandidacy {
                                    pubkey: v.clone(),
                                    peer_address: "".into(),
                                },
                            ),
                            signature: ValidatorSignature {
                                signature: Signature("".into()),
                                validator: v.clone(),
                            },
                        },
                    })
                    .collect(),
                parent_hash: ConsensusProposalHash("genesis".into()),
            },
        }
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
        let tmpdir = tempfile::Builder::new().tempdir().unwrap();
        let config = Conf {
            id: "node-4".to_string(),
            data_directory: tmpdir.path().to_path_buf(),
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
        let tmpdir = tempfile::Builder::new().tempdir().unwrap();
        let config = Conf {
            id: "single-node".to_string(),
            single_node: Some(true),
            data_directory: tmpdir.path().to_path_buf(),
            consensus: crate::utils::conf::Consensus {
                genesis_stakers: vec![("single-node".into(), 100)].into_iter().collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let (mut genesis, mut bus) = new(config).await;

        // Start the Genesis module
        let result = genesis.start().await;

        assert!(result.is_ok());

        // Check it ran the genesis block
        let rec: GenesisEvent = bus.try_recv().expect("recv");
        assert_matches!(rec, GenesisEvent::GenesisBlock(..));
        if let GenesisEvent::GenesisBlock(signed_block) = rec {
            assert!(!signed_block.txs().is_empty());
            assert_eq!(
                signed_block.consensus_proposal.new_validators_to_bond.len(),
                1
            );
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_genesis_as_leader() {
        let tmpdir = tempfile::Builder::new().tempdir().unwrap();
        let config = Conf {
            id: "node-1".to_string(),
            data_directory: tmpdir.path().to_path_buf(),
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
            da_address: "".into(),
        })
        .expect("send");

        // Start the Genesis module
        let result = genesis.start().await;

        assert!(result.is_ok());

        let rec: GenesisEvent = bus.try_recv().expect("recv");
        assert_matches!(rec, GenesisEvent::GenesisBlock(..));
        if let GenesisEvent::GenesisBlock(signed_block) = rec {
            assert!(!signed_block.txs().is_empty());
            assert_eq!(
                signed_block.consensus_proposal.new_validators_to_bond.len(),
                2
            );
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_genesis_as_follower() {
        let tmpdir = tempfile::Builder::new().tempdir().unwrap();
        let config = Conf {
            id: "node-2".to_string(),
            data_directory: tmpdir.path().to_path_buf(),
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
            da_address: "".into(),
        })
        .expect("send");

        // Start the Genesis module
        let result = genesis.start().await;

        assert!(result.is_ok());

        let rec = bus.try_recv().expect("recv");
        assert_matches!(rec, GenesisEvent::GenesisBlock(..));
        if let GenesisEvent::GenesisBlock(signed_block) = rec {
            assert!(!signed_block.txs().is_empty());
            assert_eq!(
                signed_block.consensus_proposal.new_validators_to_bond.len(),
                2
            );
        }
    }

    // test that the order of nodes connecting doesn't matter on genesis block creation
    #[test_log::test(tokio::test)]
    async fn test_genesis_connect_order() {
        let tmpdir = tempfile::Builder::new().tempdir().unwrap();
        let mut config = Conf {
            id: "node-1".to_string(),
            data_directory: tmpdir.path().to_path_buf(),
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
                da_address: "".into(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-3".into(),
                pubkey: BlstCrypto::new("node-3".into()).validator_pubkey().clone(),
                da_address: "".into(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-4".into(),
                pubkey: BlstCrypto::new("node-4".into()).validator_pubkey().clone(),
                da_address: "".into(),
            })
            .expect("send");
            let _ = genesis.start().await;
            bus.try_recv().expect("recv")
        };
        let tmpdir = tempfile::Builder::new().tempdir().unwrap();
        config.data_directory = tmpdir.path().to_path_buf();
        let rec2 = {
            let (mut genesis, mut bus) = new(config).await;
            bus.send(PeerEvent::NewPeer {
                name: "node-4".into(),
                pubkey: BlstCrypto::new("node-4".into()).validator_pubkey().clone(),
                da_address: "".into(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-2".into(),
                pubkey: BlstCrypto::new("node-2".into()).validator_pubkey().clone(),
                da_address: "".into(),
            })
            .expect("send");
            bus.send(PeerEvent::NewPeer {
                name: "node-3".into(),
                pubkey: BlstCrypto::new("node-3".into()).validator_pubkey().clone(),
                da_address: "".into(),
            })
            .expect("send");
            let _ = genesis.start().await;
            bus.try_recv().expect("recv")
        };

        assert_eq!(rec1, rec2);
    }
}
