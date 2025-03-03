use crate::{AccountInfo, Hydentity};
use anyhow::{anyhow, Context, Result};
use client_sdk::contract_indexer::{
    axum::Router,
    utoipa::openapi::OpenApi,
    utoipa_axum::{router::OpenApiRouter, routes},
    ContractHandler, ContractHandlerStore,
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
    Blob, BlobIndex, BlobTransaction, Identity, TxContext,
};
use serde::Serialize;

use client_sdk::contract_indexer::axum;
impl ContractHandler for Hydentity {
    async fn api(store: ContractHandlerStore<Self>) -> (Router<()>, OpenApi) {
        let (router, api) = OpenApiRouter::default()
            .routes(routes!(get_state))
            .routes(routes!(get_nonce))
            .split_for_parts();

        (router.with_state(store), api)
    }

    fn handle(
        tx: &BlobTransaction,
        index: BlobIndex,
        mut state: Self,
        _tx_context: TxContext,
    ) -> Result<Self> {
        let Blob {
            contract_name: _,
            data,
        } = tx.blobs.get(index.0).context("Failed to get blob")?;

        let action: IdentityAction = borsh::from_slice(&data.0)?;

        match action {
            IdentityAction::RegisterIdentity { account } => {
                let (name, hash) = Hydentity::parse_id(&account)?;
                state
                    .identities
                    .insert(name, AccountInfo { hash, nonce: 0 });
            }
            IdentityAction::VerifyIdentity { account, nonce: _ } => {
                if let Some(id) = state.identities.get_mut(&account) {
                    id.nonce += 1;
                }
            }
            _ => {}
        }

        Ok(state)
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
    State(state): State<ContractHandlerStore<Hydentity>>,
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
    State(state): State<ContractHandlerStore<Hydentity>>,
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
