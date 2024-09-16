use crate::{indexer::Indexer, model::BlockHeight};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

pub async fn get_transaction(
    Path(tx_hash): Path<String>,
    State(idxr): State<Indexer>,
) -> Result<impl IntoResponse, StatusCode> {
    idxr.lock()
        .await
        .get_tx(&tx_hash)
        .map(Json)
        .ok_or_else(|| StatusCode::NOT_FOUND)
}

pub async fn get_block(
    Path(height): Path<BlockHeight>,
    State(idxr): State<Indexer>,
) -> Result<impl IntoResponse, StatusCode> {
    idxr.lock()
        .await
        .get_block(&height)
        .map(Json)
        .ok_or_else(|| StatusCode::NOT_FOUND)
}
