use serde::{Deserialize, Serialize};

use crate::model::ValidatorPublicKey;

#[derive(Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub pubkey: Option<ValidatorPublicKey>,
    pub da_address: String,
}
