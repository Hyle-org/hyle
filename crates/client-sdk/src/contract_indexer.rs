use anyhow::{Context, Result};
use core::str;
use reqwest::StatusCode;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::debug;

use axum::{
    response::{IntoResponse, Response},
    Router,
};
use borsh::{BorshDeserialize, BorshSerialize};
use sdk::{
    guest, info, Blob, BlobIndex, BlobTransaction, Calldata, ContractName, Hashed, HyleContract,
    TxContext, TxId, ZkProgramInput,
};
use utoipa::openapi::OpenApi;

pub use axum;
pub use utoipa;
pub use utoipa_axum;

#[derive(BorshSerialize, BorshDeserialize)]
pub struct ContractStateStore<State> {
    pub state: Option<State>,
    pub contract_name: ContractName,
    pub unsettled_blobs: BTreeMap<TxId, BlobTransaction>,
}

pub type ContractHandlerStore<T> = Arc<RwLock<ContractStateStore<T>>>;

impl<State> Default for ContractStateStore<State> {
    fn default() -> Self {
        ContractStateStore {
            state: None,
            contract_name: Default::default(),
            unsettled_blobs: BTreeMap::new(),
        }
    }
}

pub trait ContractHandler
where
    Self: Sized + Default + HyleContract + BorshSerialize + BorshDeserialize + 'static,
{
    fn api(
        store: ContractHandlerStore<Self>,
    ) -> impl std::future::Future<Output = (Router<()>, OpenApi)> + std::marker::Send;

    fn handle(
        tx: &BlobTransaction,
        index: BlobIndex,
        state: Self,
        tx_context: TxContext,
    ) -> Result<Self> {
        let Blob {
            contract_name,
            data: _,
        } = tx.blobs.get(index.0).context("Failed to get blob")?;

        let serialized_state = borsh::to_vec(&state)?;
        let zk_program_input = ZkProgramInput {
            commitment_metadata: serialized_state,
            calldata: Calldata {
                identity: tx.identity.clone(),
                index,
                blobs: tx.blobs.clone(),
                tx_hash: tx.hashed(),
                tx_ctx: Some(tx_context),
                private_input: vec![],
            },
        };

        let (state, hyle_output) = guest::execute::<Self>(&zk_program_input);
        let res = str::from_utf8(&hyle_output.program_outputs).unwrap_or("no output");
        info!("🚀 Executed {contract_name}: {}", res);
        debug!(
            handler = %contract_name,
            "hyle_output: {:?}", hyle_output
        );
        Ok(state)
    }
}

// Make our own error that wraps `anyhow::Error`.
pub struct AppError(pub StatusCode, pub anyhow::Error);

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, format!("{}", self.1)).into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, err.into())
    }
}
