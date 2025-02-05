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
    pub txs: Vec<Transaction>,
    pub successful_txs: Vec<TxHash>,
    pub failed_txs: Vec<TxHash>,
    pub timed_out_txs: Vec<TxHash>,
    pub blob_proof_outputs: Vec<HandledBlobProofOutput>,
    pub verified_blobs: Vec<(TxHash, BlobIndex, Option<usize>)>,
    pub new_bounded_validators: Vec<ValidatorPublicKey>,
    pub staking_actions: Vec<(Identity, StakingAction)>,
    pub registered_contracts: Vec<(TxHash, RegisterContractEffect)>,
    pub updated_states: BTreeMap<ContractName, StateDigest>,
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

    pub fn txs(&self) -> Vec<Transaction> {
        self.data_proposals
            .iter()
            .flat_map(|(_, dps)| dps)
            .flat_map(|dp| dp.txs.clone())
            .collect()
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
