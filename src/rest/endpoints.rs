use crate::bus::command_response::CmdRespClient;
use crate::bus::BusClientSender;
use crate::data_availability::QueryBlockHeight;
use crate::model::ContractName;
use crate::tools::mock_workflow::RunScenario;
use anyhow::anyhow;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use tracing::error;

use super::{AppError, RouterState};

pub async fn get_contract(
    Path(name): Path<ContractName>,
    State(mut state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    let name_clone = name.clone();
    match state.bus.request(name).await {
        Ok(contract) => Ok(Json(contract)),
        err => {
            error!("{:?}", err);

            Err(AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Error while getting contract {}", name_clone),
            ))
        }
    }
}

pub async fn get_block_height(
    State(mut state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    match state.bus.request(QueryBlockHeight {}).await {
        Ok(block_height) => Ok(Json(block_height)),
        err => {
            error!("{:?}", err);

            Err(AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Error while getting block height"),
            ))
        }
    }
}

pub async fn run_scenario(
    State(mut state): State<RouterState>,
    Json(scenario): Json<RunScenario>,
) -> Result<impl IntoResponse, StatusCode> {
    state
        .bus
        .send(scenario)
        .map(|_| StatusCode::OK)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn get_info(State(state): State<RouterState>) -> Result<impl IntoResponse, AppError> {
    Ok(Json(state.info))
}
