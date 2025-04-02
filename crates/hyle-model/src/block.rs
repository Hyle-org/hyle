use std::{cmp::Ordering, collections::BTreeMap};

use anyhow::Context;
use anyhow::Result;
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
    pub block_timestamp: u128,
    pub txs: Vec<(TxId, Transaction)>,
    pub dp_parent_hashes: BTreeMap<TxHash, DataProposalHash>,
    pub lane_ids: BTreeMap<TxHash, LaneId>,
    pub successful_txs: Vec<TxHash>,
    pub failed_txs: Vec<TxHash>,
    pub timed_out_txs: Vec<TxHash>,
    pub blob_proof_outputs: Vec<HandledBlobProofOutput>,
    pub verified_blobs: Vec<(TxHash, BlobIndex, Option<usize>)>,
    pub new_bounded_validators: Vec<ValidatorPublicKey>,
    pub staking_actions: Vec<(Identity, StakingAction)>,
    pub registered_contracts: Vec<(TxHash, RegisterContractEffect)>,
    pub deleted_contracts: Vec<(TxHash, ContractName)>,
    pub updated_states: BTreeMap<ContractName, StateCommitment>,
    pub transactions_events: BTreeMap<TxHash, Vec<TransactionStateEvent>>,
}

impl Block {
    pub fn total_txs(&self) -> usize {
        self.txs.len()
    }

    pub fn resolve_parent_dp_hash(&self, tx_hash: &TxHash) -> Result<DataProposalHash> {
        Ok(self
            .dp_parent_hashes
            .get(tx_hash)
            .context(format!("No parent dp hash found for tx {}", tx_hash))?
            .clone())
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
    pub data_proposals: Vec<(LaneId, Vec<DataProposal>)>,
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
        for (_, _, txs) in self.iter_txs() {
            if !txs.is_empty() {
                return true;
            }
        }

        false
    }

    pub fn count_txs(&self) -> usize {
        self.iter_txs().map(|(_, _, txs)| txs.len()).sum()
    }

    pub fn iter_txs(&self) -> impl Iterator<Item = (LaneId, DataProposalHash, &Vec<Transaction>)> {
        self.data_proposals
            .iter()
            .flat_map(|(lane_id, dps)| std::iter::zip(std::iter::repeat(lane_id.clone()), dps))
            .map(|(lane_id, dp)| {
                (
                    lane_id.clone(),
                    dp.parent_data_proposal_hash
                        .clone()
                        // This is weird but has to match the workaround in own_lane.rs
                        .unwrap_or(DataProposalHash(lane_id.0.to_string())),
                    &dp.txs,
                )
            })
    }

    pub fn iter_txs_with_id(&self) -> impl Iterator<Item = (LaneId, TxId, &Transaction)> {
        self.iter_txs().flat_map(move |(lane_id, dp_hash, txs)| {
            txs.iter()
                .map(move |tx| (lane_id.clone(), TxId(dp_hash.clone(), tx.hashed()), tx))
        })
    }
}

impl Hashed<ConsensusProposalHash> for SignedBlock {
    fn hashed(&self) -> ConsensusProposalHash {
        self.consensus_proposal.hashed()
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
        self.hashed() == other.hashed()
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
