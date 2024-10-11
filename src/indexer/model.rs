use serde::{Deserialize, Serialize};
use sqlx::types::chrono::NaiveDateTime;

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlockDb {
    // Struct for the blocks table
    pub hash: Vec<u8>,            // Corresponds to BlockHash
    pub parent_hash: Vec<u8>,     // Parent block hash
    pub height: i64,              // Corresponds to BlockHeight
    pub timestamp: NaiveDateTime, // UNIX timestamp
}

#[derive(Debug, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "transaction_type", rename_all = "snake_case")]
pub enum TransactionType {
    BlobTransaction,
    ProofTransaction,
    RegisterContractTransaction,
}

impl From<String> for TransactionType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "blob_transaction" => TransactionType::BlobTransaction,
            "proof_transaction" => TransactionType::ProofTransaction,
            "register_contract_transaction" => TransactionType::RegisterContractTransaction,
            _ => panic!("Invalid transaction type"),
        }
    }
}

#[derive(Debug, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "transaction_status", rename_all = "snake_case")]
pub enum TransactionStatus {
    Success,
    Failure,
    Sequenced,
}

impl From<String> for TransactionStatus {
    fn from(s: String) -> Self {
        match s.as_str() {
            "success" => TransactionStatus::Success,
            "failure" => TransactionStatus::Failure,
            "sequenced" => TransactionStatus::Sequenced,
            _ => panic!("Invalid transaction status"),
        }
    }
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct TransactionDb {
    // Struct for the transactions table
    pub tx_hash: Vec<u8>,                      // Transaction hash
    pub block_hash: Vec<u8>,                   // Corresponds to the block hash
    pub tx_index: i32,                         // Index of the transaction in the block
    pub version: i32,                          // Transaction version
    pub transaction_type: TransactionType,     // Type of transaction
    pub transaction_status: TransactionStatus, // Status of the transaction
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlobDb {
    pub tx_hash: Vec<u8>,      // Corresponds to the transaction hash (BYTEA in SQL)
    pub blob_index: i32,       // Index of the blob within the transaction
    pub identity: String,      // Identity of the blob (TEXT in SQL)
    pub contract_name: String, // Contract name associated with the blob (TEXT in SQL)
    pub data: Vec<u8>,         // Actual blob data (BYTEA in SQL)
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct ProofTransactionDb {
    // Struct for the proof_transactions table
    pub tx_hash: Vec<u8>, // Corresponds to the transaction hash
    pub proof: Vec<u8>,   // Proof associated with the transaction
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlobReferenceDb {
    // Struct for the blob_references table
    pub id: i32,               // Unique ID for each blob reference
    pub tx_hash: Vec<u8>,      // Corresponds to the proof transaction hash
    pub contract_name: String, // Contract name
    pub blob_tx_hash: Vec<u8>, // Blob transaction hash
    pub blob_index: i32,       // Index of the blob
    // Optional field for extra data
    pub hyle_output: Option<serde_json::Value>, // Optional data in JSON format
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct ContractDb {
    // Struct for the contracts table
    pub tx_hash: Vec<u8>,    // Corresponds to the registration transaction hash
    pub owner: String,       // Owner of the contract
    pub verifier: String,    // Verifier of the contract
    pub program_id: Vec<u8>, // Program ID
    pub state_digest: Vec<u8>, // State digest of the contract
    pub contract_name: String, // Contract name
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct ContractStateDb {
    // Struct for the contract_state table
    pub contract_name: String, // Name of the contract
    pub block_hash: Vec<u8>,   // Hash of the block where the state is captured
    pub state_digest: Vec<u8>, // The contract state stored in JSON format
}
