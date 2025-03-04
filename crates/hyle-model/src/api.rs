use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use strum::IntoDiscriminant;
use utoipa::ToSchema;

use crate::{
    BlockHash, BlockHeight, ConsensusProposalHash, ContractName, DataProposalHash, Identity,
    LaneBytesSize, ProgramId, StateDigest, Transaction, TransactionKind, TxHash,
    ValidatorPublicKey, Verifier,
};

#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct NodeInfo {
    pub id: String,
    pub pubkey: Option<ValidatorPublicKey>,
    pub da_address: String,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct APIRegisterContract {
    pub verifier: Verifier,
    pub program_id: ProgramId,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}

/// Copy from Staking contract
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq)]
pub struct APIStaking {
    pub stakes: BTreeMap<Identity, u128>,
    pub delegations: BTreeMap<ValidatorPublicKey, Vec<Identity>>,
    /// When a validator distribute rewards, it is added in this list to
    /// avoid distributing twice the rewards for a same block
    pub rewarded: BTreeMap<ValidatorPublicKey, Vec<BlockHeight>>,

    /// List of validators that are part of consensus
    pub bonded: Vec<ValidatorPublicKey>,
    pub total_bond: u128,

    /// Struct to handle fees
    pub fees: APIFees,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq)]
pub struct APIFeesBalance {
    pub balance: i128,
    pub cumul_size: LaneBytesSize,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq)]
pub struct APIFees {
    /// Balance of each validator
    pub balances: BTreeMap<ValidatorPublicKey, APIFeesBalance>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq)]
pub enum TransactionTypeDb {
    BlobTransaction,
    ProofTransaction,
    RegisterContractTransaction,
    Stake,
}

#[cfg_attr(feature = "sqlx", derive(sqlx::Type))]
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "sqlx",
    sqlx(type_name = "transaction_status", rename_all = "snake_case")
)]
pub enum TransactionStatusDb {
    WaitingDissemination,
    DataProposalCreated,
    Success,
    Failure,
    Sequenced,
    TimedOut,
}

impl TransactionTypeDb {
    pub fn from(transaction: &Transaction) -> Self {
        transaction.transaction_data.discriminant().into()
    }
}

impl From<TransactionKind> for TransactionTypeDb {
    fn from(value: TransactionKind) -> Self {
        match value {
            TransactionKind::Blob => TransactionTypeDb::BlobTransaction,
            TransactionKind::Proof => TransactionTypeDb::ProofTransaction,
            TransactionKind::VerifiedProof => TransactionTypeDb::ProofTransaction,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq)]
pub struct APITransaction {
    // Struct for the transactions table
    pub tx_hash: TxHash,                           // Transaction hash
    pub parent_dp_hash: DataProposalHash,          // Data proposal hash
    pub version: u32,                              // Transaction version
    pub transaction_type: TransactionTypeDb,       // Type of transaction
    pub transaction_status: TransactionStatusDb,   // Status of the transaction
    pub block_hash: Option<ConsensusProposalHash>, // Corresponds to the block hash
    pub index: Option<u32>,                        // Index of the transaction within the block
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, PartialEq, Eq)]
pub struct APITransactionEvents {
    pub block_hash: BlockHash,
    pub block_height: BlockHeight,
    pub events: Vec<serde_json::Value>,
}

#[derive(Serialize, Deserialize, ToSchema, Debug, Clone, PartialEq)]
pub struct TransactionWithBlobs {
    pub tx_hash: TxHash,
    pub parent_dp_hash: DataProposalHash,
    pub block_hash: ConsensusProposalHash,
    pub index: u32,
    pub version: u32,
    pub transaction_type: TransactionTypeDb,
    pub transaction_status: TransactionStatusDb,
    pub identity: String,
    pub blobs: Vec<BlobWithStatus>,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BlobWithStatus {
    pub contract_name: String, // Contract name associated with the blob
    #[serde_as(as = "serde_with::hex::Hex")]
    pub data: Vec<u8>, // Actual blob data
    pub proof_outputs: Vec<serde_json::Value>, // outputs of proofs
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct APIContract {
    // Struct for the contracts table
    pub tx_hash: TxHash,  // Corresponds to the registration transaction hash
    pub verifier: String, // Verifier of the contract
    #[serde_as(as = "serde_with::hex::Hex")]
    pub program_id: Vec<u8>, // Program ID
    #[serde_as(as = "serde_with::hex::Hex")]
    pub state_digest: Vec<u8>, // State digest of the contract
    pub contract_name: String, // Contract name
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct APIContractState {
    // Struct for the contract_state table
    pub contract_name: String,             // Name of the contract
    pub block_hash: ConsensusProposalHash, // Hash of the block where the state is captured
    pub state_digest: Vec<u8>,             // The contract state stored in JSON format
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct APIBlob {
    pub tx_hash: TxHash,       // Corresponds to the transaction hash
    pub blob_index: u32,       // Index of the blob within the transaction
    pub identity: String,      // Identity of the blob
    pub contract_name: String, // Contract name associated with the blob
    #[serde_as(as = "serde_with::hex::Hex")]
    pub data: Vec<u8>, // Actual blob data
    pub verified: bool,        // Verification status
}
