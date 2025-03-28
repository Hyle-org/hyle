use anyhow::Result;
use client_sdk::contract_indexer::{
    axum::{extract::State, response::IntoResponse, Json, Router},
    utoipa::openapi::OpenApi,
    utoipa_axum::{router::OpenApiRouter, routes},
    AppError, ContractHandler, ContractHandlerStore,
};
use serde::Serialize;

use crate::*;
use client_sdk::contract_indexer::axum;
use client_sdk::contract_indexer::utoipa;

impl ContractHandler for Twitter {
    async fn api(store: ContractHandlerStore<Twitter>) -> (Router<()>, OpenApi) {
        let (router, api) = OpenApiRouter::default()
            .routes(routes!(get_state))
            .split_for_parts();

        (router.with_state(store), api)
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
    State(_state): State<ContractHandlerStore<S>>,
) -> Result<impl IntoResponse, AppError> {
    Ok(Json(Twitter {}))
}
