use anyhow::{Context, Result};
use core::str;
use reqwest::StatusCode;
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::debug;

use axum::{
    response::{IntoResponse, Response},
    Router,
};
use borsh::{BorshDeserialize, BorshSerialize};
use sdk::{
    guest, info, Blob, BlobIndex, BlobTransaction, ContractName, Hashed, HyleContract,
    ProgramInput, TxContext, TxId,
};
use utoipa::openapi::OpenApi;

pub use axum;
pub use utoipa;
pub use utoipa_axum;

#[derive(BorshSerialize, BorshDeserialize)]
pub struct ContractStore<Contract> {
    pub contract: Option<Contract>,
    pub contract_name: ContractName,
    pub unsettled_blobs: BTreeMap<TxId, BlobTransaction>,
}

pub type ContractHandlerStore<T> = Arc<RwLock<ContractStore<T>>>;

impl<Contract> Default for ContractStore<Contract> {
    fn default() -> Self {
        ContractStore {
            contract: None,
            contract_name: Default::default(),
            unsettled_blobs: BTreeMap::new(),
        }
    }
}

pub trait ContractHandler
where
    Self: Sized
        + std::fmt::Debug
        + Default
        + HyleContract
        + Serialize
        + BorshSerialize
        + BorshDeserialize
        + 'static,
{
    fn api(
        store: ContractHandlerStore<Self>,
    ) -> impl std::future::Future<Output = (Router<()>, OpenApi)> + std::marker::Send;

    fn handle(
        tx: &BlobTransaction,
        index: BlobIndex,
        contract: Self,
        tx_context: TxContext,
    ) -> Result<Self> {
        let Blob {
            contract_name,
            data: _,
        } = tx.blobs.get(index.0).context("Failed to get blob")?;

        let serialized_contract = borsh::to_vec(&contract)?;
        let program_input = ProgramInput {
            contract: serialized_contract,
            identity: tx.identity.clone(),
            index,
            blobs: tx.blobs.clone(),
            tx_hash: tx.hashed(),
            tx_ctx: Some(tx_context),
            private_input: vec![],
        };

        let (contract, hyle_output) = guest::execute::<Self>(&program_input);
        let res = str::from_utf8(&hyle_output.program_outputs).unwrap_or("no output");
        info!("🚀 Executed {contract_name}: {}", res);
        debug!(
            handler = %contract_name,
            "hyle_output: {:?}", hyle_output
        );
        Ok(contract)
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
