#![cfg_attr(not(test), no_std)]

extern crate alloc;

use core::fmt::Display;

use alloc::string::String;
use alloc::vec::Vec;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

pub mod erc20;
pub mod guest;
pub mod identity_provider;

pub trait HyleContract {
    fn caller(&self) -> Identity;
}

pub trait Digestable {
    fn as_digest(&self) -> StateDigest;
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ContractInput<State>
where
    State: Digestable,
{
    pub initial_state: State,
    pub identity: Identity,
    pub tx_hash: TxHash,
    pub private_blob: BlobData,
    pub blobs: Vec<Blob>,
    pub index: BlobIndex,
}

#[derive(Default, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct StateDigest(pub Vec<u8>);

impl alloc::fmt::Debug for StateDigest {
    fn fmt(&self, f: &mut alloc::fmt::Formatter) -> alloc::fmt::Result {
        write!(f, "StateDigest({})", hex::encode(&self.0))
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct Identity(pub String);

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct TxHash(pub String);

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct BlobIndex(pub u32);

#[derive(Default, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct BlobData(pub Vec<u8>);

impl alloc::fmt::Debug for BlobData {
    fn fmt(&self, f: &mut alloc::fmt::Formatter) -> alloc::fmt::Result {
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

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
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

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Encode, Decode)]
pub struct ContractName(pub String);

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct HyleOutput {
    pub version: u32,
    pub initial_state: StateDigest,
    pub next_state: StateDigest,
    pub identity: Identity,
    pub tx_hash: TxHash,
    pub index: BlobIndex,
    pub blobs: Vec<u8>,
    pub success: bool,
    pub program_outputs: Vec<u8>,
}

impl From<String> for Identity {
    fn from(s: String) -> Self {
        Identity(s)
    }
}
impl From<&str> for Identity {
    fn from(s: &str) -> Self {
        Identity(s.into())
    }
}
impl From<String> for TxHash {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for TxHash {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}
impl From<&str> for ContractName {
    fn from(s: &str) -> Self {
        ContractName(s.into())
    }
}
impl From<String> for ContractName {
    fn from(s: String) -> Self {
        ContractName(s)
    }
}

impl TxHash {
    pub fn new(s: &str) -> TxHash {
        TxHash(s.into())
    }
}
impl Display for TxHash {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for BlobIndex {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for ContractName {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for Identity {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl From<u32> for BlobIndex {
    fn from(i: u32) -> Self {
        BlobIndex(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, string::ToString, vec};

    #[test]
    fn test_identity_from_string() {
        let identity_str = "test_identity".to_string();
        let identity = Identity::from(identity_str.clone());
        assert_eq!(identity.0, identity_str);
    }

    #[test]
    fn test_identity_from_str() {
        let identity_str = "test_identity";
        let identity = Identity::from(identity_str);
        assert_eq!(identity.0, identity_str.to_string());
    }

    #[test]
    fn test_txhash_from_string() {
        let txhash_str = "test_txhash".to_string();
        let txhash = TxHash::from(txhash_str.clone());
        assert_eq!(txhash.0, txhash_str);
    }

    #[test]
    fn test_txhash_from_str() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::from(txhash_str);
        assert_eq!(txhash.0, txhash_str.to_string());
    }

    #[test]
    fn test_txhash_new() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::new(txhash_str);
        assert_eq!(txhash.0, txhash_str.to_string());
    }

    #[test]
    fn test_blobindex_from_u32() {
        let index = 42;
        let blob_index = BlobIndex::from(index);
        assert_eq!(blob_index.0, index);
    }

    #[test]
    fn test_txhash_display() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::new(txhash_str);
        assert_eq!(format!("{}", txhash), txhash_str);
    }

    #[test]
    fn test_blobindex_display() {
        let index = 42;
        let blob_index = BlobIndex::from(index);
        assert_eq!(format!("{}", blob_index), index.to_string());
    }

    #[test]
    fn test_state_digest_encoding() {
        let state_digest = StateDigest(vec![1, 2, 3, 4]);
        let encoded = bincode::encode_to_vec(&state_digest, bincode::config::standard())
            .expect("Failed to encode StateDigest");
        let decoded: StateDigest =
            bincode::decode_from_slice(&encoded, bincode::config::standard())
                .expect("Failed to decode StateDigest")
                .0;
        assert_eq!(state_digest, decoded);
    }

    #[test]
    fn test_identity_encoding() {
        let identity = Identity("test_identity".to_string());
        let encoded = bincode::encode_to_vec(&identity, bincode::config::standard())
            .expect("Failed to encode Identity");
        let decoded: Identity = bincode::decode_from_slice(&encoded, bincode::config::standard())
            .expect("Failed to decode Identity")
            .0;
        assert_eq!(identity, decoded);
    }

    #[test]
    fn test_txhash_encoding() {
        let txhash = TxHash("test_txhash".to_string());
        let encoded = bincode::encode_to_vec(&txhash, bincode::config::standard())
            .expect("Failed to encode TxHash");
        let decoded: TxHash = bincode::decode_from_slice(&encoded, bincode::config::standard())
            .expect("Failed to decode TxHash")
            .0;
        assert_eq!(txhash, decoded);
    }

    #[test]
    fn test_blobindex_encoding() {
        let blob_index = BlobIndex(42);
        let encoded = bincode::encode_to_vec(&blob_index, bincode::config::standard())
            .expect("Failed to encode BlobIndex");
        let decoded: BlobIndex = bincode::decode_from_slice(&encoded, bincode::config::standard())
            .expect("Failed to decode BlobIndex")
            .0;
        assert_eq!(blob_index, decoded);
    }

    #[test]
    fn test_blobdata_encoding() {
        let blob_data = BlobData(vec![1, 2, 3, 4]);
        let encoded = bincode::encode_to_vec(&blob_data, bincode::config::standard())
            .expect("Failed to encode BlobData");
        let decoded: BlobData = bincode::decode_from_slice(&encoded, bincode::config::standard())
            .expect("Failed to decode BlobData")
            .0;
        assert_eq!(blob_data, decoded);
    }
}
