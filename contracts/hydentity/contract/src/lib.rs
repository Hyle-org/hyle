use std::collections::HashMap;

use anyhow::Error;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use sdk::{identity_provider::IdentityVerification, Digestable, HyleContract};
use sha3::{Digest, Sha3_256};

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
        private_input: &str,
    ) -> Result<(), &'static str> {
        let id = format!("{account}:{private_input}");
        let mut hasher = Sha3_256::new();
        hasher.update(id.as_bytes());
        let hash_bytes = hasher.finalize();
        self.identities
            .insert(account.to_string(), hex::encode(hash_bytes));
        Ok(())
    }

    fn verify_identity(
        &self,
        account: &str,
        blobs_hash: Vec<String>,
        private_input: &str,
    ) -> Result<bool, &'static str> {
        match self.identities.get(account) {
            Some(stored_info) => {
                let id = format!("{account}:{private_input}");
                let mut hasher = Sha3_256::new();
                hasher.update(id.as_bytes());
                let hashed = hex::encode(hasher.finalize());
                Ok(*stored_info == hashed && !blobs_hash.is_empty())
            }
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
