use std::collections::BTreeMap;

use anyhow::Error;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use sdk::{identity_provider::IdentityVerification, Digestable};
use sha2::{Digest, Sha256};

#[cfg(feature = "metadata")]
pub mod metadata {
    pub const HYDENTITY_ELF: &[u8] = include_bytes!("../hydentity.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../hydentity.txt"));
}

#[cfg(feature = "client")]
pub mod client;

#[derive(Encode, Decode, Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct AccountInfo {
    pub hash: String,
    pub nonce: u32,
}

#[derive(Encode, Decode, Serialize, Deserialize, Debug, Clone)]
pub struct Hydentity {
    identities: BTreeMap<String, AccountInfo>,
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
        let account_info = AccountInfo {
            hash: hex::encode(hash_bytes),
            nonce: 0,
        };

        self.identities.insert(account.to_string(), account_info);
        Ok(())
    }

    fn verify_identity(
        &mut self,
        account: &str,
        nonce: u32,
        private_input: &str,
    ) -> Result<bool, &'static str> {
        match self.identities.get_mut(account) {
            Some(stored_info) => {
                if nonce != stored_info.nonce {
                    return Err("Invalid nonce");
                }
                let id = format!("{account}:{private_input}");
                let mut hasher = Sha256::new();
                hasher.update(id.as_bytes());
                let hashed = hex::encode(hasher.finalize());
                if *stored_info.hash != hashed {
                    return Ok(false);
                }
                stored_info.nonce += 1;
                Ok(true)
            }
            None => Err("Identity not found"),
        }
    }

    fn get_identity_info(&self, account: &str) -> Result<String, &'static str> {
        match self.identities.get(account) {
            Some(info) => Ok(serde_json::to_string(&info).map_err(|_| "Failed to serialize")?),
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
            .map_err(|_| anyhow::anyhow!("Could not decode hydentity state"))?;
        Ok(balances)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn test_register_identity() {
        let mut hydentity = Hydentity::default();
        let account = "test_account";
        let private_input = "test_input";

        assert!(hydentity.register_identity(account, private_input).is_ok());

        let id = format!("{account}:{private_input}");
        let mut hasher = Sha256::new();
        hasher.update(id.as_bytes());
        let hash_bytes = hasher.finalize();
        let expected_hash = hex::encode(hash_bytes);

        let registered = hydentity.identities.get(account).unwrap();
        assert_eq!(registered.hash, expected_hash);
        assert_eq!(registered.nonce, 0);
    }

    #[test]
    fn test_verify_identity() {
        let mut hydentity = Hydentity::default();
        let account = "test_account";
        let private_input = "test_input";

        hydentity.register_identity(account, private_input).unwrap();

        assert!(hydentity
            .verify_identity(account, 1, private_input)
            .is_err());
        assert!(hydentity
            .verify_identity(account, 0, private_input)
            .unwrap());
        assert!(hydentity
            .verify_identity(account, 0, private_input)
            .is_err()); // Same is wrong as nonce increased
        assert!(hydentity
            .verify_identity(account, 1, private_input)
            .unwrap());
        assert!(hydentity
            .verify_identity("nonexistent_account", 0, private_input)
            .is_err());
    }

    #[test]
    fn test_get_identity_info() {
        let mut hydentity = Hydentity::default();
        let account = "test_account";
        let private_input = "test_input";

        hydentity.register_identity(account, private_input).unwrap();

        let id = format!("{account}:{private_input}");
        let mut hasher = Sha256::new();
        hasher.update(id.as_bytes());
        let hash_bytes = hasher.finalize();
        let expected_hash = hex::encode(hash_bytes);
        let expected_info = serde_json::to_string(&AccountInfo {
            hash: expected_hash.clone(),
            nonce: 0,
        });

        assert_eq!(
            hydentity.get_identity_info(account).unwrap(),
            expected_info.unwrap()
        );
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
