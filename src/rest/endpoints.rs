use crate::{
    bus::command_response::CmdRespClient,
    model::{
        BlobTransaction, BlockHeight, ContractName, Hashable, ProofTransaction,
        RegisterContractTransaction, Transaction, TransactionData, TxHash,
    },
    node_state::{NodeStateQuery, NodeStateQueryResponse},
    p2p::network::{MempoolNetMessage, OutboundMessage},
};
use anyhow::anyhow;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use super::{AppError, RouterState};

async fn handle_send(
    state: RouterState,
    payload: TransactionData,
) -> Result<Json<TxHash>, StatusCode> {
    let tx = Transaction::wrap(payload);
    let tx_hash = tx.hash();
    state
        .bus
        .sender::<OutboundMessage>()
        .await
        .send(OutboundMessage::broadcast(MempoolNetMessage::NewTx(
            tx.clone(),
        )))
        .map(|_| ())
        .ok();
    state
        .bus
        .sender::<MempoolNetMessage>()
        .await
        .send(MempoolNetMessage::NewTx(tx))
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
    state
        .idxr
        .lock()
        .await
        .get_tx(&tx_hash)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

pub async fn get_block(
    Path(height): Path<BlockHeight>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    state
        .idxr
        .lock()
        .await
        .get_block(&height)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

pub async fn get_current_block(
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, StatusCode> {
    state
        .idxr
        .lock()
        .await
        .last_block()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
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
