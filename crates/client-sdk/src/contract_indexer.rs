use anyhow::{Context, Result};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::RwLock;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Router,
};
use borsh::{BorshDeserialize, BorshSerialize};
use sdk::{
    info, Blob, BlobIndex, BlobTransaction, Calldata, ContractName, Hashed, LightState, TxContext,
    TxId,
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
    Self: LightState + Sized,
{
    fn api(
        store: ContractHandlerStore<Self>,
    ) -> impl std::future::Future<Output = (Router<()>, OpenApi)> + std::marker::Send;

    fn handle_transaction(
        &mut self,
        tx: &BlobTransaction,
        index: BlobIndex,
        tx_context: TxContext,
    ) -> Result<()> {
        let Blob {
            contract_name,
            data: _,
        } = tx.blobs.get(index.0).context("Failed to get blob")?;

        let calldata = Calldata {
            identity: tx.identity.clone(),
            index,
            blobs: tx.blobs.clone(),
            tx_hash: tx.hashed(),
            tx_ctx: Some(tx_context),
            private_input: vec![],
        };

        let program_outputs = self
            .execute_light(&calldata)
            .map_err(|e| anyhow::anyhow!(e))?;

        info!("🚀 Executed {contract_name}: {}", program_outputs);
        Ok(())
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
