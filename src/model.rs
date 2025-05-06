//! Various data structures

use hyle_crypto::SharedBlstCrypto;
use std::sync::Arc;

// Re-export
pub use hyle_model::*;

mod indexer;

pub use indexer::*;

pub use hyle_modules::modules::CommonRunContext;

pub struct NodeRunContext {
    pub crypto: SharedBlstCrypto,
}

#[derive(Clone)]
pub struct SharedRunContext {
    pub common: Arc<CommonRunContext>,
    pub node: Arc<NodeRunContext>,
}
