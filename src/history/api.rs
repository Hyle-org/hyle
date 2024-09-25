use super::{
    blobs::BlobsKey,
    blocks::BlocksKey,
    contracts::ContractsKey,
    model::{Blob, Contract, Proof, Transaction},
    proofs::ProofsKey,
    transactions::TransactionsKey,
};
use crate::{
    model::{Block, BlockHeight},
    rest::RouterState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{de::DeserializeOwned, Deserialize};
use tracing::error;

#[derive(Deserialize, Debug)]
pub struct Filters {
    pub skip: Option<usize>,
    pub reverse: Option<bool>,
    pub limit: Option<usize>,
}

fn filter_iter<T: DeserializeOwned>(iter: crate::history::db::Iter<T>, filters: Filters) -> Vec<T> {
    // from the start
    if !filters.reverse.unwrap_or(false) {
        iter.rev() // just for this !
            .skip(filters.skip.unwrap_or(0))
            .take(filters.limit.unwrap_or(10))
            .flat_map(|i| {
                i.map(|i| i.value().map_err(|e| error!("deserializing  data: {}", e)))
                    .map_err(|e| error!("iterating over data: {}", e))
            })
            .filter_map(|i| i.ok())
            .collect::<Vec<T>>()
    } else {
        iter.skip(filters.skip.unwrap_or(0))
            .take(filters.limit.unwrap_or(10))
            .flat_map(|i| {
                i.map(|i| i.value().map_err(|e| error!("deserializing  data: {}", e)))
                    .map_err(|e| error!("iterating over data: {}", e))
            })
            .filter_map(|i| i.ok())
            .collect::<Vec<T>>()
    }
}

pub async fn get_blocks(
    Query(filters): Query<Filters>,
    State(state): State<RouterState>,
) -> Result<Json<Vec<Block>>, StatusCode> {
    let last_height = state.history.read().await.blocks.last().height;
    Ok(Json(filter_iter(
        state
            .history
            .write()
            .await
            .blocks
            .range(BlocksKey(BlockHeight(0)), BlocksKey(last_height)),
        filters,
    )))
}

pub async fn get_block(
    Path(height): Path<BlockHeight>,
    State(state): State<RouterState>,
) -> Result<Json<Block>, StatusCode> {
    match state.history.write().await.blocks.get(height) {
        Ok(Some(block)) => Ok(Json(block)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_last_block(State(state): State<RouterState>) -> Result<Json<Block>, StatusCode> {
    Ok(Json(state.history.read().await.blocks.last().clone()))
}

pub async fn get_proofs(
    Query(filters): Query<Filters>,
    State(state): State<RouterState>,
) -> Result<Json<Vec<Proof>>, StatusCode> {
    let last = match state.history.read().await.proofs.last() {
        Ok(Some(proof)) => proof,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    Ok(Json(filter_iter(
        state.history.write().await.proofs.range(
            ProofsKey(BlockHeight(0), 0),
            ProofsKey(last.block_height, last.tx_index),
        ),
        filters,
    )))
}

pub async fn get_last_proof(State(state): State<RouterState>) -> Result<Json<Proof>, StatusCode> {
    match state.history.read().await.proofs.last() {
        Ok(Some(proof)) => Ok(Json(proof)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_proof(
    Path((block_height, tx_index)): Path<(BlockHeight, usize)>,
    State(state): State<RouterState>,
) -> Result<Json<Proof>, StatusCode> {
    match state
        .history
        .write()
        .await
        .proofs
        .get(block_height, tx_index)
    {
        Ok(Some(proof)) => Ok(Json(proof)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_proof_with_hash(
    Path(tx_hash): Path<String>,
    State(state): State<RouterState>,
) -> Result<Json<Proof>, StatusCode> {
    match state.history.write().await.proofs.get_with_hash(&tx_hash) {
        Ok(Some(proof)) => Ok(Json(proof)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_blobs(
    Query(filters): Query<Filters>,
    State(state): State<RouterState>,
) -> Result<Json<Vec<Blob>>, StatusCode> {
    let blob = match state.history.read().await.blobs.last() {
        Ok(Some(blob)) => blob,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    Ok(Json({
        filter_iter(
            state.history.write().await.blobs.range(
                BlobsKey(BlockHeight(0), 0, 0),
                BlobsKey(blob.block_height, blob.tx_index, blob.blob_index),
            ),
            filters,
        )
    }))
}

pub async fn get_last_blob(State(state): State<RouterState>) -> Result<Json<Blob>, StatusCode> {
    match state.history.read().await.blobs.last() {
        Ok(Some(blob)) => Ok(Json(blob)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_blob(
    Path((block_height, tx_index, blob_index)): Path<(BlockHeight, usize, usize)>,
    State(state): State<RouterState>,
) -> Result<Json<Blob>, StatusCode> {
    match state
        .history
        .write()
        .await
        .blobs
        .get(block_height, tx_index, blob_index)
    {
        Ok(Some(blob)) => Ok(Json(blob)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_blob_with_hash(
    Path((tx_hash, blob_index)): Path<(String, usize)>,
    State(state): State<RouterState>,
) -> Result<Json<Blob>, StatusCode> {
    match state
        .history
        .write()
        .await
        .blobs
        .get_with_hash(&tx_hash, blob_index)
    {
        Ok(Some(blob)) => Ok(Json(blob)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_transactions(
    Query(filters): Query<Filters>,
    State(state): State<RouterState>,
) -> Result<Json<Vec<Transaction>>, StatusCode> {
    let (last_height, txs_len) = {
        let blocks = &state.history.read().await.blocks;
        let b = blocks.last();
        (b.height, b.txs.len())
    };
    Ok(Json({
        filter_iter(
            state.history.write().await.transactions.range(
                TransactionsKey(BlockHeight(0), 0),
                TransactionsKey(last_height, txs_len),
            ),
            filters,
        )
    }))
}

pub async fn get_last_transaction(
    State(state): State<RouterState>,
) -> Result<Json<Transaction>, StatusCode> {
    match state.history.read().await.transactions.last() {
        Ok(Some(transaction)) => Ok(Json(transaction)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_transaction(
    Path((block_height, tx_index)): Path<(BlockHeight, usize)>,
    State(state): State<RouterState>,
) -> Result<Json<Transaction>, StatusCode> {
    match state
        .history
        .write()
        .await
        .transactions
        .get(block_height, tx_index)
    {
        Ok(Some(transactions)) => Ok(Json(transactions)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_transaction_with_hash(
    Path(tx_hash): Path<String>,
    State(state): State<RouterState>,
) -> Result<Json<Transaction>, StatusCode> {
    match state
        .history
        .write()
        .await
        .transactions
        .get_with_hash(&tx_hash)
    {
        Ok(Some(tx)) => Ok(Json(tx)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_last_contract(
    State(state): State<RouterState>,
) -> Result<Json<Contract>, StatusCode> {
    match state.history.read().await.contracts.last() {
        Ok(Some(contract)) => Ok(Json(contract)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_contracts(
    Query(filters): Query<Filters>,
    State(state): State<RouterState>,
) -> Result<Json<Vec<Contract>>, StatusCode> {
    let contract = match state.history.read().await.contracts.last() {
        Ok(Some(contract)) => contract,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    Ok(Json({
        filter_iter(
            state.history.write().await.contracts.range(
                ContractsKey(BlockHeight(0), 0),
                ContractsKey(contract.block_height, contract.tx_index),
            ),
            filters,
        )
    }))
}

pub async fn get_contract(
    Path((block_height, tx_index)): Path<(BlockHeight, usize)>,
    State(state): State<RouterState>,
) -> Result<Json<Contract>, StatusCode> {
    match state
        .history
        .write()
        .await
        .contracts
        .get(block_height, tx_index)
    {
        Ok(Some(contract)) => Ok(Json(contract)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_contracts_with_name(
    Query(limit): Query<Option<usize>>,
    Path(name): Path<String>,
    State(state): State<RouterState>,
) -> Result<Json<Vec<Contract>>, StatusCode> {
    match state.history.write().await.contracts.get_with_name(&name) {
        Some(contracts) => Ok(Json(
            contracts
                .take(limit.map(|l| if l > 50 { 50 } else { l }).unwrap_or(10))
                .flat_map(|i| {
                    i.map(|i| i.value().map_err(|e| error!("deserializing  data: {}", e)))
                        .map_err(|e| error!("iterating over data: {}", e))
                })
                .filter_map(|i| i.ok())
                .collect(),
        )),
        None => Err(StatusCode::NOT_FOUND),
    }
}
