use crate::model::{Block, Transaction};
use axum::{http::StatusCode, Json};
use serde::{Deserialize, Serialize};

pub async fn get_transaction(
    Json(_payload): Json<TransactionRequest>,
) -> (StatusCode, Json<Transaction>) {
    let response: Transaction = Default::default();
    (StatusCode::OK, Json(response))
}

pub async fn get_block(Json(_payload): Json<BlockRequest>) -> (StatusCode, Json<Block>) {
    let response: Block = Default::default();
    (StatusCode::OK, Json(response))
}

#[derive(Serialize, Deserialize)]
pub struct TransactionRequest {
    pub tx_hash: String,
}

#[derive(Serialize, Deserialize)]
pub struct BlockRequest {
    pub block_number: u64,
}
