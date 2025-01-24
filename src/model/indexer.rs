use hyle_model::api::{
    APIBlob, APIBlock, APIContract, APIContractState, APITransaction, TransactionStatus,
    TransactionType,
};
use hyle_model::ConsensusProposalHash;
use serde::{Deserialize, Serialize};

use sqlx::types::chrono::NaiveDateTime;
use sqlx::{prelude::Type, Postgres};

use hyle_contract_sdk::TxHash;

#[derive(sqlx::FromRow, Debug)]
pub struct BlockDb {
    // Struct for the blocks table
    pub hash: ConsensusProposalHash,
    pub parent_hash: ConsensusProposalHash,
    #[sqlx(try_from = "i64")]
    pub height: u64, // Corresponds to BlockHeight
    pub timestamp: NaiveDateTime, // UNIX timestamp
}

impl From<BlockDb> for APIBlock {
    fn from(value: BlockDb) -> Self {
        APIBlock {
            hash: value.hash,
            parent_hash: value.parent_hash,
            height: value.height,
            timestamp: value.timestamp.and_utc().timestamp(),
        }
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct TransactionDb {
    // Struct for the transactions table
    pub tx_hash: TxHashDb,                 // Transaction hash
    pub block_hash: ConsensusProposalHash, // Corresponds to the block hash
    #[sqlx(try_from = "i32")]
    pub index: u32, // Index of the transaction within the block
    #[sqlx(try_from = "i32")]
    pub version: u32, // Transaction version
    pub transaction_type: TransactionType, // Type of transaction
    pub transaction_status: TransactionStatus, // Status of the transaction
}

impl From<TransactionDb> for APITransaction {
    fn from(val: TransactionDb) -> Self {
        APITransaction {
            tx_hash: val.tx_hash.0,
            block_hash: val.block_hash,
            index: val.index,
            version: val.version,
            transaction_type: val.transaction_type,
            transaction_status: val.transaction_status,
        }
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct BlobDb {
    pub tx_hash: TxHashDb, // Corresponds to the transaction hash
    #[sqlx(try_from = "i32")]
    pub blob_index: u32, // Index of the blob within the transaction
    pub identity: String,  // Identity of the blob
    pub contract_name: String, // Contract name associated with the blob
    pub data: Vec<u8>,     // Actual blob data
    pub verified: bool,    // Verification status
}

impl From<BlobDb> for APIBlob {
    fn from(value: BlobDb) -> Self {
        APIBlob {
            tx_hash: value.tx_hash.0,
            blob_index: value.blob_index,
            identity: value.identity,
            contract_name: value.contract_name,
            data: value.data,
            verified: value.verified,
        }
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct ProofTransactionDb {
    // Struct for the proof_transactions table
    pub tx_hash: TxHashDb,     // Corresponds to the transaction hash
    pub contract_name: String, // Contract name associated with the proof
    pub proof: Vec<u8>,        // Proof associated with the transaction
}

#[derive(sqlx::FromRow, Debug)]
pub struct ContractDb {
    // Struct for the contracts table
    pub tx_hash: TxHashDb,   // Corresponds to the registration transaction hash
    pub verifier: String,    // Verifier of the contract
    pub program_id: Vec<u8>, // Program ID
    pub state_digest: Vec<u8>, // State digest of the contract
    pub contract_name: String, // Contract name
}

impl From<ContractDb> for APIContract {
    fn from(val: ContractDb) -> Self {
        APIContract {
            tx_hash: val.tx_hash.0,
            verifier: val.verifier,
            program_id: val.program_id,
            state_digest: val.state_digest,
            contract_name: val.contract_name,
        }
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct ContractStateDb {
    // Struct for the contract_state table
    pub contract_name: String,             // Name of the contract
    pub block_hash: ConsensusProposalHash, // Hash of the block where the state is captured
    pub state_digest: Vec<u8>,             // The contract state stored in JSON format
}

impl From<ContractStateDb> for APIContractState {
    fn from(value: ContractStateDb) -> Self {
        APIContractState {
            contract_name: value.contract_name,
            block_hash: value.block_hash,
            state_digest: value.state_digest,
        }
    }
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
impl sqlx::Encode<'_, sqlx::Postgres> for TxHashDb {
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
