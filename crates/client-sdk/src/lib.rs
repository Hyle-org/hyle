use anyhow::Context;
use bincode::{Decode, Encode};
use derive_more::derive::Display;
use sdk::{flatten_blobs, Blob, Identity, TxHash};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};

pub mod helpers;
pub mod transaction_builder;

pub trait Hashable<T> {
    fn hash(&self) -> T;
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq, Clone, Encode, Decode)]
pub struct ProofData(#[serde(with = "base64_field")] pub Vec<u8>);

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode)]
pub struct ProofDataHash(pub String);

impl Hashable<ProofDataHash> for ProofData {
    fn hash(&self) -> ProofDataHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.0.as_slice());
        let hash_bytes = hasher.finalize();
        ProofDataHash(hex::encode(hash_bytes))
    }
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode)]
pub struct BlobTransaction {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    // FIXME: add a nonce or something to prevent BlobTransaction to share the same hash
}
impl Hashable<TxHash> for BlobTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.identity.0.as_bytes());
        hasher.update(self.blobs_hash().0);
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}

impl BlobTransaction {
    pub fn blobs_hash(&self) -> BlobsHash {
        BlobsHash::from_vec(&self.blobs)
    }

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

#[derive(
    Debug, Display, Default, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Encode, Decode,
)]
pub struct BlobsHash(pub String);

impl BlobsHash {
    pub fn new(s: &str) -> BlobsHash {
        BlobsHash(s.into())
    }

    pub fn from_vec(vec: &[Blob]) -> BlobsHash {
        Self::from_concatenated(&flatten_blobs(vec))
    }

    pub fn from_concatenated(vec: &Vec<u8>) -> BlobsHash {
        let mut hasher = Sha3_256::new();
        hasher.update(vec.as_slice());
        let hash_bytes = hasher.finalize();
        BlobsHash(hex::encode(hash_bytes))
    }
}

pub mod base64_field {
    use base64::prelude::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded = BASE64_STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        BASE64_STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}
