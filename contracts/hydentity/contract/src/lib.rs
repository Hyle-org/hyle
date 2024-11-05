use std::collections::HashMap;

use anyhow::Error;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use sdk::{identity_provider::IdentityVerification, Digestable, HyleContract};

#[derive(Encode, Decode, Serialize, Deserialize, Debug, Clone)]
pub struct Hydentity {
    identities: HashMap<String, String>,
}

impl Hydentity {
    pub fn new() -> Self {
        Hydentity {
            identities: HashMap::new(),
        }
    }
}

impl Default for Hydentity {
    fn default() -> Self {
        Self::new()
    }
}

impl HyleContract for Hydentity {
    fn caller(&self) -> sdk::Identity {
        todo!()
    }
}

impl IdentityVerification for Hydentity {
    fn register_identity(
        &mut self,
        account: &str,
        identity_info: &str,
    ) -> Result<(), &'static str> {
        self.identities
            .insert(account.to_string(), identity_info.to_string());
        Ok(())
    }

    fn verify_identity(
        &self,
        account: &str,
        blobs_hash: Vec<String>,
        identity_info: &str,
    ) -> Result<bool, &'static str> {
        match self.identities.get(account) {
            // TODO: instead of ==, we should do signature verification
            Some(stored_info) => Ok(stored_info == identity_info && !blobs_hash.is_empty()),
            None => Err("Identity not found"),
        }
    }

    fn get_identity_info(&self, account: &str) -> Result<String, &'static str> {
        match self.identities.get(account) {
            Some(info) => Ok(info.clone()),
            None => Err("Identity not found"),
        }
    }
}

impl Digestable for Hydentity {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(
            bincode::encode_to_vec(self, bincode::config::standard())
                .expect("Failed to encode Balances"),
        )
    }
}
impl TryFrom<sdk::StateDigest> for Hydentity {
    type Error = Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        let (balances, _) = bincode::decode_from_slice(&state.0, bincode::config::standard())
            .map_err(|_| anyhow::anyhow!("Could not decode start height"))?;
        Ok(balances)
    }
}
