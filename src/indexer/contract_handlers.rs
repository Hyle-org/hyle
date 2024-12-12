use std::sync::Arc;

use super::contract_state_indexer::Store;
use crate::model::BlobTransaction;
use crate::rest::AppError;
use anyhow::{anyhow, Result};
use axum::Router;
use axum::{extract::Path, routing::get};
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

pub trait ContractHandler
where
    Self: Sized,
{
    fn api(
        store: Arc<RwLock<Store<Self>>>,
    ) -> impl std::future::Future<Output = Router<()>> + std::marker::Send;

    fn handle(tx: &BlobTransaction, index: BlobIndex, state: Self) -> Result<Self>;
}

impl ContractHandler for Hydentity {
    async fn api(store: Arc<RwLock<Store<Self>>>) -> Router<()> {
        Router::new()
            .route("/state", get(get_state))
            .route("/nonce/:account", get(get_nonce))
            .with_state(store)
    }

    fn handle(tx: &BlobTransaction, index: BlobIndex, mut state: Self) -> Result<Self> {
        let Blob {
            data,
            contract_name,
        } = tx.blobs.get(index.0).unwrap();

        let (action, _): (IdentityAction, _) =
            bincode::decode_from_slice(data.0.as_slice(), bincode::config::standard())
                .expect("Failed to decode payload");

        let res = identity_provider::execute_action(&mut state, action, "");
        info!("🚀 Executed {contract_name}: {res:?}");
        Ok(state)
    }
}

impl ContractHandler for HyllarToken {
    async fn api(store: Arc<RwLock<Store<HyllarToken>>>) -> Router<()> {
        Router::new()
            .route("/state", get(get_state))
            .route("/balance/:account", get(get_balance))
            .route("/allowance/:account/:spender", get(get_allowance))
            .with_state(store)
    }

    fn handle(tx: &BlobTransaction, index: BlobIndex, state: HyllarToken) -> Result<HyllarToken> {
        let Blob {
            contract_name,
            data,
        } = tx.blobs.get(index.0).unwrap();

        let data: StructuredBlobData<ERC20Action> = data.clone().try_into()?;

        let caller: Identity = data
            .caller
            .map(|i| {
                tx.blobs
                    .get(i.0)
                    .unwrap()
                    .contract_name
                    .0
                    .clone()
                    .into()
            })
            .unwrap_or(tx.identity.clone());

        let mut contract = HyllarTokenContract::init(state, caller);
        let res = erc20::execute_action(&mut contract, data.parameters);
        info!("🚀 Executed {contract_name}: {res:?}");
        Ok(contract.state())
    }
}

pub async fn get_state<S: Serialize + Clone + 'static>(
    State(state): State<Arc<RwLock<Store<S>>>>,
) -> Result<impl IntoResponse, AppError> {
    let store = state.read().await;
    store.state.clone().map(Json).ok_or(AppError(
        StatusCode::NOT_FOUND,
        anyhow!("No state found for contract '{}'", store.contract_name),
    ))
}

pub async fn get_nonce(
    Path(account): Path<Identity>,
    State(state): State<Arc<RwLock<Store<Hydentity>>>>,
) -> Result<impl IntoResponse, AppError> {
    #[derive(Serialize)]
    struct Response {
        account: String,
        nonce: u32,
    }
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

    Ok(Json(Response {
        account: account.0,
        nonce: state.nonce,
    }))
}

pub async fn get_balance(
    Path(account): Path<Identity>,
    State(state): State<Arc<RwLock<Store<HyllarToken>>>>,
) -> Result<impl IntoResponse, AppError> {
    #[derive(Serialize)]
    struct Response {
        account: String,
        balance: u128,
    }
    let store = state.read().await;
    let state = store.state.clone().ok_or(AppError(
        StatusCode::NOT_FOUND,
        anyhow!("Contract '{}' not found", store.contract_name),
    ))?;

    let c = HyllarTokenContract::init(state, account.clone());
    c.balance_of(&account.0)
        .map(|balance| Response {
            account: account.0,
            balance,
        })
        .map(Json)
        .map_err(|err| AppError(StatusCode::NOT_FOUND, anyhow!("{err}'")))
}

pub async fn get_allowance(
    Path((account, spender)): Path<(Identity, Identity)>,
    State(state): State<Arc<RwLock<Store<HyllarToken>>>>,
) -> Result<impl IntoResponse, AppError> {
    #[derive(Serialize)]
    struct Response {
        account: String,
        spender: String,
        allowance: u128,
    }

    let store = state.read().await;
    let state = store.state.clone().ok_or(AppError(
        StatusCode::NOT_FOUND,
        anyhow!("Contract '{}' not found", store.contract_name),
    ))?;

    let c = HyllarTokenContract::init(state, account.clone());
    c.allowance(&account.0, &spender.0)
        .map(|allowance| Response {
            account: account.0,
            spender: spender.0,
            allowance,
        })
        .map(Json)
        .map_err(|err| AppError(StatusCode::NOT_FOUND, anyhow!("{err}'")))
}
