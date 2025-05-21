use anyhow::Result;
use chrono::{DateTime, Utc};
use std::num::TryFromIntError;

use anyhow::Context;
use hyle_model::api::{
    APIBlob, APIBlock, APIContract, APIContractState, APITransaction, TransactionStatusDb,
    TransactionTypeDb,
};
use hyle_model::utils::TimestampMs;
use hyle_model::{ConsensusProposalHash, DataProposalHash};
use serde::{Deserialize, Serialize};

use sqlx::postgres::PgRow;
use sqlx::types::chrono::NaiveDateTime;
use sqlx::FromRow;
use sqlx::Row;
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
    #[sqlx(try_from = "i64")]
    pub total_txs: u64, // Total number of transactions in the block
}

impl From<BlockDb> for APIBlock {
    fn from(value: BlockDb) -> Self {
        APIBlock {
            hash: value.hash,
            parent_hash: value.parent_hash,
            height: value.height,
            timestamp: value.timestamp.and_utc().timestamp_millis(),
            total_txs: value.total_txs,
        }
    }
}

#[derive(Debug)]
pub struct TransactionDb {
    // Struct for the transactions table
    pub tx_hash: TxHashDb,                         // Transaction hash
    pub parent_dp_hash: DataProposalHash,          // Corresponds to the data proposal hash
    pub block_hash: Option<ConsensusProposalHash>, // Corresponds to the block hash
    pub index: Option<u32>,                        // Index of the transaction within the block
    pub version: u32,                              // Transaction version
    pub transaction_type: TransactionTypeDb,       // Type of transaction
    pub transaction_status: TransactionStatusDb,   // Status of the transaction
    pub timestamp: Option<NaiveDateTime>,          // Timestamp of the transaction (block timestamp)
}

impl<'r> FromRow<'r, PgRow> for TransactionDb {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        let tx_hash = row.try_get("tx_hash")?;
        let block_hash = row.try_get("block_hash")?;
        let dp_hash_db: DataProposalHashDb = row.try_get("parent_dp_hash")?;
        let parent_dp_hash = dp_hash_db.0;
        let index: Option<i32> = row.try_get("index")?;
        let version: i32 = row.try_get("version")?;
        let version: u32 = version
            .try_into()
            .map_err(|e: TryFromIntError| sqlx::Error::Decode(e.into()))?;
        let transaction_type: TransactionTypeDb = row.try_get("transaction_type")?;
        let transaction_status: TransactionStatusDb = row.try_get("transaction_status")?;
        let index = match index {
            None => None,
            Some(index) => Some(
                index
                    .try_into()
                    .map_err(|e: TryFromIntError| sqlx::Error::Decode(e.into()))?,
            ),
        };
        let timestamp: Option<NaiveDateTime> = row.try_get("timestamp")?;
        Ok(TransactionDb {
            tx_hash,
            parent_dp_hash,
            block_hash,
            index,
            version,
            transaction_type,
            transaction_status,
            timestamp,
        })
    }
}

impl From<TransactionDb> for APITransaction {
    fn from(val: TransactionDb) -> Self {
        let timestamp = val
            .timestamp
            .map(|t| TimestampMs(t.and_utc().timestamp_millis() as u128));
        APITransaction {
            tx_hash: val.tx_hash.0,
            parent_dp_hash: val.parent_dp_hash,
            block_hash: val.block_hash,
            index: val.index,
            version: val.version,
            transaction_type: val.transaction_type,
            transaction_status: val.transaction_status,
            timestamp,
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
    pub proof_outputs: Vec<serde_json::Value>, // outputs of proofs
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
            proof_outputs: value.proof_outputs,
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
    pub state_commitment: Vec<u8>, // state commitment of the contract
    pub contract_name: String, // Contract name
    #[sqlx(try_from = "i64")]
    pub total_tx: u64, // Total number of transactions associated with the contract
    #[sqlx(try_from = "i64")]
    pub unsettled_tx: u64, // Total number of unsettled transactions
    pub earliest_unsettled: Option<i64>, // Block height of the earliest unsettled transaction
}

impl From<ContractDb> for APIContract {
    fn from(val: ContractDb) -> Self {
        APIContract {
            tx_hash: val.tx_hash.0,
            verifier: val.verifier,
            program_id: val.program_id,
            state_commitment: val.state_commitment,
            contract_name: val.contract_name,
            total_tx: val.total_tx,
            unsettled_tx: val.unsettled_tx,
            earliest_unsettled: val.earliest_unsettled.map(|a| a as u64),
        }
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct ContractStateDb {
    // Struct for the contract_state table
    pub contract_name: String,             // Name of the contract
    pub block_hash: ConsensusProposalHash, // Hash of the block where the state is captured
    pub state_commitment: Vec<u8>,         // The contract state stored in JSON format
}

impl From<ContractStateDb> for APIContractState {
    fn from(value: ContractStateDb) -> Self {
        APIContractState {
            contract_name: value.contract_name,
            block_hash: value.block_hash,
            state_commitment: value.state_commitment,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct DataProposalHashDb(pub DataProposalHash);

impl From<DataProposalHash> for DataProposalHashDb {
    fn from(dp_hash: DataProposalHash) -> Self {
        DataProposalHashDb(dp_hash)
    }
}

impl Type<Postgres> for DataProposalHashDb {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }
}
impl sqlx::Encode<'_, sqlx::Postgres> for DataProposalHashDb {
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

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for DataProposalHashDb {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> std::result::Result<
        DataProposalHashDb,
        std::boxed::Box<(dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static)>,
    > {
        let inner = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(DataProposalHashDb(DataProposalHash(inner)))
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

pub fn into_utc_date_time(ts: &TimestampMs) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ts.0.try_into().context("Converting u64 into i64")?)
        .context("Converting i64 into UTC DateTime")
}
