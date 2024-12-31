use anyhow::anyhow;
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use bincode::{Decode, Encode};
use hyle_contract_sdk::TxHash;
use serde::{Deserialize, Serialize};

use crate::{
    bus::{bus_client, metrics::BusMetrics, BusClientSender, BusMessage},
    model::{
        BlobTransaction, CommonRunContext, Hashable, ProofData, ProofTransaction,
        RegisterContractTransaction, Transaction, TransactionData,
    },
    rest::AppError,
};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum RestApiMessage {
    NewTx(Transaction),
}
impl BusMessage for RestApiMessage {}

bus_client! {
struct RestBusClient {
    sender(RestApiMessage),
}
}

pub struct RouterState {
    bus: RestBusClient,
}

pub async fn api(ctx: &CommonRunContext) -> Router<()> {
    let state = RouterState {
        bus: RestBusClient::new_from_bus(ctx.bus.new_handle()).await,
    };

    Router::new()
        .route("/contract/register", post(send_contract_transaction))
        .route("/tx/send/blob", post(send_blob_transaction))
        .route("/tx/send/proof", post(send_proof_transaction))
        .with_state(state)
}

async fn handle_send(
    mut state: RouterState,
    payload: TransactionData,
) -> Result<Json<TxHash>, AppError> {
    let tx = Transaction::wrap(payload);
    let tx_hash = tx.hash();
    state
        .bus
        .send(RestApiMessage::NewTx(tx))
        .map(|_| tx_hash)
        .map(Json)
        .map_err(|err| AppError(StatusCode::INTERNAL_SERVER_ERROR, anyhow!(err)))
}

pub async fn send_contract_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<RegisterContractTransaction>,
) -> Result<impl IntoResponse, AppError> {
    handle_send(state, TransactionData::RegisterContract(payload)).await
}

pub async fn send_blob_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<BlobTransaction>,
) -> Result<impl IntoResponse, AppError> {
    handle_send(state, TransactionData::Blob(payload)).await
}

pub async fn send_proof_transaction(
    State(state): State<RouterState>,
    Json(mut payload): Json<ProofTransaction>,
) -> Result<impl IntoResponse, AppError> {
    let proof_bytes = payload
        .proof
        .to_bytes()
        .map_err(|err| AppError(StatusCode::BAD_REQUEST, anyhow!(err)))?;
    payload.proof = ProofData::Bytes(proof_bytes);
    handle_send(state, TransactionData::Proof(payload)).await
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        use crate::utils::static_type_map::Pick;
        Self {
            bus: RestBusClient::new(
                Pick::<BusMetrics>::get(&self.bus).clone(),
                Pick::<tokio::sync::broadcast::Sender<RestApiMessage>>::get(&self.bus).clone(),
            ),
        }
    }
}
