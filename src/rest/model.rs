use serde::{Deserialize, Serialize};

use crate::{bus::SharedMessageBus, indexer::Indexer};

#[derive(Serialize, Deserialize)]
pub struct TransactionRequest {
    pub tx_hash: String,
}

#[derive(Serialize, Deserialize)]
pub struct BlockRequest {
    pub block_number: u64,
}

pub struct RouterState {
    pub bus: SharedMessageBus,
    pub idxr: Indexer,
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        Self {
            bus: self.bus.new_handle(),
            idxr: self.idxr.clone(),
        }
    }
}
