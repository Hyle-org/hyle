use serde::{Deserialize, Serialize};
use sqlx::types::chrono::NaiveDateTime;

use crate::model::{BlockHash, TxHash};

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlockDb {
    // Struct for the blocks table
    pub hash: BlockHash,        // Corresponds to BlockHash
    pub parent_hash: BlockHash, // Parent block hash
    #[sqlx(try_from = "i64")]
    pub height: u64, // Corresponds to BlockHeight
    pub timestamp: NaiveDateTime, // UNIX timestamp
}

#[derive(Debug, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "transaction_type", rename_all = "snake_case")]
pub enum TransactionType {
    BlobTransaction,
    ProofTransaction,
    RegisterContractTransaction,
}

#[derive(Debug, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "transaction_status", rename_all = "snake_case")]
pub enum TransactionStatus {
    Success,
    Failure,
    Sequenced,
}
#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct TransactionDb {
    // Struct for the transactions table
    pub tx_hash: TxHash,       // Transaction hash
    pub block_hash: BlockHash, // Corresponds to the block hash
    pub tx_index: i32,         // Index of the transaction in the block
    #[sqlx(try_from = "i32")]
    pub version: u32, // Transaction version
    pub transaction_type: TransactionType, // Type of transaction
    pub transaction_status: TransactionStatus, // Status of the transaction
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlobDb {
    pub tx_hash: TxHash, // Corresponds to the transaction hash
    #[sqlx(try_from = "i32")]
    pub blob_index: u32, // Index of the blob within the transaction
    pub identity: String, // Identity of the blob
    pub contract_name: String, // Contract name associated with the blob
    pub data: Vec<u8>,   // Actual blob data
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct ProofTransactionDb {
    // Struct for the proof_transactions table
    pub tx_hash: TxHash, // Corresponds to the transaction hash
    pub proof: Vec<u8>,  // Proof associated with the transaction
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlobReferenceDb {
    // Struct for the blob_references table
    pub tx_hash: TxHash,       // Corresponds to the proof transaction hash
    pub contract_name: String, // Contract name
    pub blob_tx_hash: TxHash,  // Blob transaction hash
    #[sqlx(try_from = "i32")]
    pub blob_index: u32, // Index of the blob
    // Optional field for extra data
    pub hyle_output: Option<serde_json::Value>, // Optional data in JSON format
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct ContractDb {
    // Struct for the contracts table
    pub tx_hash: TxHash,       // Corresponds to the registration transaction hash
    pub owner: String,         // Owner of the contract
    pub verifier: String,      // Verifier of the contract
    pub program_id: Vec<u8>,   // Program ID
    pub state_digest: Vec<u8>, // State digest of the contract
    pub contract_name: String, // Contract name
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct ContractStateDb {
    // Struct for the contract_state table
    pub contract_name: String, // Name of the contract
    pub block_hash: BlockHash, // Hash of the block where the state is captured
    pub state_digest: Vec<u8>, // The contract state stored in JSON format
}
