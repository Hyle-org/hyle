//! Various data structures

#[cfg(feature = "node")]
use crate::bus::SharedMessageBus;
#[cfg(feature = "node")]
use crate::utils::{conf::SharedConf, crypto::SharedBlstCrypto};
#[cfg(feature = "node")]
use axum::Router;
#[cfg(feature = "node")]
use std::sync::Arc;

// Re-export
pub use hyle_model::*;

mod indexer;
mod rest;
pub mod verifiers;

pub use indexer::*;
pub use rest::*;

#[cfg(feature = "node")]
pub struct CommonRunContext {
    pub config: SharedConf,
    pub bus: SharedMessageBus,
    pub router: std::sync::Mutex<Option<Router>>,
}

#[cfg(feature = "node")]
pub struct NodeRunContext {
    pub crypto: SharedBlstCrypto,
}

#[cfg(feature = "node")]
#[derive(Clone)]
pub struct SharedRunContext {
    pub common: Arc<CommonRunContext>,
    pub node: Arc<NodeRunContext>,
}
