use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::{
    BlockHeight, ConsensusProposalHash, Identity, Transaction, TransactionData, TxHash,
    ValidatorPublicKey,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub pubkey: Option<ValidatorPublicKey>,
    pub da_address: String,
}

/// Copy from Staking contract
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct APIStaking {
    pub stakes: BTreeMap<Identity, u128>,
    pub delegations: BTreeMap<ValidatorPublicKey, Vec<Identity>>,
    /// When a validator distribute rewards, it is added in this list to
    /// avoid distributing twice the rewards for a same block
    pub rewarded: BTreeMap<ValidatorPublicKey, Vec<BlockHeight>>,

    /// List of validators that are part of consensus
    pub bonded: Vec<ValidatorPublicKey>,
    pub total_bond: u128,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct APIBlock {
    // Struct for the blocks table
    pub hash: ConsensusProposalHash,
    pub parent_hash: ConsensusProposalHash,
    pub height: u64,    // Corresponds to BlockHeight
    pub timestamp: i64, // UNIX timestamp
}

#[cfg_attr(feature = "sqlx", derive(sqlx::Type))]
#[cfg_attr(
    feature = "sqlx",
    sqlx(type_name = "transaction_type", rename_all = "snake_case")
)]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum TransactionType {
    BlobTransaction,
    ProofTransaction,
    RegisterContractTransaction,
    Stake,
}

#[cfg_attr(feature = "sqlx", derive(sqlx::Type))]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "sqlx",
    sqlx(type_name = "transaction_status", rename_all = "snake_case")
)]
pub enum TransactionStatus {
    Success,
    Failure,
    Sequenced,
    TimedOut,
}

impl TransactionType {
    pub fn get_type_from_transaction(transaction: &Transaction) -> Self {
        match transaction.transaction_data {
            TransactionData::Blob(_) => TransactionType::BlobTransaction,
            TransactionData::Proof(_) => TransactionType::ProofTransaction,
            TransactionData::VerifiedProof(_) => TransactionType::ProofTransaction,
            TransactionData::RegisterContract(_) => TransactionType::RegisterContractTransaction,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct APITransaction {
    // Struct for the transactions table
    pub tx_hash: TxHash,                       // Transaction hash
    pub block_hash: ConsensusProposalHash,     // Corresponds to the block hash
    pub index: u32,                            // Index of the transaction within the block
    pub version: u32,                          // Transaction version
    pub transaction_type: TransactionType,     // Type of transaction
    pub transaction_status: TransactionStatus, // Status of the transaction
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TransactionWithBlobs {
    pub tx_hash: TxHash,
    pub block_hash: ConsensusProposalHash,
    pub index: u32,
    pub version: u32,
    pub transaction_type: TransactionType,
    pub transaction_status: TransactionStatus,
    pub identity: String,
    pub blobs: Vec<BlobWithStatus>,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobWithStatus {
    pub contract_name: String, // Contract name associated with the blob
    #[serde_as(as = "serde_with::hex::Hex")]
    pub data: Vec<u8>, // Actual blob data
    pub proof_outputs: Vec<serde_json::Value>, // outputs of proofs
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct APIContract {
    // Struct for the contracts table
    pub tx_hash: TxHash,  // Corresponds to the registration transaction hash
    pub owner: String,    // Owner of the contract
    pub verifier: String, // Verifier of the contract
    #[serde_as(as = "serde_with::hex::Hex")]
    pub program_id: Vec<u8>, // Program ID
    #[serde_as(as = "serde_with::hex::Hex")]
    pub state_digest: Vec<u8>, // State digest of the contract
    pub contract_name: String, // Contract name
}

#[derive(Debug, Serialize, Deserialize)]
pub struct APIContractState {
    // Struct for the contract_state table
    pub contract_name: String,             // Name of the contract
    pub block_hash: ConsensusProposalHash, // Hash of the block where the state is captured
    pub state_digest: Vec<u8>,             // The contract state stored in JSON format
}

#[derive(Debug, Serialize, Deserialize)]
pub struct APIBlob {
    pub tx_hash: TxHash,       // Corresponds to the transaction hash
    pub blob_index: u32,       // Index of the blob within the transaction
    pub identity: String,      // Identity of the blob
    pub contract_name: String, // Contract name associated with the blob
    pub data: Vec<u8>,         // Actual blob data
    pub verified: bool,        // Verification status
}
