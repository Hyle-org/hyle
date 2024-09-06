use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct TransactionRequest {
    pub tx_hash: String,
}

#[derive(Serialize, Deserialize)]
pub struct BlockRequest {
    pub block_number: u64,
}
