//! Various data structures

use crate::bus::SharedMessageBus;
use crate::utils::conf::SharedConf;
use axum::Router;
use hyle_crypto::SharedBlstCrypto;
use std::sync::Arc;

// Re-export
pub use hyle_model::*;

pub mod contract_registration;
mod indexer;

pub use indexer::*;

pub struct CommonRunContext {
    pub config: SharedConf,
    pub bus: SharedMessageBus,
    pub router: std::sync::Mutex<Option<Router>>,
    pub openapi: std::sync::Mutex<utoipa::openapi::OpenApi>,
}

pub struct NodeRunContext {
    pub crypto: SharedBlstCrypto,
}

#[derive(Clone)]
pub struct SharedRunContext {
    pub common: Arc<CommonRunContext>,
    pub node: Arc<NodeRunContext>,
}
