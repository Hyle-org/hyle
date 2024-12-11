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
use serde::{
    de::{self, Visitor},
    Deserialize, Serialize,
};
use sha3::{Digest, Sha3_256};
use sqlx::{prelude::Type, Postgres};
use std::{
    cmp::Ordering,
    collections::HashMap,
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
    consensus::{staking::Staker, utils::HASH_DISPLAY_SIZE},
    utils::{conf::SharedConf, crypto::SharedBlstCrypto},
};

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

#[derive(
    Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode, Hash, IntoStaticStr,
)]
pub enum TransactionData {
    Stake(Staker), // FIXME: to remove, this is temporary waiting for real staking contract !!
    Blob(BlobTransaction),
    Proof(ProofTransaction),
    VerifiedProof(VerifiedProofTransaction),
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

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Encode, Decode, Hash)]
#[serde(untagged)]
pub enum ProofData {
    Base64(String),
    Bytes(Vec<u8>),
}

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
#[derive(Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode, Hash)]
pub struct ProofTransaction {
    // TODO: investigate if we can remove blob_tx_hash. It can be reconstrustruced from HyleOutput attributes (blob + identity)
    pub blob_tx_hash: TxHash,
    pub proof: ProofData,
    pub contract_name: ContractName,
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct VerifiedProofTransaction {
    pub proof_transaction: ProofTransaction,
    pub hyle_output: HyleOutput,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct RegisterContractTransaction {
    pub owner: String,
    pub verifier: Verifier,
    pub program_id: ProgramId,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode, Hash)]
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
pub struct ProcessedBlock {
    pub block_parent_hash: ProcessedBlockHash,
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
    pub updated_states: HashMap<ContractName, StateDigest>,
}

impl Ord for ProcessedBlock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.block_height.0.cmp(&other.block_height.0)
    }
}

impl PartialOrd for ProcessedBlock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hashable<ProcessedBlockHash> for ProcessedBlock {
    fn hash(&self) -> ProcessedBlockHash {
        let mut hasher = Sha3_256::new();

        _ = write!(hasher, "{}", self.block_parent_hash);
        _ = write!(hasher, "{}", self.block_height);
        _ = write!(hasher, "{}", self.block_timestamp);
        for tx in self
            .new_contract_txs
            .iter()
            .chain(self.new_blob_txs.iter())
            .chain(self.new_verified_proof_txs.iter())
        {
            hasher.update(tx.hash().0);
        }
        for (tx_hash, blob_v) in self.verified_blobs.iter() {
            _ = write!(hasher, "{}", tx_hash);
            _ = write!(hasher, "{}", blob_v);
        }
        for tx_f in self.failed_txs.iter() {
            hasher.update(tx_f.hash().0);
        }
        for staker in self.stakers.iter() {
            hasher.update(staker.hash().0);
        }
        for new_bounded_validator in self.new_bounded_validators.iter() {
            hasher.update(new_bounded_validator.0.as_slice());
        }
        for settled_blob_tx_hash in self.settled_blob_tx_hashes.iter() {
            _ = write!(hasher, "{}", settled_blob_tx_hash);
        }
        for (cn, sd) in self.updated_states.iter() {
            _ = write!(hasher, "{}", cn);
            _ = write!(hasher, "{:?}", sd);
        }
        _ = write!(hasher, "{}", self.block_timestamp);

        ProcessedBlockHash(hex::encode(hasher.finalize()))
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ProcessedBlockHash(pub String);

impl Display for ProcessedBlockHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.0.get(..HASH_DISPLAY_SIZE * 2).unwrap_or(&self.0)
        )
    }
}

impl Type<Postgres> for ProcessedBlockHash {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }
}
impl sqlx::Encode<'_, sqlx::Postgres> for ProcessedBlockHash {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> std::result::Result<
        sqlx::encode::IsNull,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        <String as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for ProcessedBlockHash {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> std::result::Result<
        ProcessedBlockHash,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        let inner = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(ProcessedBlockHash(inner))
    }
}

impl ProcessedBlockHash {
    pub fn new(s: &str) -> ProcessedBlockHash {
        ProcessedBlockHash(s.into())
    }
}

pub trait Hashable<T> {
    fn hash(&self) -> T;
}

impl Hashable<TxHash> for Transaction {
    fn hash(&self) -> TxHash {
        match &self.transaction_data {
            TransactionData::Stake(staker) => staker.hash(),
            TransactionData::Blob(tx) => tx.hash(),
            TransactionData::Proof(tx) => tx.hash(),
            TransactionData::VerifiedProof(tx) => tx.hash(),
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
        match self.proof.clone() {
            ProofData::Base64(v) => hasher.update(v),
            ProofData::Bytes(vec) => hasher.update(vec),
        }
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
impl Hashable<TxHash> for VerifiedProofTransaction {
    fn hash(&self) -> TxHash {
        self.proof_transaction.hash()
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

#[derive(Clone, Encode, Decode, Default, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct ValidatorPublicKey(pub Vec<u8>);

impl std::fmt::Debug for ValidatorPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValidatorPubK")
            .field(&hex::encode(
                self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0),
            ))
            .finish()
    }
}

impl Display for ValidatorPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            &hex::encode(self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0),)
        )
    }
}

impl Serialize for ValidatorPublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(hex::encode(&self.0).as_str())
    }
}

impl<'de> Deserialize<'de> for ValidatorPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ValidatorPublicKeyVisitor;

        impl Visitor<'_> for ValidatorPublicKeyVisitor {
            type Value = ValidatorPublicKey;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a hex string representing a ValidatorPublicKey")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let bytes = hex::decode(value).map_err(de::Error::custom)?;
                Ok(ValidatorPublicKey(bytes))
            }
        }

        deserializer.deserialize_str(ValidatorPublicKeyVisitor)
    }
}

pub fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
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
