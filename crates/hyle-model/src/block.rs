use std::{cmp::Ordering, collections::BTreeMap};

use borsh::{BorshDeserialize, BorshSerialize};
use derive_more::derive::Display;
use serde::{Deserialize, Serialize};

use crate::{staking::*, *};

#[derive(
    Debug, Default, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize, Eq, PartialEq,
)]
pub struct Block {
    pub parent_hash: ConsensusProposalHash,
    pub hash: ConsensusProposalHash,
    pub block_height: BlockHeight,
    pub block_timestamp: u64,
    pub txs: Vec<(TxId, Transaction)>,
    pub dp_hashes: BTreeMap<TxHash, DataProposalHash>,
    pub successful_txs: Vec<TxHash>,
    pub failed_txs: Vec<TxHash>,
    pub timed_out_txs: Vec<TxHash>,
    pub blob_proof_outputs: Vec<HandledBlobProofOutput>,
    pub verified_blobs: Vec<(TxHash, BlobIndex, Option<usize>)>,
    pub new_bounded_validators: Vec<ValidatorPublicKey>,
    pub staking_actions: Vec<(Identity, StakingAction)>,
    pub registered_contracts: Vec<(TxHash, RegisterContractEffect)>,
    pub updated_states: BTreeMap<ContractName, StateDigest>,
    pub transactions_events: BTreeMap<TxHash, Vec<TransactionStateEvent>>,
}

impl Block {
    pub fn total_txs(&self) -> usize {
        self.txs.len()
    }
}

impl Ord for Block {
    fn cmp(&self, other: &Self) -> Ordering {
        self.block_height.0.cmp(&other.block_height.0)
    }
}

impl PartialOrd for Block {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Display)]
#[display("")]
pub struct SignedBlock {
    pub data_proposals: Vec<(ValidatorPublicKey, Vec<DataProposal>)>,
    pub certificate: AggregateSignature,
    pub consensus_proposal: ConsensusProposal,
}

impl SignedBlock {
    pub fn parent_hash(&self) -> &ConsensusProposalHash {
        &self.consensus_proposal.parent_hash
    }

    pub fn height(&self) -> BlockHeight {
        BlockHeight(self.consensus_proposal.slot)
    }

    pub fn has_txs(&self) -> bool {
        for (_, txs) in self.iter_txs() {
            if !txs.is_empty() {
                return true;
            }
        }

        false
    }

    pub fn count_txs(&self) -> usize {
        self.iter_txs().map(|(_, txs)| txs.len()).sum()
    }

    pub fn iter_txs(&self) -> impl Iterator<Item = (DataProposalHash, &Vec<Transaction>)> {
        self.data_proposals
            .iter()
            .flat_map(|(_pub_key, dps)| dps)
            .map(|dp| {
                (
                    dp.parent_data_proposal_hash
                        .clone()
                        .unwrap_or(DataProposalHash("".to_string())),
                    &dp.txs,
                )
            })
    }

    pub fn iter_txs_with_id(&self) -> impl Iterator<Item = (TxId, &Transaction)> {
        self.iter_txs().flat_map(move |(dp_hash, txs)| {
            txs.iter()
                .map(move |tx| (TxId(dp_hash.clone(), tx.hash()), tx))
        })
    }

    pub fn iter_clone_txs_with_id(&self) -> impl Iterator<Item = (TxId, Transaction)> + '_ {
        self.iter_txs_with_id()
            .map(|(tx_id, tx)| (tx_id, tx.clone()))
    }
}
impl Hashable<ConsensusProposalHash> for SignedBlock {
    fn hash(&self) -> ConsensusProposalHash {
        self.consensus_proposal.hash()
    }
}

impl Ord for SignedBlock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.height().0.cmp(&other.height().0)
    }
}

impl PartialOrd for SignedBlock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for SignedBlock {
    fn eq(&self, other: &Self) -> bool {
        self.hash() == other.hash()
    }
}

impl Eq for SignedBlock {}

impl std::default::Default for SignedBlock {
    fn default() -> Self {
        SignedBlock {
            consensus_proposal: ConsensusProposal::default(),
            data_proposals: vec![],
            certificate: AggregateSignature {
                signature: crate::Signature("signature".into()),
                validators: vec![],
            },
        }
    }
}
