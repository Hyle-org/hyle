use crate::model::{Block, Transaction};
use axum::{http::StatusCode, Json};

use super::model::{BlockRequest, TransactionRequest};

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
