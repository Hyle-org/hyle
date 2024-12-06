use std::sync::Arc;

use super::contract_state_indexer::Store;
use crate::model::BlobTransaction;
use crate::rest::AppError;
use anyhow::{anyhow, Result};
use axum::Router;
use axum::{extract::Path, routing::get};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use hyle_contract_sdk::ContractName;
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

impl ContractHandler for HyllarToken {
    async fn api(store: Arc<RwLock<Store<HyllarToken>>>) -> Router<()> {
        Router::new()
            .route("/:name/state", get(get_state))
            .route("/:name/balance/:account", get(get_balance))
            .route("/:name/allowance/:account/:spender", get(get_allowance))
            .with_state(store)
    }

    fn handle(tx: &BlobTransaction, index: BlobIndex, state: HyllarToken) -> Result<HyllarToken> {
        let Blob {
            contract_name,
            data,
        } = tx.blobs.get(index.0 as usize).unwrap();

        let data: StructuredBlobData<ERC20Action> = data.clone().try_into()?;

        let caller: Identity = data
            .caller
            .map(|i| {
                tx.blobs
                    .get(i.0 as usize)
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
    Path(name): Path<ContractName>,
    State(state): State<Arc<RwLock<Store<S>>>>,
) -> Result<impl IntoResponse, AppError> {
    let s = state.read().await;
    s.states.get(&name).cloned().map(Json).ok_or_else(|| {
        AppError(
            StatusCode::NOT_FOUND,
            anyhow!("Contract '{}' not found", name),
        )
    })
}

pub async fn get_balance(
    Path((name, account)): Path<(ContractName, Identity)>,
    State(state): State<Arc<RwLock<Store<HyllarToken>>>>,
) -> Result<impl IntoResponse, AppError> {
    #[derive(Serialize)]
    struct Response {
        account: String,
        balance: u128,
    }

    let state = state
        .read()
        .await
        .states
        .get(&name)
        .cloned()
        .ok_or_else(|| {
            AppError(
                StatusCode::NOT_FOUND,
                anyhow!("Contract '{}' not found", name),
            )
        })?;
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
    Path((name, account, spender)): Path<(ContractName, Identity, Identity)>,
    State(state): State<Arc<RwLock<Store<HyllarToken>>>>,
) -> Result<impl IntoResponse, AppError> {
    #[derive(Serialize)]
    struct Response {
        account: String,
        spender: String,
        allowance: u128,
    }

    let state = state
        .read()
        .await
        .states
        .get(&name)
        .cloned()
        .ok_or_else(|| {
            AppError(
                StatusCode::NOT_FOUND,
                anyhow!("Contract '{}' not found", name),
            )
        })?;
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
