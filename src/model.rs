//! Various data structures

#[cfg(feature = "node")]
use crate::bus::SharedMessageBus;
#[cfg(feature = "node")]
use crate::utils::{conf::SharedConf, crypto::SharedBlstCrypto};
use anyhow::Context;
#[cfg(feature = "node")]
use axum::Router;
use data_availability::HandledBlobProofOutput;
#[cfg(feature = "node")]
use std::sync::Arc;

use base64::prelude::*;
use bincode::{Decode, Encode};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt,
    io::Write,
    ops::Add,
    time::{SystemTime, UNIX_EPOCH},
};
use strum_macros::IntoStaticStr;

use consensus::{ConsensusProposal, ConsensusProposalHash};
use crypto::AggregateSignature;
use hyle_contract_sdk::{
    flatten_blobs, BlobIndex, HyleOutput, Identity, ProgramId, StateDigest, TxHash, Verifier,
};
use mempool::DataProposal;
use staking::StakingAction;
use tracing::debug;

// Re-export
pub use hyle_contract_sdk::{Blob, BlobData, ContractName};
pub use staking::model::ValidatorPublicKey;

pub mod consensus;
pub mod crypto;
pub mod data_availability;
pub mod indexer;
pub mod mempool;
pub mod rest;

pub const HASH_DISPLAY_SIZE: usize = 3;

#[derive(
    Debug, Display, Default, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Encode, Decode,
)]
pub struct BlobsHash(pub String);

impl BlobsHash {
    pub fn new(s: &str) -> BlobsHash {
        BlobsHash(s.into())
    }

    pub fn from_vec(vec: &Vec<Blob>) -> BlobsHash {
        debug!("From vec {:?}", vec);
        Self::from_concatenated(&flatten_blobs(vec))
    }

    pub fn from_concatenated(vec: &Vec<u8>) -> BlobsHash {
        debug!("From concatenated {:?}", vec);
        let mut hasher = Sha3_256::new();
        hasher.update(vec.as_slice());
        let hash_bytes = hasher.finalize();
        BlobsHash(hex::encode(hash_bytes))
    }
}

#[derive(
    Default,
    Debug,
    Clone,
    Serialize,
    Deserialize,
    Eq,
    PartialEq,
    Hash,
    Display,
    Copy,
    Encode,
    Decode,
)]
pub struct BlockHeight(pub u64);

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode)]
pub struct Transaction {
    pub version: u32,
    pub transaction_data: TransactionData,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode, IntoStaticStr)]
pub enum TransactionData {
    Blob(BlobTransaction),
    Proof(ProofTransaction),
    VerifiedProof(VerifiedProofTransaction),
    RegisterContract(RegisterContractTransaction),
}

impl Default for TransactionData {
    fn default() -> Self {
        TransactionData::Blob(BlobTransaction::default())
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Encode, Decode)]
#[serde(untagged)]
pub enum ProofData {
    Base64(String),
    Bytes(Vec<u8>),
}
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode)]
pub struct ProofDataHash(pub String);

impl Default for ProofData {
    fn default() -> Self {
        ProofData::Bytes(Vec::new())
    }
}

impl ProofData {
    pub fn to_bytes(&self) -> Result<Vec<u8>, base64::DecodeError> {
        match self {
            ProofData::Base64(s) => BASE64_STANDARD.decode(s),
            ProofData::Bytes(b) => Ok(b.clone()),
        }
    }
}

#[derive(Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode)]
pub struct ProofTransaction {
    pub contract_name: ContractName,
    pub proof: ProofData,
    // TODO: this can technically be recovered from the hyle output
    // It's currently in a "somewhat trusted" limbo.
    pub tx_hashes: Vec<TxHash>,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode)]
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

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode)]
pub struct VerifiedProofTransaction {
    pub contract_name: ContractName,
    pub proof: Option<ProofData>, // Kept only on the local lane for indexing purposes
    pub proof_hash: ProofDataHash,
    pub proven_blobs: Vec<BlobProofOutput>,
    pub is_recursive: bool,
}

impl fmt::Debug for VerifiedProofTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedProofTransaction")
            .field("contract_name", &self.contract_name)
            .field("proof_hash", &self.proof_hash)
            .field("proof", &"[HIDDEN]")
            .field(
                "proof_len",
                &match &self.proof {
                    Some(ProofData::Base64(v)) => v.len(),
                    Some(ProofData::Bytes(v)) => v.len(),
                    None => 0,
                },
            )
            .field("proven_blobs", &self.proven_blobs)
            .finish()
    }
}

impl fmt::Debug for ProofTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofTransaction")
            .field("contract_name", &self.contract_name)
            .field("tx_hashes", &self.tx_hashes)
            .field("proof", &"[HIDDEN]")
            .field(
                "proof_len",
                &self.proof.to_bytes().unwrap_or_default().len(),
            )
            .finish()
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode)]
pub struct RegisterContractTransaction {
    pub owner: String,
    pub verifier: Verifier,
    pub program_id: ProgramId,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode)]
pub struct BlobTransaction {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    // FIXME: add a nonce or something to prevent BlobTransaction to share the same hash
}

impl BlobTransaction {
    pub fn validate_identity(&self) -> Result<(), anyhow::Error> {
        // Checks that there is a blob that proves the identity
        let identity_contract_name = self
                .identity
                .0
                .split('.')
                .last()
                .context("Transaction identity is not correctly formed. It should be in the form <id>.<contract_id_name>")?;

        // Check that there is at least one blob that has identity_contract_name as contract name
        if !self
            .blobs
            .iter()
            .any(|blob| blob.contract_name.0 == identity_contract_name)
        {
            anyhow::bail!(
                "Can't find blob that proves the identity on contract '{}'",
                identity_contract_name
            );
        }
        Ok(())
    }
}

