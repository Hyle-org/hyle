use crate::{
    history::{BlocksFilter, ContractsFilter, TransactionsFilter},
    model::BlockHeight,
    rest::RouterState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

pub async fn get_transaction(
    Path(tx_hash): Path<String>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.transactions.get(&tx_hash) {
        Ok(Some(tx)) => Ok(Json(tx)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_block(
    Path(height): Path<BlockHeight>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.blocks.get(height) {
        Ok(Some(block)) => Ok(Json(block)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_blocks(
    Query(filter): Query<BlocksFilter>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.blocks.search(&filter) {
        Ok(blocks) => Ok(Json(blocks)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_last_block(
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.blocks.last.clone() {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_proof(
    Path(tx_hash): Path<String>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.proofs.get(&tx_hash) {
        Ok(Some(proof)) => Ok(Json(proof)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_blobs(
    Path(tx_hash): Path<String>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.blobs.get_from_tx_hash(&tx_hash) {
        Ok(blob) => Ok(Json(blob)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_blob(
    Path(tx_hash): Path<String>,
    Path(blob_index): Path<usize>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.blobs.get(&tx_hash, blob_index) {
        Ok(Some(blob)) => Ok(Json(blob)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_transactions(
    Query(filter): Query<TransactionsFilter>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.transactions.search(&filter) {
        Ok(transactions) => Ok(Json(transactions)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_transaction2(
    Path(block_height): Path<BlockHeight>,
    Path(tx_index): Path<usize>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .history
        .lock()
        .await
        .transactions
        .get_with_height_and_index(block_height, tx_index)
    {
        Ok(Some(transactions)) => Ok(Json(transactions)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_contracts(
    Query(filter): Query<ContractsFilter>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.contracts.search(&filter) {
        Ok(contract) => Ok(Json(contract)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_contracts2(
    Path(name): Path<String>,
    Query(filter): Query<ContractsFilter>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .history
        .lock()
        .await
        .contracts
        .search_by_name(&name, &filter)
    {
        Ok(contract) => Ok(Json(contract)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_contract2(
    Path(name): Path<String>,
    Path(tx_hash): Path<String>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.history.lock().await.contracts.get(&name, &tx_hash) {
        Ok(Some(contract)) => Ok(Json(contract)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
