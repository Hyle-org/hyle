use crate::bus::command_response::CmdRespClient;
use crate::bus::BusClientSender;
use crate::bus::BusMessage;
use crate::consensus::staking::Staker;
use crate::data_availability::QueryBlockHeight;
use crate::model::ProofData;
use crate::model::Transaction;
use crate::model::{BlobTransaction, ContractName};
use crate::model::{Hashable, ProofTransaction, RegisterContractTransaction, TransactionData};
use crate::tools::mock_workflow::RunScenario;
use anyhow::anyhow;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use bincode::{Decode, Encode};
use hyle_contract_sdk::TxHash;
use serde::{Deserialize, Serialize};
use tracing::error;

use super::{AppError, RouterState};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum RestApiMessage {
    NewTx(Transaction),
}
impl BusMessage for RestApiMessage {}

async fn handle_send(
    mut state: RouterState,
    payload: TransactionData,
) -> Result<Json<TxHash>, StatusCode> {
    let tx = Transaction::wrap(payload);
    let tx_hash = tx.hash();
    state
        .bus
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

/// # Curl example
/// ```bash
/// curl -X POST --location 'http://localhost:4321/v1/tx/send/blob' \
///     --header 'Content-Type: application/json' \
///     --data '{
///         "identity": "ident",
///         "blobs": [
///             {
///                 "contract_name": "contrat de test",
///                 "data": []
///             }
///         ]
///     }'
/// ```
/// # Example decoding
/// ```
/// use hyle::model::{Blob, BlobData, BlobTransaction, ContractName, Identity};
///
/// let payload_json = r#"
///  {
///     "identity": "ident",
///     "blobs": [
///         {
///             "contract_name": "contrat de test",
///             "data": []
///         }
///     ]
/// }
///  "#;
/// let decoded: BlobTransaction = serde_json::from_str(payload_json).unwrap();
/// assert_eq!(decoded.identity, Identity("ident".to_string()));
/// ```
///
/// # Example encoding
/// ```
/// use hyle::model::{Blob, BlobData, BlobTransaction, ContractName, Identity};
///
/// let payload = BlobTransaction {
///     identity: Identity("tata".to_string()),
///     blobs: vec![Blob {
///         contract_name: ContractName("contract_name".to_string()),
///         data: BlobData(vec![]),
///     }],
/// };
///
/// let encoded = serde_json::to_string(&payload).unwrap();
///
/// assert_eq!(
///     encoded,
///     "{\"identity\":\"tata\",\"blobs\":[{\"contract_name\":\"contract_name\",\"data\":[]}]}"
///         .to_string()
/// );
/// ```
pub async fn send_blob_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<BlobTransaction>,
) -> Result<impl IntoResponse, StatusCode> {
    handle_send(state, TransactionData::Blob(payload)).await
}

pub async fn send_proof_transaction(
    State(state): State<RouterState>,
    Json(mut payload): Json<ProofTransaction>,
) -> Result<impl IntoResponse, StatusCode> {
    let proof_bytes = payload
        .proof
        .to_bytes()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    payload.proof = ProofData::Bytes(proof_bytes);
    handle_send(state, TransactionData::Proof(payload)).await
}

pub async fn send_staking_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<Staker>,
) -> Result<impl IntoResponse, StatusCode> {
    handle_send(state, TransactionData::Stake(payload)).await
}

pub async fn get_contract(
    Path(name): Path<ContractName>,
    State(mut state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    let name_clone = name.clone();
    match state.bus.request(name).await {
        Ok(contract) => Ok(Json(contract)),
        err => {
            error!("{:?}", err);

            Err(AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Error while getting contract {}", name_clone),
            ))
        }
    }
}

pub async fn get_block_height(
    State(mut state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    match state.bus.request(QueryBlockHeight {}).await {
        Ok(block_height) => Ok(Json(block_height)),
        err => {
            error!("{:?}", err);

            Err(AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Error while getting block height"),
            ))
        }
    }
}

pub async fn run_scenario(
    State(mut state): State<RouterState>,
    Json(scenario): Json<RunScenario>,
) -> Result<impl IntoResponse, StatusCode> {
    state
        .bus
        .send(scenario)
        .map(|_| StatusCode::OK)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
