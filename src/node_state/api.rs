use anyhow::anyhow;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use hyle_contract_sdk::ContractName;
use hyle_model::UnsettledBlobTransaction;
use tracing::error;

use crate::{
    bus::{
        bus_client,
        command_response::{CmdRespClient, Query},
        metrics::BusMetrics,
    },
    model::{BlockHeight, CommonRunContext, Contract},
    node_state::module::{QueryBlockHeight, QueryUnsettledTx},
    rest::AppError,
};

bus_client! {
struct RestBusClient {
    sender(Query<ContractName, Contract>),
    sender(Query<QueryBlockHeight, BlockHeight>),
    sender(Query<QueryUnsettledTx, UnsettledBlobTransaction>),
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
        .route("/contract/{name}", get(get_contract))
        // TODO: figure out if we want to rely on the indexer instead
        .route("/unsettled_tx/{blob_tx_hash}", get(get_unsettled_tx))
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

pub async fn get_unsettled_tx(
    Path(blob_tx_hash): Path<String>,
    State(mut state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    match state
        .bus
        .request(QueryUnsettledTx(hyle_model::TxHash(blob_tx_hash)))
        .await
    {
        Ok(tx_context) => Ok(Json(tx_context)),
        err => {
            error!("{:?}", err);

            Err(AppError(
                StatusCode::INTERNAL_SERVER_ERROR,
                anyhow!("Error while getting tx context"),
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
                Pick::<
                    tokio::sync::broadcast::Sender<
                        Query<QueryUnsettledTx, UnsettledBlobTransaction>,
                    >,
                >::get(&self.bus)
                .clone(),
            ),
        }
    }
}
