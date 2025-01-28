use std::{
    fmt::Display,
    ops::{Add, Sub},
};

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(
    Debug, Serialize, Deserialize, ToSchema, Clone, Encode, Decode, PartialEq, Eq, Default,
)]
pub struct ConsensusProposalHash(pub String);
pub type BlockHash = ConsensusProposalHash;

impl std::hash::Hash for ConsensusProposalHash {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write(self.0.as_bytes());
    }
}

pub trait Hashable<T> {
    fn hash(&self) -> T;
}

pub trait DataSized {
    fn estimate_size(&self) -> usize;
}

pub trait Digestable {
    fn as_digest(&self) -> StateDigest;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContractInput {
    pub initial_state: StateDigest,
    pub identity: Identity,
    pub index: BlobIndex,
    pub blobs: Vec<Blob>,
    pub tx_hash: TxHash,
    pub tx_ctx: Option<TxContext>,
    pub private_input: Vec<u8>,
}

#[derive(Default, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct StateDigest(pub Vec<u8>);

impl std::fmt::Debug for StateDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "StateDigest({})", hex::encode(&self.0))
    }
}

impl Digestable for StateDigest {
    fn as_digest(&self) -> StateDigest {
        self.clone()
    }
}

#[derive(
    Default,
    Serialize,
    Deserialize,
    ToSchema,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Encode,
    Decode,
    Ord,
    PartialOrd,
)]
pub struct Identity(pub String);

#[derive(
    Default,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Encode,
    Decode,
    Ord,
    PartialOrd,
    ToSchema,
)]
pub struct TxHash(pub String);

#[derive(
    Default,
    Serialize,
    Deserialize,
    ToSchema,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Encode,
    Decode,
    Copy,
)]
pub struct BlobIndex(pub usize);

impl Add<usize> for BlobIndex {
    type Output = BlobIndex;
    fn add(self, other: usize) -> BlobIndex {
        BlobIndex(self.0 + other)
    }
}

#[derive(Default, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct BlobData(pub Vec<u8>);

impl std::fmt::Debug for BlobData {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "BlobData({})", hex::encode(&self.0))
    }
}

#[derive(Debug, Encode, Decode)]
pub struct StructuredBlobData<Parameters> {
    pub caller: Option<BlobIndex>,
    pub callees: Option<Vec<BlobIndex>>,
    pub parameters: Parameters,
}

impl<Parameters: Encode> From<StructuredBlobData<Parameters>> for BlobData {
    fn from(val: StructuredBlobData<Parameters>) -> Self {
        BlobData(
            bincode::encode_to_vec(val, bincode::config::standard())
                .expect("failed to encode BlobData"),
        )
    }
}
impl<Parameters: Decode> TryFrom<BlobData> for StructuredBlobData<Parameters> {
    type Error = bincode::error::DecodeError;

    fn try_from(val: BlobData) -> Result<StructuredBlobData<Parameters>, Self::Error> {
        bincode::decode_from_slice(&val.0, bincode::config::standard()).map(|(data, _)| data)
    }
}

#[derive(
    Debug, Serialize, Deserialize, ToSchema, Default, Clone, PartialEq, Eq, Encode, Decode, Hash,
)]
pub struct Blob {
    pub contract_name: ContractName,
    pub data: BlobData,
}

#[derive(Debug, Encode, Decode)]
pub struct StructuredBlob<Parameters> {
    pub contract_name: ContractName,
    pub data: StructuredBlobData<Parameters>,
}

impl<Parameters: Encode> From<StructuredBlob<Parameters>> for Blob {
    fn from(val: StructuredBlob<Parameters>) -> Self {
        Blob {
            contract_name: val.contract_name,
            data: BlobData::from(val.data),
        }
    }
}

impl<Parameters: Decode> TryFrom<Blob> for StructuredBlob<Parameters> {
    type Error = bincode::error::DecodeError;

    fn try_from(val: Blob) -> Result<StructuredBlob<Parameters>, Self::Error> {
        let data = bincode::decode_from_slice(&val.data.0, bincode::config::standard())
            .map(|(data, _)| data)?;
        Ok(StructuredBlob {
            contract_name: val.contract_name,
            data,
        })
    }
}

pub trait ContractAction: Send {
    fn as_blob(
        &self,
        contract_name: ContractName,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Blob;
}

pub fn flatten_blobs(blobs: &[Blob]) -> Vec<u8> {
    blobs
        .iter()
        .flat_map(|b| {
            b.contract_name
                .0
                .as_bytes()
                .iter()
                .chain(b.data.0.iter())
                .copied()
        })
        .collect()
}

#[derive(
    Default,
    Debug,
    Clone,
    Serialize,
    Deserialize,
    ToSchema,
    Eq,
    PartialEq,
    Hash,
    Encode,
    Decode,
    Ord,
    PartialOrd,
)]
pub struct ContractName(pub String);

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, ToSchema, Eq, PartialEq, Hash, Encode, Decode,
)]
pub struct Verifier(pub String);

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, ToSchema, Eq, PartialEq, Hash, Encode, Decode,
)]
pub struct ProgramId(pub Vec<u8>);

