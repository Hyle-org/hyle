use anyhow::anyhow;
use axum::{
    debug_handler, extract::State, http::StatusCode, response::IntoResponse, routing::get, Json,
    Router,
};
use staking::state::Staking;
use tracing::error;

use crate::{
    bus::{
        bus_client,
        command_response::{CmdRespClient, Query},
        metrics::BusMetrics,
    },
    model::CommonRunContext,
    rest::AppError,
};

use super::{ConsensusInfo, QueryConsensusInfo, QueryConsensusStakingState};

bus_client! {
struct RestBusClient {
    sender(Query<QueryConsensusInfo, ConsensusInfo>),
    sender(Query<QueryConsensusStakingState, Staking>),
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
        .route("/info", get(get_consensus_state))
        .route("/staking_state", get(get_consensus_staking_state))
        .with_state(state)
}

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

#[debug_handler]
pub async fn get_consensus_staking_state(
    State(mut state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    match state.bus.request(QueryConsensusStakingState {}).await {
        Ok(staking_state) => Ok(Json(staking_state)),
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
