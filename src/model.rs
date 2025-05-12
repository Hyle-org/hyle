//! Various data structures

use hyle_crypto::SharedBlstCrypto;
use hyle_modules::{modules::SharedBuildApiCtx, utils::conf::SharedConf};

// Re-export
pub use hyle_model::*;

mod indexer;

pub use indexer::*;

#[derive(Clone)]
pub struct SharedRunContext {
    pub config: SharedConf,
    pub api: SharedBuildApiCtx,
    pub crypto: SharedBlstCrypto,
}
