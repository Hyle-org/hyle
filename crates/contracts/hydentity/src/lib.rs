use borsh::{BorshDeserialize, BorshSerialize};
use sdk::{utils::parse_raw_contract_input, ContractInput};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use sdk::{identity_provider::IdentityVerification, Digestable, HyleContract, RunResult};
use sha2::{Digest, Sha256};

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod indexer;

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct AccountInfo {
    pub hash: String,
    pub nonce: u32,
}

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone, Default)]
pub struct Hydentity {
    identities: BTreeMap<String, AccountInfo>,
}

impl HyleContract for Hydentity {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, exec_ctx) = parse_raw_contract_input(contract_input)?;
        let private_input = std::str::from_utf8(&contract_input.private_input)
            .map_err(|_| "Invalid UTF-8 sequence")?;
        let output = self.execute_identity_action(action, private_input);

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, exec_ctx, vec![])),
        }
    }
}

impl Hydentity {
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }

    pub fn get_nonce(&self, username: &str) -> Result<u32, &'static str> {
        let info = self.get_identity_info(username)?;
        let state: AccountInfo =
            serde_json::from_str(&info).map_err(|_| "Failed to parse accounf info")?;
        Ok(state.nonce)
    }

    pub fn as_bytes(&self) -> anyhow::Result<Vec<u8>> {
        borsh::to_vec(self).map_err(|_| anyhow::anyhow!("Failed to serialize"))
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

        if self
            .identities
            .insert(account.to_string(), account_info)
            .is_some()
        {
            return Err("Identity already exists");
        }
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
        let mut hasher = Sha256::new();
        for (account, info) in &self.identities {
            hasher.update(account.as_bytes());
            hasher.update(info.hash.as_bytes());
            hasher.update(info.nonce.to_be_bytes());
        }
        sdk::StateDigest(hasher.finalize().to_vec())
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
    fn test_register_identity_that_already_exists() {
        let mut hydentity = Hydentity::default();
        let account = "test_account";
        let private_input = "test_input";

        assert!(hydentity.register_identity(account, private_input).is_ok());
        assert!(hydentity.register_identity(account, private_input).is_err());
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
}
