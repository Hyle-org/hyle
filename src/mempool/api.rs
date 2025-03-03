use anyhow::anyhow;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json, Router};
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_contract_sdk::TxHash;
use hyle_model::{
    api::APIRegisterContract, ContractAction, RegisterContractAction, StructuredBlobData,
};
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    bus::{bus_client, metrics::BusMetrics, BusClientSender, BusMessage},
    model::{
        contract_registration::validate_contract_registration_metadata, BlobTransaction,
        CommonRunContext, Hashed, ProofTransaction, Transaction, TransactionData,
    },
    rest::AppError,
};

#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize)]
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

#[derive(OpenApi)]
struct MempoolAPI;

pub async fn api(ctx: &CommonRunContext) -> Router<()> {
    let state = RouterState {
        bus: RestBusClient::new_from_bus(ctx.bus.new_handle()).await,
    };

    let (router, api) = OpenApiRouter::with_openapi(MempoolAPI::openapi())
        .routes(routes!(register_contract))
        .routes(routes!(send_blob_transaction))
        .routes(routes!(send_proof_transaction))
        .split_for_parts();

    if let Ok(mut o) = ctx.openapi.lock() {
        *o = o.clone().nest("/v1", api);
    }
    router.with_state(state)
}

async fn handle_send(
    mut state: RouterState,
    payload: TransactionData,
) -> Result<Json<TxHash>, AppError> {
    let tx: Transaction = payload.into();
    let tx_hash = tx.hashed();
    state
        .bus
        .send(RestApiMessage::NewTx(tx))
        .map(|_| tx_hash)
        .map(Json)
        .map_err(|err| AppError(StatusCode::INTERNAL_SERVER_ERROR, anyhow!(err)))
}

#[utoipa::path(
    post,
    path = "/tx/send/blob",
    tag = "Mempool",
    responses(
        (status = OK, description = "Send blob transaction", body = TxHash)
    )
)]
pub async fn send_blob_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<BlobTransaction>,
) -> Result<impl IntoResponse, AppError> {
    info!("Got blob transaction {}", payload.hashed());

    // Filter out incorrect contract-registring transactions
    for blob in payload.blobs.iter() {
        if blob.contract_name.0 != "hyle" {
            continue;
        }
        if let Ok(tx) = StructuredBlobData::<RegisterContractAction>::try_from(blob.data.clone()) {
            let parameters = tx.parameters;
            validate_contract_registration_metadata(
                &"hyle".into(),
                &parameters.contract_name,
                &parameters.verifier,
                &parameters.program_id,
                &parameters.state_digest,
            )
            .map_err(|err| AppError(StatusCode::BAD_REQUEST, anyhow!(err)))?;
        }
    }

    // Filter out transactions with incorrect identity
    if let Err(e) = payload.validate_identity() {
        return Err(AppError(
            StatusCode::BAD_REQUEST,
            anyhow!("Invalid identity for blob tx: {}", e),
        ));
    }
    handle_send(state, TransactionData::Blob(payload)).await
}

#[utoipa::path(
    post,
    path = "/tx/send/proof",
    tag = "Mempool",
    responses(
        (status = OK, description = "Send proof transaction", body = TxHash)
    )
)]
pub async fn send_proof_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<ProofTransaction>,
) -> Result<impl IntoResponse, AppError> {
    info!("Got proof transaction {}", payload.hashed());
    handle_send(state, TransactionData::Proof(payload)).await
}

#[utoipa::path(
    post,
    path = "/contract/register",
    tag = "Mempool",
    responses(
        (status = OK, description = "Register contract", body = TxHash)
    )
)]
pub async fn register_contract(
    State(state): State<RouterState>,
    Json(payload): Json<APIRegisterContract>,
) -> Result<impl IntoResponse, AppError> {
    let owner = "hyle".into();
    validate_contract_registration_metadata(
        &owner,
        &payload.contract_name,
        &payload.verifier,
        &payload.program_id,
        &payload.state_digest,
    )
    .map_err(|err| AppError(StatusCode::BAD_REQUEST, anyhow!(err)))?;

    let tx = BlobTransaction::new(
        "hyle.hyle",
        vec![RegisterContractAction {
            verifier: payload.verifier,
            program_id: payload.program_id,
            state_digest: payload.state_digest,
            contract_name: payload.contract_name,
        }
        .as_blob(owner, None, None)],
    );

    handle_send(state, TransactionData::Blob(tx)).await
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
