//! Various data structures

use anyhow::{bail, Error};
use axum::Router;
use base64::prelude::*;
use bincode::{Decode, Encode};
use derive_more::Display;
use hyle_contract_sdk::{BlobIndex, HyleOutput, Identity, StateDigest, TxHash};
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
        let concatenated = vec
            .iter()
            .flat_map(|b| b.data.0.clone())
            .collect::<Vec<u8>>();
        Self::from_concatenated(&concatenated)
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

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Display, Encode, Decode,
)]
pub struct ContractName(pub String);

impl From<String> for ContractName {
    fn from(s: String) -> Self {
        ContractName(s)
    }
}

impl From<&str> for ContractName {
    fn from(s: &str) -> Self {
        ContractName(s.into())
    }
}

pub use hyle_contract_sdk::BlobData;

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct Transaction {
    pub version: u32,
    pub transaction_data: TransactionData,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode, Hash)]
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
    pub blobs_references: Vec<BlobReference>,
    pub proof: ProofData,
}

impl fmt::Debug for ProofTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofTransaction")
            .field("blobs_references", &self.blobs_references)
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
    pub hyle_outputs: Vec<HyleOutput>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct BlobReference {
    pub contract_name: ContractName,
    pub blob_tx_hash: TxHash,
    pub blob_index: BlobIndex,
}

impl Display for BlobReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.blob_tx_hash, self.blob_index, self.contract_name
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct RegisterContractTransaction {
    pub owner: String,
    pub verifier: String,
    pub program_id: Vec<u8>,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode, Hash)]
pub struct BlobTransaction {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    // FIXME: add a nonce or something to prevent BlobTransaction to share the same hash
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct Blob {
    pub contract_name: ContractName,
    pub data: BlobData,
}

impl Transaction {
    pub fn wrap(data: TransactionData) -> Self {
        Transaction {
            version: 1,
            transaction_data: data,
        }
    }
}

#[derive(Debug)]
pub struct HandledBlockOutput {
    pub new_contract_txs: Vec<Transaction>,
    pub new_blob_txs: Vec<Transaction>,
    pub new_verified_proof_txs: Vec<Transaction>,
    pub verified_blobs: Vec<(TxHash, BlobIndex)>,
    pub failed_txs: Vec<Transaction>,
    pub stakers: Vec<Staker>,
    pub timed_out_tx_hashes: Vec<TxHash>,
    pub settled_blob_tx_hashes: Vec<TxHash>,
    pub updated_states: HashMap<ContractName, StateDigest>,
}

#[derive(Debug)]
pub struct HandledProofTxOutput {
    pub settled_blob_tx_hashes: Vec<TxHash>,
    pub updated_states: HashMap<ContractName, StateDigest>,
}

#[derive(
    Display, Debug, Serialize, Deserialize, Default, Clone, Encode, Decode, PartialEq, Eq, Hash,
)]
pub struct BlockHash(pub String);

impl Type<Postgres> for BlockHash {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }
}
impl<'q> sqlx::Encode<'q, sqlx::Postgres> for BlockHash {
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

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for BlockHash {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> std::result::Result<
        BlockHash,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        let inner = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(BlockHash(inner))
    }
}

impl BlockHash {
    pub fn new(s: &str) -> BlockHash {
        BlockHash(s.into())
    }
}

pub trait Hashable<T> {
    fn hash(&self) -> T;
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Display)]
#[display("")]
pub struct Block {
    pub parent_hash: BlockHash,
    pub height: BlockHeight,
    pub timestamp: u64,
    pub new_bonded_validators: Vec<ValidatorPublicKey>,
    pub txs: Vec<Transaction>,
}

impl Ord for Block {
    fn cmp(&self, other: &Self) -> Ordering {
        self.height.0.cmp(&other.height.0)
    }
}

impl PartialOrd for Block {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.hash() == other.hash()
    }
}

impl Eq for Block {}

impl std::hash::Hash for Block {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let h: BlockHash = Hashable::hash(self);
        h.hash(state);
    }
}

impl Hashable<BlockHash> for Block {
    fn hash(&self) -> BlockHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.parent_hash);
        _ = write!(hasher, "{}", self.height);
        _ = write!(hasher, "{}", self.timestamp);
        for tx in self.txs.iter() {
            hasher.update(tx.hash().0);
        }
        BlockHash(hex::encode(hasher.finalize()))
    }
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
        for blob_ref in self.blobs_references.iter() {
            _ = write!(hasher, "{}", blob_ref.contract_name);
            _ = write!(hasher, "{}", blob_ref.blob_tx_hash);
            _ = write!(hasher, "{}", blob_ref.blob_index);
        }
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
        _ = write!(hasher, "{}", self.owner);
        _ = write!(hasher, "{}", self.verifier);
        hasher.update(self.program_id.clone());
        hasher.update(self.state_digest.0.clone());
        _ = write!(hasher, "{}", self.contract_name);
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
impl BlobTransaction {
    pub fn blobs_hash(&self) -> BlobsHash {
        BlobsHash::from_vec(&self.blobs)
    }
}

impl std::default::Default for Block {
    fn default() -> Self {
        Block {
            parent_hash: BlockHash::default(),
            height: BlockHeight(0),
            new_bonded_validators: vec![],
            timestamp: get_current_timestamp(),
            txs: vec![],
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

        impl<'de> Visitor<'de> for ValidatorPublicKeyVisitor {
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
