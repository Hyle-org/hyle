use anyhow::anyhow;
use axum::{debug_handler, extract::State, http::StatusCode, response::IntoResponse, Json, Router};
use hyle_model::api::APIStaking;
use staking::state::Staking;
use tracing::error;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    bus::{
        bus_client,
        command_response::{CmdRespClient, Query},
        metrics::BusMetrics,
    },
    model::{CommonRunContext, ConsensusInfo},
    rest::AppError,
};

use super::{QueryConsensusInfo, QueryConsensusStakingState};

bus_client! {
struct RestBusClient {
    sender(Query<QueryConsensusInfo, ConsensusInfo>),
    sender(Query<QueryConsensusStakingState, Staking>),
}
}

pub struct RouterState {
    bus: RestBusClient,
}

#[derive(OpenApi)]
struct ConsensusAPI;

pub async fn api(ctx: &CommonRunContext) -> Router<()> {
    let state = RouterState {
        bus: RestBusClient::new_from_bus(ctx.bus.new_handle()).await,
    };

    let (router, api) = OpenApiRouter::with_openapi(ConsensusAPI::openapi())
        .routes(routes!(get_consensus_state))
        .routes(routes!(get_consensus_staking_state))
        .split_for_parts();

    if let Ok(mut o) = ctx.openapi.lock() {
        *o = o.clone().nest("/v1", api);
    }

    router.with_state(state)
}

#[utoipa::path(
    get,
    path = "/info",
    tag = "Consensus",
    responses(
        (status = OK, body = ConsensusInfo)
    )
)]
#[debug_handler]
pub async fn get_consensus_state(
    State(mut state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    match state.bus.request(QueryConsensusInfo {}).await {
        Ok(consensus_state) => Ok(Json(consensus_state)),
        Err(err) => {
            error!("{:?}", err);

            Err(AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Error while getting consensus state: {err}"),
            ))
        }
    }
}

#[utoipa::path(
    get,
    path = "/staking_state",
    tag = "Consensus",
    responses(
        (status = OK, body = APIStaking)
    )
)]
#[debug_handler]
pub async fn get_consensus_staking_state(
    State(mut state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    match state.bus.request(QueryConsensusStakingState {}).await {
        Ok(staking) => {
            let api: APIStaking = staking.into();
            Ok(Json(api))
        }
        Err(err) => {
            error!("{:?}", err);

            Err(AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Error while getting staking state: {err}"),
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
                Pick::<tokio::sync::broadcast::Sender<Query<QueryConsensusInfo, ConsensusInfo>>>::get(
                    &self.bus,
                )
                .clone(),
                Pick::<tokio::sync::broadcast::Sender<Query<QueryConsensusStakingState, Staking>>>::get(
                    &self.bus,
                )
                .clone(),
            )
        }
    }
}
