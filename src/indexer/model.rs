use serde::{Deserialize, Serialize};
use sqlx::types::chrono::NaiveDateTime;
use sqlx::{prelude::Type, Postgres};

use crate::model::{Blob, BlockHash, Transaction, TransactionData};
use hyle_contract_sdk::TxHash;

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlockDb {
    // Struct for the blocks table
    pub hash: BlockHash,
    pub parent_hash: BlockHash, // Parent block hash
    #[sqlx(try_from = "i64")]
    pub height: u64, // Corresponds to BlockHeight
    pub timestamp: NaiveDateTime, // UNIX timestamp
}

#[derive(Debug, sqlx::Type, Serialize, Deserialize, Clone, PartialEq)]
#[sqlx(type_name = "transaction_type", rename_all = "snake_case")]
pub enum TransactionType {
    BlobTransaction,
    ProofTransaction,
    RegisterContractTransaction,
    Stake,
}

impl TransactionType {
    pub fn get_type_from_transaction(transaction: &Transaction) -> Self {
        match transaction.transaction_data {
            TransactionData::Blob(_) => TransactionType::BlobTransaction,
            TransactionData::Proof(_) => TransactionType::ProofTransaction,
            TransactionData::RegisterContract(_) => TransactionType::RegisterContractTransaction,
            TransactionData::Stake(_) => TransactionType::Stake,
        }
    }
}

#[derive(Debug, sqlx::Type, Serialize, Deserialize, Clone, PartialEq)]
#[sqlx(type_name = "transaction_status", rename_all = "snake_case")]
pub enum TransactionStatus {
    Success,
    Failure,
    Sequenced,
    TimedOut,
}
#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct TransactionDb {
    // Struct for the transactions table
    pub tx_hash: TxHashDb,     // Transaction hash
    pub block_hash: BlockHash, // Corresponds to the block hash
    pub tx_index: i32,         // Index of the transaction in the block
    #[sqlx(try_from = "i32")]
    pub version: u32, // Transaction version
    pub transaction_type: TransactionType, // Type of transaction
    pub transaction_status: TransactionStatus, // Status of the transaction
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TransactionWithBlobs {
    pub tx_hash: TxHashDb,
    pub block_hash: BlockHash,
    pub tx_index: i32,
    pub version: i32,
    pub transaction_type: TransactionType,
    pub transaction_status: TransactionStatus,
    pub identity: String,
    pub blobs: Vec<Blob>,
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlobDb {
    pub tx_hash: TxHashDb, // Corresponds to the transaction hash
    #[sqlx(try_from = "i32")]
    pub blob_index: u32, // Index of the blob within the transaction
    pub identity: String,  // Identity of the blob
    pub contract_name: String, // Contract name associated with the blob
    pub data: Vec<u8>,     // Actual blob data
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct ProofTransactionDb {
    // Struct for the proof_transactions table
    pub tx_hash: TxHashDb, // Corresponds to the transaction hash
    pub proof: Vec<u8>,    // Proof associated with the transaction
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct BlobReferenceDb {
    // Struct for the blob_references table
    pub tx_hash: TxHashDb,      // Corresponds to the proof transaction hash
    pub contract_name: String,  // Contract name
    pub blob_tx_hash: TxHashDb, // Blob transaction hash
    #[sqlx(try_from = "i32")]
    pub blob_index: u32, // Index of the blob
    // Optional field for extra data
    pub hyle_output: Option<serde_json::Value>, // Optional data in JSON format
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct ContractDb {
    // Struct for the contracts table
    pub tx_hash: TxHashDb,   // Corresponds to the registration transaction hash
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
    pub block_hash: BlockHash, // Hash of the block where the state is captured
    pub state_digest: Vec<u8>, // The contract state stored in JSON format
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TxHashDb(pub TxHash);

impl From<TxHash> for TxHashDb {
    fn from(tx_hash: TxHash) -> Self {
        TxHashDb(tx_hash)
    }
}

impl Type<Postgres> for TxHashDb {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }
}
impl<'q> sqlx::Encode<'q, sqlx::Postgres> for TxHashDb {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> std::result::Result<
        sqlx::encode::IsNull,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        <String as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0 .0, buf)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for TxHashDb {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> std::result::Result<
        TxHashDb,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        let inner = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(TxHashDb(TxHash(inner)))
    }
}
