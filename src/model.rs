//! Various data structures

use anyhow::{bail, Error};
use axum::Router;
use base64::prelude::*;
use bincode::{Decode, Encode};
use derive_more::Display;
use hyle_contract_sdk::{
    flatten_blobs, BlobIndex, HyleOutput, Identity, ProgramId, StateDigest, TxHash, Verifier,
};
pub use hyle_contract_sdk::{Blob, BlobData, ContractName};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt,
    io::Write,
    ops::Add,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use strum_macros::IntoStaticStr;
use tracing::debug;

use crate::{
    bus::SharedMessageBus,
    consensus::{ConsensusProposal, ConsensusProposalHash},
    mempool::storage::DataProposal,
    utils::{
        conf::SharedConf,
        crypto::{AggregateSignature, SharedBlstCrypto},
    },
};

use staking::Staker;

// Re-export
pub use staking::model::ValidatorPublicKey;

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
    Stake(Staker), // FIXME: to remove, this is temporary waiting for real staking contract !!
    Blob(BlobTransaction),
    Proof(ProofTransaction),
    VerifiedProof(VerifiedProofTransaction),
    RecursiveProof(RecursiveProofTransaction),
    VerifiedRecursiveProof(VerifiedRecursiveProofTransaction),
    RegisterContract(RegisterContractTransaction),
}

impl TransactionData {
    pub fn blob(&self) -> Result<BlobTransaction, Error> {
        match self {
            TransactionData::Blob(blob_tx) => Ok(blob_tx.clone()),
            _ => bail!("Called blob() on non-Blob transaction data"),
        }
    }
    pub fn verified_proof(&self) -> Result<VerifiedProofTransaction, Error> {
        match self {
            TransactionData::VerifiedProof(verified_proof_tx) => Ok(verified_proof_tx.clone()),
            _ => bail!("Called blob() on non-VerifiedProof transaction data"),
        }
    }
    pub fn register_contract(&self) -> Result<RegisterContractTransaction, Error> {
        match self {
            TransactionData::RegisterContract(register_contract_tx) => {
                Ok(register_contract_tx.clone())
            }
            _ => bail!("Called blob() on non-RegisterContract transaction data"),
        }
    }
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
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode)]
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
    // TODO: investigate if we can remove blob_tx_hash. It can be reconstrustruced from HyleOutput attributes (blob + identity)
    pub blob_tx_hash: TxHash,
    pub proof: ProofData,
    pub contract_name: ContractName,
}

#[derive(Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode)]
pub struct RecursiveProofTransaction {
    pub via: ContractName,
    pub proof: ProofData,
    pub verifies: Vec<(TxHash, ContractName)>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode)]
pub struct VerifiedProofTransaction {
    pub blob_tx_hash: TxHash,
    pub contract_name: ContractName,
    pub proof_hash: ProofDataHash,
    pub hyle_output: HyleOutput,
    pub proof: Option<ProofData>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode)]
pub struct VerifiedRecursiveProofTransaction {
    pub via: ContractName,
    pub proof: Option<ProofData>,
    pub proof_hash: ProofDataHash,
    pub verifies: Vec<VerifiedProofTransaction>,
}

impl fmt::Debug for ProofTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofTransaction")
            .field("contract_name", &self.contract_name)
            .field("proof", &"[HIDDEN]")
            .field(
                "proof_len",
                &self.proof.to_bytes().unwrap_or_default().len(),
            )
            .finish()
    }
}

impl fmt::Debug for VerifiedProofTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedProofTransaction")
            .field("contract_name", &self.contract_name)
            .field("blob_tx_hash", &self.blob_tx_hash)
            .field("proof_hash", &self.proof_hash)
            .field("hyle_output", &self.hyle_output)
            .field("proof", &"[HIDDEN]")
            .field(
                "proof_len",
                &self
                    .proof
                    .as_ref()
                    .unwrap_or(&ProofData::default())
                    .to_bytes()
                    .unwrap_or_default()
                    .len(),
            )
            .finish()
    }
}

impl fmt::Debug for RecursiveProofTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecursiveProofTransaction")
            .field("via", &self.via)
            .field("verifies", &self.verifies)
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
    pub verified_blobs: Vec<(TxHash, BlobIndex)>,
    pub failed_txs: Vec<Transaction>,
    pub stakers: Vec<Staker>,
    pub new_bounded_validators: Vec<ValidatorPublicKey>,
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
            + self.stakers.len()
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

impl std::hash::Hash for SignedBlock {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let h: ConsensusProposalHash = Hashable::hash(self);
        h.hash(state);
    }
}

impl Hashable<ConsensusProposalHash> for SignedBlock {
    fn hash(&self) -> ConsensusProposalHash {
        self.consensus_proposal.hash()
    }
}
impl Hashable<TxHash> for Transaction {
    fn hash(&self) -> TxHash {
        match &self.transaction_data {
            TransactionData::Stake(staker) => staker.hash(),
            TransactionData::Blob(tx) => tx.hash(),
            TransactionData::Proof(tx) => tx.hash(),
            TransactionData::RecursiveProof(tx) => tx.hash(),
            TransactionData::VerifiedProof(tx) => tx.hash(),
            TransactionData::VerifiedRecursiveProof(tx) => tx.hash(),
            TransactionData::RegisterContract(tx) => tx.hash(),
        }
    }
}
impl Hashable<TxHash> for Staker {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{:?}", self.pubkey.0);
        _ = write!(hasher, "{}", self.stake.amount);
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
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
impl Hashable<TxHash> for RecursiveProofTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.via);
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
        _ = write!(hasher, "{}", self.blob_tx_hash);
        _ = write!(hasher, "{}", self.contract_name);
        _ = write!(hasher, "{:?}", self.proof_hash);
        _ = write!(hasher, "{:?}", self.hyle_output);
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
impl Hashable<TxHash> for VerifiedRecursiveProofTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.via);
        _ = write!(hasher, "{:?}", self.proof_hash);
        _ = write!(hasher, "{:?}", self.verifies);
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
                signature: crate::utils::crypto::Signature("signature".into()),
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

pub struct CommonRunContext {
    pub config: SharedConf,
    pub bus: SharedMessageBus,
    pub router: std::sync::Mutex<Option<Router>>,
}
pub struct NodeRunContext {
    pub crypto: SharedBlstCrypto,
}

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
