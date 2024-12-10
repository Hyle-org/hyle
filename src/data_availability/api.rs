use anyhow::anyhow;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use hyle_contract_sdk::ContractName;
use tracing::error;

use crate::{
    bus::{
        bus_client,
        command_response::{CmdRespClient, Query},
        metrics::BusMetrics,
    },
    data_availability::node_state::model::Contract,
    model::{BlockHeight, CommonRunContext},
    rest::AppError,
};

use super::QueryBlockHeight;

bus_client! {
struct RestBusClient {
    sender(Query<ContractName, Contract>),
    sender(Query<QueryBlockHeight, BlockHeight>),
}
}

pub struct RouterState {
    bus: RestBusClient,
}

pub async fn api(ctx: &CommonRunContext) -> Router<()> {
    let state = RouterState {
        bus: RestBusClient::new_from_bus(ctx.bus.new_handle()).await,
    };

    Router::new()
        .route("/da/block/height", get(get_block_height))
        // FIXME: we expose this endpoint for testing purposes. This should be removed or adapted
        .route("/contract/:name", get(get_contract))
        .with_state(state)
}

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

impl Clone for RouterState {
    fn clone(&self) -> Self {
        use crate::utils::static_type_map::Pick;
        Self {
            bus: RestBusClient::new(
                Pick::<BusMetrics>::get(&self.bus).clone(),
                Pick::<tokio::sync::broadcast::Sender<Query<ContractName, Contract>>>::get(
                    &self.bus,
                )
                .clone(),
                Pick::<tokio::sync::broadcast::Sender<Query<QueryBlockHeight, BlockHeight>>>::get(
                    &self.bus,
                )
                .clone(),
            ),
        }
    }
}
