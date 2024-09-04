use crate::model::{Block, Transaction};
use axum::{http::StatusCode, Json};
use serde::Deserialize;

pub async fn get_transaction(
    Json(payload): Json<TransactionRequest>,
) -> (StatusCode, Json<Transaction>) {
    let response: Transaction = Default::default();
    (StatusCode::OK, Json(response))
}

pub async fn get_block(Json(payload): Json<BlockRequest>) -> (StatusCode, Json<Block>) {
    let response: Block = Default::default();
    (StatusCode::OK, Json(response))
}

#[derive(Deserialize)]
pub struct TransactionRequest {
    tx_hash: String,
}

#[derive(Deserialize)]
pub struct BlockRequest {
    block_number: u64,
}
