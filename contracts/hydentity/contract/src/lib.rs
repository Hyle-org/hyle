use std::collections::BTreeMap;

use anyhow::Error;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use sdk::{identity_provider::IdentityVerification, Digestable, HyleContract};
use sha2::{Digest, Sha256};

#[derive(Encode, Decode, Serialize, Deserialize, Debug, Clone)]
pub struct Hydentity {
    identities: BTreeMap<String, String>,
}

impl Hydentity {
    pub fn new() -> Self {
        Hydentity {
            identities: BTreeMap::new(),
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
        unreachable!()
    }
}

impl IdentityVerification for Hydentity {
    fn register_identity(
        &mut self,
        account: &str,
        private_input: &str,
    ) -> Result<(), &'static str> {
        let id = format!("{account}:{private_input}");
        let mut hasher = Sha256::new();
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
                let mut hasher = Sha256::new();
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha3::{Digest, Sha3_256};

    #[test]
    fn test_register_identity() {
        let mut hydentity = Hydentity::default();
        let account = "test_account";
        let private_input = "test_input";

        assert!(hydentity.register_identity(account, private_input).is_ok());

        let id = format!("{account}:{private_input}");
        let mut hasher = Sha3_256::new();
        hasher.update(id.as_bytes());
        let hash_bytes = hasher.finalize();
        let expected_hash = hex::encode(hash_bytes);

        assert_eq!(hydentity.identities.get(account).unwrap(), &expected_hash);
    }

    #[test]
    fn test_verify_identity() {
        let mut hydentity = Hydentity::default();
        let account = "test_account";
        let private_input = "test_input";
        let blobs_hash = vec!["blob1".to_string(), "blob2".to_string()];

        hydentity.register_identity(account, private_input).unwrap();

        assert!(hydentity
            .verify_identity(account, blobs_hash.clone(), private_input)
            .unwrap());
        assert!(!hydentity
            .verify_identity(account, vec![], private_input)
            .unwrap());
        assert!(hydentity
            .verify_identity("nonexistent_account", blobs_hash, private_input)
            .is_err());
    }

    #[test]
    fn test_get_identity_info() {
        let mut hydentity = Hydentity::default();
        let account = "test_account";
        let private_input = "test_input";

        hydentity.register_identity(account, private_input).unwrap();

        let id = format!("{account}:{private_input}");
        let mut hasher = Sha3_256::new();
        hasher.update(id.as_bytes());
        let hash_bytes = hasher.finalize();
        let expected_hash = hex::encode(hash_bytes);

        assert_eq!(hydentity.get_identity_info(account).unwrap(), expected_hash);
        assert!(hydentity.get_identity_info("nonexistent_account").is_err());
    }

    #[test]
    fn test_as_digest() {
        let mut hydentity = Hydentity::default();
        hydentity
            .register_identity("test_account", "test_input")
            .unwrap();
        let digest = hydentity.as_digest();

        let encoded = bincode::encode_to_vec(&hydentity, bincode::config::standard())
            .expect("Failed to encode Hydentity");
        assert_eq!(digest.0, encoded);
    }

    #[test]
    fn test_try_from_state_digest() {
        let mut hydentity = Hydentity::default();
        hydentity
            .register_identity("test_account", "test_input")
            .unwrap();

        let digest = hydentity.as_digest();

        let decoded_hydentity: Hydentity =
            Hydentity::try_from(digest.clone()).expect("Failed to decode state digest");
        assert_eq!(decoded_hydentity.identities, hydentity.identities);

        let invalid_digest = sdk::StateDigest(vec![5, 5, 5]);
        let result = Hydentity::try_from(invalid_digest);
        assert!(result.is_err());
    }
}
