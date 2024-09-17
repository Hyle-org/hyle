use crate::model::Transaction;
use crate::tools::mock_workflow::RunScenario;
use crate::{
    bus::command_response::CmdRespClient,
    indexer::model::TransactionOwned,
    model::{
        BlobTransaction, BlockHeight, ContractName, Hashable, ProofTransaction,
        RegisterContractTransaction, TransactionData, TxHash,
    },
    node_state::{NodeStateQuery, NodeStateQueryResponse},
};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use anyhow::anyhow;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use super::{AppError, RouterState};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum RestApiMessage {
    NewTx(Transaction),
}

async fn handle_send(
    state: RouterState,
    payload: TransactionData,
) -> Result<Json<TxHash>, StatusCode> {
    let tx = Transaction::wrap(payload);
    let tx_hash = tx.hash();
    state
        .bus
        .sender::<RestApiMessage>()
        .await
        .send(RestApiMessage::NewTx(tx))
        .map(|_| tx_hash)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn send_contract_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<RegisterContractTransaction>,
) -> Result<impl IntoResponse, StatusCode> {
    handle_send(state, TransactionData::RegisterContract(payload)).await
}

pub async fn send_blob_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<BlobTransaction>,
) -> Result<impl IntoResponse, StatusCode> {
    handle_send(state, TransactionData::Blob(payload)).await
}

pub async fn send_proof_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<ProofTransaction>,
) -> Result<impl IntoResponse, StatusCode> {
    handle_send(state, TransactionData::Proof(payload)).await
}

pub async fn get_transaction(
    Path(tx_hash): Path<String>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.idxr.lock().await.get_tx(&tx_hash) {
        Ok(Some(tx)) => Ok(Json(TransactionOwned::from(tx))),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_block(
    Path(height): Path<BlockHeight>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.idxr.lock().await.get_block(height) {
        Ok(Some(block)) => Ok(Json(block)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_current_block(
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.idxr.lock().await.last_block() {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_contract(
    Path(name): Path<ContractName>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    let name_clone = name.clone();
    if let Some(res) = state
        .bus
        .request(NodeStateQuery::GetContract { name })
        .await?
    {
        match res {
            NodeStateQueryResponse::Contract { contract } => Ok(Json(contract)),
        }
    } else {
        Err(AppError(
            StatusCode::NOT_FOUND,
            anyhow!("Contract {} not found", name_clone),
        ))
    }
}

pub async fn run_scenario(
    State(state): State<RouterState>,
    Json(scenario): Json<RunScenario>,
) -> Result<impl IntoResponse, StatusCode> {
    state
        .bus
        .sender::<RunScenario>()
        .await
        .send(scenario)
        .map(|_| StatusCode::OK)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
