use crate::{AccountInfo, Hydentity};
use anyhow::{anyhow, Context, Result};
use client_sdk::contract_indexer::{
    axum::Router,
    utoipa::openapi::OpenApi,
    utoipa_axum::{router::OpenApiRouter, routes},
    ContractHandler, RwLock, Store,
};
use client_sdk::contract_indexer::{
    axum::{
        extract::{Path, State},
        http::StatusCode,
        response::IntoResponse,
        Json,
    },
    utoipa::{self, ToSchema},
    AppError,
};
use sdk::{
    identity_provider::{IdentityAction, IdentityVerification},
    tracing::info,
    Blob, BlobIndex, BlobTransaction, Identity,
};
use serde::Serialize;
use std::sync::Arc;

pub mod metadata {
    pub const HYDENTITY_ELF: &[u8] = include_bytes!("../hydentity.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../hydentity.txt"));
}

use client_sdk::contract_indexer::axum;
impl ContractHandler for Hydentity {
    async fn api(store: Arc<RwLock<Store<Self>>>) -> (Router<()>, OpenApi) {
        let (router, api) = OpenApiRouter::default()
            .routes(routes!(get_state))
            .routes(routes!(get_nonce))
            .split_for_parts();

        (router.with_state(store), api)
    }

    fn handle(tx: &BlobTransaction, index: BlobIndex, state: Self) -> Result<Self> {
        let Blob {
            data,
            contract_name,
        } = tx.blobs.get(index.0).context("Failed to get blob")?;

        let action: IdentityAction =
            borsh::from_slice(data.0.as_slice()).context("Failed to decode payload")?;

        let res = sdk::identity_provider::execute_action(state, action, "")
            .map_err(|e| anyhow::anyhow!(e))?;
        info!("🚀 Executed {contract_name}: {res:?}");
        Ok(res.1)
    }
}

#[utoipa::path(
    get,
    path = "/state",
    tag = "Contract",
    responses(
        (status = OK, description = "Get json state of contract")
    )
)]
pub async fn get_state(
    State(state): State<Arc<RwLock<Store<Hydentity>>>>,
) -> Result<impl IntoResponse, AppError> {
    let store = state.read().await;
    store.state.clone().map(Json).ok_or(AppError(
        StatusCode::NOT_FOUND,
        anyhow::anyhow!("No state found for contract '{}'", store.contract_name),
    ))
}

#[derive(Serialize, ToSchema)]
struct NonceResponse {
    account: String,
    nonce: u32,
}

#[utoipa::path(
    get,
    path = "/nonce/{account}",
    params(
        ("account" = String, Path, description = "Account")
    ),
    tag = "Contract",
    responses(
        (status = OK, description = "Get nonce of account", body = NonceResponse)
    )
)]
pub async fn get_nonce(
    Path(account): Path<Identity>,
    State(state): State<Arc<RwLock<Store<Hydentity>>>>,
) -> Result<impl IntoResponse, AppError> {
    let store = state.read().await;
    let state = store.state.clone().ok_or(AppError(
        StatusCode::NOT_FOUND,
        anyhow!("Contract '{}' not found", store.contract_name),
    ))?;

    let info = state
        .get_identity_info(&account.0)
        .map_err(|err| AppError(StatusCode::NOT_FOUND, anyhow::anyhow!(err)))?;
    let state: AccountInfo = serde_json::from_str(&info).map_err(|_| {
        AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("Failed to parse identity info"),
        )
    })?;

    Ok(Json(NonceResponse {
        account: account.0,
        nonce: state.nonce,
    }))
}
