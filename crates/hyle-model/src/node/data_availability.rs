use std::{hash::Hash, sync::Arc};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use utoipa::ToSchema;

use crate::*;

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, ToSchema, BorshSerialize, BorshDeserialize,
)]
pub struct Contract {
    pub name: ContractName,
    pub program_id: ProgramId,
    pub state: StateDigest,
    pub verifier: Verifier,
}

#[derive(
    Default,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    serde::Serialize,
    serde::Deserialize,
    ToSchema,
)]
pub struct UnsettledBlobTransaction {
    pub identity: Identity,
    pub hash: TxHash,
    #[schema(value_type=TxContext)]
    pub tx_context: Arc<TxContext>,
    pub blobs_hash: BlobsHash,
    pub blobs: Vec<UnsettledBlobMetadata>,
}

#[derive(
    Default,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    serde::Serialize,
    serde::Deserialize,
    ToSchema,
)]
pub struct UnsettledBlobMetadata {
    pub blob: Blob,
    // Each time we receive a proof, we add it to this list
    pub possible_proofs: Vec<(ProgramId, HyleOutput)>,
}

#[derive(
    Debug,
    Default,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    BorshSerialize,
    BorshDeserialize,
    Eq,
    PartialEq,
)]
pub struct HandledBlobProofOutput {
    pub proof_tx_hash: TxHash,
    pub blob_tx_hash: TxHash,
    pub blob_index: BlobIndex,
    pub contract_name: ContractName,
    pub hyle_output: HyleOutput,
    pub blob_proof_output_index: usize,
}

#[derive(
    Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize,
)]
pub struct BlobProofOutput {
    // TODO: this can be recovered from the hyle output
    pub blob_tx_hash: TxHash,
    // TODO: remove this?
    pub original_proof_hash: ProofDataHash,

    /// HyleOutput of the proof for this blob
    pub hyle_output: HyleOutput,
    /// Program ID used to verify the proof.
    pub program_id: ProgramId,
}

pub struct BlobProofOutputHash(pub Vec<u8>);

impl Hashable<BlobProofOutputHash> for BlobProofOutput {
    fn hash(&self) -> BlobProofOutputHash {
        let mut hasher = Sha3_256::new();
        // blob_tx_id.0 == data_proposal hash is not taken into account
        hasher.update(self.blob_tx_hash.0.as_bytes());
        hasher.update(self.original_proof_hash.0.as_bytes());
        hasher.update(self.program_id.0.clone());
        hasher.update(contract::Hashable::hash(&self.hyle_output).0);
        BlobProofOutputHash(hasher.finalize().to_vec())
    }
}

pub struct HyleOutputHash(pub Vec<u8>);
impl Hashable<HyleOutputHash> for HyleOutput {
    fn hash(&self) -> HyleOutputHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.version.to_le_bytes());
        hasher.update(self.initial_state.0.clone());
        hasher.update(self.next_state.0.clone());
        hasher.update(self.identity.0.as_bytes());
        hasher.update(self.index.0.to_le_bytes());
        hasher.update(&self.blobs);
        hasher.update([self.success as u8]);
        hasher.update(self.registered_contracts.len().to_le_bytes());
        self.registered_contracts
            .iter()
            .for_each(|c| hasher.update(contract::Hashable::hash(c).0));
        hasher.update(&self.program_outputs);
        HyleOutputHash(hasher.finalize().to_vec())
    }
}

#[derive(
    Debug, Clone, Serialize, Deserialize, ToSchema, BorshSerialize, BorshDeserialize, Eq, PartialEq,
)]
#[serde(tag = "name", content = "metadata")]
pub enum TransactionStateEvent {
    Sequenced,
    Error(String),
    NewProof {
        blob_index: BlobIndex,
        proof_tx_hash: TxHash,
    },
    SettleEvent(String),
    Settled,
    SettledAsFailed,
    TimedOut,
}