impl Transaction {
    pub fn wrap(data: TransactionData) -> Self {
        Transaction {
            version: 1,
            transaction_data: data,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, Encode, Decode, Eq, PartialEq)]
pub struct Block {
    pub parent_hash: ConsensusProposalHash,
    pub hash: ConsensusProposalHash,
    pub block_height: BlockHeight,
    pub block_timestamp: u64,
    pub new_contract_txs: Vec<Transaction>,
    pub new_blob_txs: Vec<Transaction>,
    pub new_verified_proof_txs: Vec<Transaction>,
    pub blob_proof_outputs: Vec<HandledBlobProofOutput>,
    pub verified_blobs: Vec<(TxHash, BlobIndex, usize)>,
    pub failed_txs: Vec<Transaction>,
    pub new_bounded_validators: Vec<ValidatorPublicKey>,
    pub staking_actions: Vec<(Identity, StakingAction)>,
    pub timed_out_tx_hashes: Vec<TxHash>,
    pub settled_blob_tx_hashes: Vec<TxHash>,
    pub updated_states: BTreeMap<ContractName, StateDigest>,
}

impl Block {
    pub fn total_txs(&self) -> usize {
        self.new_contract_txs.len()
            + self.new_blob_txs.len()
            + self.new_verified_proof_txs.len()
            + self.failed_txs.len()
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

pub trait Hashable<T> {
    fn hash(&self) -> T;
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Display)]
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

impl Hashable<ConsensusProposalHash> for ConsensusProposal {
    fn hash(&self) -> ConsensusProposalHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.slot);
        _ = write!(hasher, "{}", self.view);
        _ = write!(hasher, "{:?}", self.cut);
        _ = write!(hasher, "{:?}", self.new_validators_to_bond);
        ConsensusProposalHash(hex::encode(hasher.finalize()))
    }
}

// TODO: Return consensus proposal hash
impl Hashable<ConsensusProposalHash> for SignedBlock {
    fn hash(&self) -> ConsensusProposalHash {
        self.consensus_proposal.hash()
    }
}

impl Hashable<TxHash> for Transaction {
    fn hash(&self) -> TxHash {
        match &self.transaction_data {
            TransactionData::Blob(tx) => tx.hash(),
            TransactionData::Proof(tx) => tx.hash(),
            TransactionData::VerifiedProof(tx) => tx.hash(),
            TransactionData::RegisterContract(tx) => tx.hash(),
        }
    }
}

impl Hashable<TxHash> for BlobTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.identity.0);
        hasher.update(self.blobs_hash().0);
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
impl Hashable<TxHash> for ProofTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.contract_name);
        hasher.update(self.proof.hash().0);
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
impl Hashable<ProofDataHash> for ProofData {
    fn hash(&self) -> ProofDataHash {
        let mut hasher = Sha3_256::new();
        match self.clone() {
            ProofData::Base64(v) => hasher.update(v),
            ProofData::Bytes(vec) => hasher.update(vec),
        }
        let hash_bytes = hasher.finalize();
        ProofDataHash(hex::encode(hash_bytes))
    }
}
impl Hashable<TxHash> for VerifiedProofTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.contract_name);
        _ = write!(hasher, "{:?}", self.proof_hash);
        _ = write!(hasher, "{:?}", self.proven_blobs);
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
impl Hashable<TxHash> for RegisterContractTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.owner.clone());
        hasher.update(self.verifier.0.clone());
        hasher.update(self.program_id.0.clone());
        hasher.update(self.state_digest.0.clone());
        hasher.update(self.contract_name.0.clone());
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
impl BlobTransaction {
    pub fn blobs_hash(&self) -> BlobsHash {
        BlobsHash::from_vec(&self.blobs)
    }
}

impl std::default::Default for SignedBlock {
    fn default() -> Self {
        SignedBlock {
            consensus_proposal: ConsensusProposal::default(),
            data_proposals: vec![],
            certificate: AggregateSignature {
                signature: crypto::Signature("signature".into()),
                validators: vec![],
            },
        }
    }
}

impl Add<BlockHeight> for u64 {
    type Output = BlockHeight;
    fn add(self, other: BlockHeight) -> BlockHeight {
        BlockHeight(self + other.0)
    }
}

impl Add<u64> for BlockHeight {
    type Output = BlockHeight;
    fn add(self, other: u64) -> BlockHeight {
        BlockHeight(self.0 + other)
    }
}

impl Add<BlockHeight> for BlockHeight {
    type Output = BlockHeight;
    fn add(self, other: BlockHeight) -> BlockHeight {
        BlockHeight(self.0 + other.0)
    }
}

pub fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

pub fn get_current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

#[cfg(feature = "node")]
pub struct CommonRunContext {
    pub config: SharedConf,
    pub bus: SharedMessageBus,
    pub router: std::sync::Mutex<Option<Router>>,
}

#[cfg(feature = "node")]
pub struct NodeRunContext {
    pub crypto: SharedBlstCrypto,
}

#[cfg(feature = "node")]
#[derive(Clone)]
pub struct SharedRunContext {
    pub common: Arc<CommonRunContext>,
    pub node: Arc<NodeRunContext>,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::model::BlockHeight;

    proptest! {
        #[test]
        fn block_height_add(x in 0u64..10000, y in 0u64..10000) {
            let b = BlockHeight(x);
            let c = BlockHeight(y);

            assert_eq!((b + c).0, (c + b).0);
            assert_eq!((b + y).0, x + y);
            assert_eq!((y + b).0, x + y);
        }
    }
}
