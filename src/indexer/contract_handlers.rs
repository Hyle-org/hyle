use std::sync::Arc;

use super::contract_state_indexer::Store;
use crate::model::BlobTransaction;
use crate::rest::AppError;
use anyhow::{anyhow, Context, Result};
use axum::extract::Path;
use axum::Router;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use hydentity::{AccountInfo, Hydentity};
use hyle_contract_sdk::identity_provider::{self, IdentityAction, IdentityVerification};
use hyle_contract_sdk::{
    erc20::{self, ERC20Action, ERC20},
    Blob, BlobIndex, Identity, StructuredBlobData,
};
use hyllar::{HyllarToken, HyllarTokenContract};
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::info;
use utoipa::openapi::OpenApi;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub trait ContractHandler
where
    Self: Sized,
{
    fn api(
        store: Arc<RwLock<Store<Self>>>,
    ) -> impl std::future::Future<Output = (Router<()>, OpenApi)> + std::marker::Send;

    fn handle(tx: &BlobTransaction, index: BlobIndex, state: Self) -> Result<Self>;
}

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

        let res =
            identity_provider::execute_action(state, action, "").map_err(|e| anyhow::anyhow!(e))?;
        info!("🚀 Executed {contract_name}: {res:?}");
        Ok(res.1)
    }
}

impl ContractHandler for HyllarToken {
    async fn api(store: Arc<RwLock<Store<HyllarToken>>>) -> (Router<()>, OpenApi) {
        let (router, api) = OpenApiRouter::default()
            .routes(routes!(get_state))
            .routes(routes!(get_balance))
            .routes(routes!(get_allowance))
            .split_for_parts();

        (router.with_state(store), api)
    }

    fn handle(tx: &BlobTransaction, index: BlobIndex, state: HyllarToken) -> Result<HyllarToken> {
        let Blob {
            contract_name,
            data,
        } = tx.blobs.get(index.0).context("Failed to get blob")?;

        let data: StructuredBlobData<ERC20Action> = data.clone().try_into()?;

        let caller = data
            .caller
            .and_then(|idx| {
                tx.blobs
                    .get(idx.0)
                    .map(|b| Identity(b.contract_name.0.clone()))
            })
            .unwrap_or(tx.identity.clone());

        let contract = HyllarTokenContract::init(state, caller);
        let res =
            erc20::execute_action(contract, data.parameters).map_err(|e| anyhow::anyhow!(e))?;
        info!("🚀 Executed {contract_name}: {res:?}");
        Ok(res.1.state())
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
pub async fn get_state<S: Serialize + Clone + 'static>(
    State(state): State<Arc<RwLock<Store<S>>>>,
) -> Result<impl IntoResponse, AppError> {
    let store = state.read().await;
    store.state.clone().map(Json).ok_or(AppError(
        StatusCode::NOT_FOUND,
        anyhow!("No state found for contract '{}'", store.contract_name),
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

#[derive(Serialize, ToSchema)]
struct BalanceResponse {
    account: String,
    balance: u128,
}
#[utoipa::path(
    get,
    path = "/balance/{account}",
    params(
        ("account" = String, Path, description = "Account")
    ),
    tag = "Contract",
    responses(
        (status = OK, description = "Get balance of account", body = BalanceResponse)
    )
)]
pub async fn get_balance(
    Path(account): Path<Identity>,
    State(state): State<Arc<RwLock<Store<HyllarToken>>>>,
) -> Result<impl IntoResponse, AppError> {
    let store = state.read().await;
    let state = store.state.clone().ok_or(AppError(
        StatusCode::NOT_FOUND,
        anyhow!("Contract '{}' not found", store.contract_name),
    ))?;

    let c = HyllarTokenContract::init(state, account.clone());
    c.balance_of(&account.0)
        .map(|balance| BalanceResponse {
            account: account.0,
            balance,
        })
        .map(Json)
        .map_err(|err| AppError(StatusCode::NOT_FOUND, anyhow!("{err}'")))
}

#[derive(Serialize, ToSchema)]
struct AllowanceResponse {
    account: String,
    spender: String,
    allowance: u128,
}

#[utoipa::path(
    get,
    path = "/allowance/{account}/{spender}",
    params(
        ("account" = String, Path, description = "Account"),
        ("spender" = String, Path, description = "Spender")
    ),
    tag = "Contract",
    responses(
        (status = OK, description = "Get allowance of account for given spender", body = AllowanceResponse)
    )
)]
pub async fn get_allowance(
    Path((account, spender)): Path<(Identity, Identity)>,
    State(state): State<Arc<RwLock<Store<HyllarToken>>>>,
) -> Result<impl IntoResponse, AppError> {
    let store = state.read().await;
    let state = store.state.clone().ok_or(AppError(
        StatusCode::NOT_FOUND,
        anyhow!("Contract '{}' not found", store.contract_name),
    ))?;

    let c = HyllarTokenContract::init(state, account.clone());
    c.allowance(&account.0, &spender.0)
        .map(|allowance| AllowanceResponse {
            account: account.0,
            spender: spender.0,
            allowance,
        })
        .map(Json)
        .map_err(|err| AppError(StatusCode::NOT_FOUND, anyhow!("{err}'")))
}