#[derive(
    Default, Serialize, Deserialize, ToSchema, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode,
)]
pub struct HyleOutput {
    pub version: u32,
    pub initial_state: StateDigest,
    pub next_state: StateDigest,
    pub identity: Identity,
    pub index: BlobIndex,
    pub blobs: Vec<u8>,
    pub tx_hash: TxHash, // Technically redundant with identity + blobs hash
    pub success: bool,

    // Optional - if empty, these won't be checked, but also can't be used inside the program.
    pub tx_ctx: Option<TxContext>,

    pub registered_contracts: Vec<RegisterContractEffect>,

    pub program_outputs: Vec<u8>,
}

#[derive(
    Default, Serialize, Deserialize, ToSchema, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode,
)]
pub struct TxContext {
    pub block_hash: BlockHash,
    pub block_height: BlockHeight,
    pub timestamp: u128,
    pub chain_id: u128,
}

impl Identity {
    pub fn new<S: Into<Self>>(s: S) -> Self {
        s.into()
    }
}
impl<S: Into<String>> From<S> for Identity {
    fn from(s: S) -> Self {
        Identity(s.into())
    }
}

impl TxHash {
    pub fn new<S: Into<Self>>(s: S) -> Self {
        s.into()
    }
}
impl<S: Into<String>> From<S> for TxHash {
    fn from(s: S) -> Self {
        TxHash(s.into())
    }
}

impl ContractName {
    pub fn new<S: Into<Self>>(s: S) -> Self {
        s.into()
    }
}
impl<S: Into<String>> From<S> for ContractName {
    fn from(s: S) -> Self {
        ContractName(s.into())
    }
}

impl Verifier {
    pub fn new<S: Into<Self>>(s: S) -> Self {
        s.into()
    }
}
impl<S: Into<String>> From<S> for Verifier {
    fn from(s: S) -> Self {
        Verifier(s.into())
    }
}
impl From<Vec<u8>> for ProgramId {
    fn from(v: Vec<u8>) -> Self {
        ProgramId(v.clone())
    }
}
impl From<&Vec<u8>> for ProgramId {
    fn from(v: &Vec<u8>) -> Self {
        ProgramId(v.clone())
    }
}
impl From<&[u8]> for ProgramId {
    fn from(v: &[u8]) -> Self {
        ProgramId(v.to_vec().clone())
    }
}

impl Display for TxHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for BlobIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for ContractName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl From<usize> for BlobIndex {
    fn from(i: usize) -> Self {
        BlobIndex(i)
    }
}

#[cfg_attr(feature = "full", derive(derive_more::derive::Display))]
#[derive(
    Default,
    Debug,
    Clone,
    Serialize,
    Deserialize,
    ToSchema,
    Eq,
    PartialEq,
    Hash,
    Copy,
    Encode,
    Decode,
)]
pub struct BlockHeight(pub u64);

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

impl Sub<u64> for BlockHeight {
    type Output = BlockHeight;
    fn sub(self, other: u64) -> BlockHeight {
        BlockHeight(self.0 - other)
    }
}

impl Add<BlockHeight> for BlockHeight {
    type Output = BlockHeight;
    fn add(self, other: BlockHeight) -> BlockHeight {
        BlockHeight(self.0 + other.0)
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode)]
pub struct RegisterContractAction {
    pub verifier: Verifier,
    pub program_id: ProgramId,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}

#[cfg(feature = "full")]
impl Hashable<TxHash> for RegisterContractAction {
    fn hash(&self) -> TxHash {
        use sha3::{Digest, Sha3_256};

        let mut hasher = Sha3_256::new();
        hasher.update(self.verifier.0.clone());
        hasher.update(self.program_id.0.clone());
        hasher.update(self.state_digest.0.clone());
        hasher.update(self.contract_name.0.clone());
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}

impl ContractAction for RegisterContractAction {
    fn as_blob(
        &self,
        contract_name: ContractName,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        Blob {
            contract_name,
            data: BlobData::from(StructuredBlobData {
                caller,
                callees,
                parameters: self.clone(),
            }),
        }
    }
}

/// Used by the Hylé node to recognize contract registration.
/// Simply output this struct in your HyleOutput registered_contracts.
/// See uuid-tld for examples.
#[derive(
    Default, Serialize, Deserialize, ToSchema, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode,
)]
pub struct RegisterContractEffect {
    pub verifier: Verifier,
    pub program_id: ProgramId,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}

#[cfg(feature = "full")]
impl Hashable<TxHash> for RegisterContractEffect {
    fn hash(&self) -> TxHash {
        use sha3::{Digest, Sha3_256};

        let mut hasher = Sha3_256::new();
        hasher.update(self.verifier.0.clone());
        hasher.update(self.program_id.0.clone());
        hasher.update(self.state_digest.0.clone());
        hasher.update(self.contract_name.0.clone());
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
