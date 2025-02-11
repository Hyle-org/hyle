use borsh::{BorshDeserialize, BorshSerialize};
use derive_more::derive::Display;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use strum::IntoDiscriminant;
use strum_macros::{EnumDiscriminants, IntoStaticStr};
use utoipa::ToSchema;

use crate::*;

#[derive(
    Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize,
)]
pub struct Transaction {
    pub version: u32,
    pub transaction_data: TransactionData,
}

impl Transaction {
    pub fn metadata(&self, parent_data_proposal_hash: DataProposalHash) -> TransactionMetadata {
        TransactionMetadata {
            version: self.version,
            transaction_kind: self.transaction_data.discriminant(),
            id: TxId(parent_data_proposal_hash.clone(), self.hash()),
        }
    }
}

impl DataSized for Transaction {
    fn estimate_size(&self) -> usize {
        match &self.transaction_data {
            TransactionData::Blob(tx) => tx.estimate_size(),
            TransactionData::Proof(tx) => tx.estimate_size(),
            TransactionData::VerifiedProof(tx) => tx.proof_size,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TransactionMetadata {
    pub version: u32,
    pub transaction_kind: TransactionKind,
    pub id: TxId,
}

#[derive(
    EnumDiscriminants,
    Debug,
    Serialize,
    Deserialize,
    Clone,
    PartialEq,
    Eq,
    BorshSerialize,
    BorshDeserialize,
    IntoStaticStr,
)]
#[strum_discriminants(derive(Default, BorshSerialize, BorshDeserialize))]
#[strum_discriminants(name(TransactionKind))]
pub enum TransactionData {
    #[strum_discriminants(default)]
    Blob(BlobTransaction),
    Proof(ProofTransaction),
    VerifiedProof(VerifiedProofTransaction),
}

impl Default for TransactionData {
    fn default() -> Self {
        TransactionData::Blob(BlobTransaction::default())
    }
}

#[derive(
    Serialize,
    Deserialize,
    ToSchema,
    Default,
    PartialEq,
    Eq,
    Clone,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct ProofTransaction {
    pub contract_name: ContractName,
    pub proof: ProofData,
}

impl ProofTransaction {
    pub fn estimate_size(&self) -> usize {
        borsh::to_vec(self).unwrap_or_default().len()
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct VerifiedProofTransaction {
    pub contract_name: ContractName,
    pub proof: Option<ProofData>, // Kept only on the local lane for indexing purposes
    pub proof_hash: ProofDataHash,
    pub proof_size: usize,
    pub proven_blobs: Vec<BlobProofOutput>,
    pub is_recursive: bool,
}

impl std::fmt::Debug for VerifiedProofTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedProofTransaction")
            .field("contract_name", &self.contract_name)
            .field("proof_hash", &self.proof_hash)
            .field("proof_size", &self.proof_size)
            .field("proof", &"[HIDDEN]")
            .field(
                "proof_len",
                &match &self.proof {
                    Some(v) => v.0.len(),
                    None => 0,
                },
            )
            .field("proven_blobs", &self.proven_blobs)
            .finish()
    }
}

impl std::fmt::Debug for ProofTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProofTransaction")
            .field("contract_name", &self.contract_name)
            .field("proof", &"[HIDDEN]")
            .field("proof_len", &self.proof.0.len())
            .finish()
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

impl From<TransactionData> for Transaction {
    fn from(data: TransactionData) -> Self {
        Transaction::wrap(data)
    }
}

impl From<BlobTransaction> for Transaction {
    fn from(tx: BlobTransaction) -> Self {
        Transaction::wrap(TransactionData::Blob(tx))
    }
}

impl From<ProofTransaction> for Transaction {
    fn from(tx: ProofTransaction) -> Self {
        Transaction::wrap(TransactionData::Proof(tx))
    }
}

impl From<VerifiedProofTransaction> for Transaction {
    fn from(tx: VerifiedProofTransaction) -> Self {
        Transaction::wrap(TransactionData::VerifiedProof(tx))
    }
}

impl Hashable<TxHash> for Transaction {
    fn hash(&self) -> TxHash {
        match &self.transaction_data {
            TransactionData::Blob(tx) => tx.hash(),
            TransactionData::Proof(tx) => tx.hash(),
            TransactionData::VerifiedProof(tx) => tx.hash(),
        }
    }
}

impl Hashable<TxHash> for ProofTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.contract_name.0.as_bytes());
        hasher.update(self.proof.hash().0);
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}
impl Hashable<TxHash> for VerifiedProofTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.contract_name.0.as_bytes());
        hasher.update(self.proof_hash.0.as_bytes());
        hasher.update(self.proven_blobs.len().to_le_bytes());
        for proven_blob in self.proven_blobs.iter() {
            hasher.update(proven_blob.hash().0);
        }
        let hash_bytes = hasher.finalize();
        TxHash(hex::encode(hash_bytes))
    }
}

#[derive(
    Debug,
    Default,
    Serialize,
    Deserialize,
    ToSchema,
    PartialEq,
    Eq,
    Clone,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct ProofData(#[serde(with = "base64_field")] pub Vec<u8>);

#[derive(
    Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize,
)]
pub struct ProofDataHash(pub String);

impl Hashable<ProofDataHash> for ProofData {
    fn hash(&self) -> ProofDataHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.0.as_slice());
        let hash_bytes = hasher.finalize();
        ProofDataHash(hex::encode(hash_bytes))
    }
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    ToSchema,
    Default,
    PartialEq,
    Eq,
    Clone,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct BlobTransaction {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    // FIXME: add a nonce or something to prevent BlobTransaction to share the same hash
}

impl BlobTransaction {
    pub fn estimate_size(&self) -> usize {
        borsh::to_vec(self).unwrap_or_default().len()
    }
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
        let Some((identity, identity_contract_name)) = self.identity.0.split_once('.') else {
            anyhow::bail!("Transaction identity {} is not correctly formed. It should be in the form <id>.<contract_id_name>", self.identity.0);
        };

        if identity.is_empty() || identity_contract_name.is_empty() {
            anyhow::bail!(
                "Transaction identity {}.{} must not have empty parts",
                identity,
                identity_contract_name
            );
        }

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
    Debug,
    Display,
    Default,
    Clone,
    Serialize,
    Deserialize,
    ToSchema,
    Eq,
    PartialEq,
    Hash,
    BorshSerialize,
    BorshDeserialize,
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
