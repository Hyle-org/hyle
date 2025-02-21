use alloc::{format, string::String, vec::Vec};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use hyle_model::{Blob, BlobData, BlobIndex, ContractAction, ContractName};

/// Trait representing an identity verification contract.
pub trait IdentityVerification {
    /// Registers a new identity for a given account.
    ///
    /// # Arguments
    ///
    /// * `account` - The address of the account as a string slice.
    /// * `private_input` - A string representing the identity information to be registered.
    ///
    /// # Returns
    ///
    /// * `Result<(), &'static str>` - `Ok(())` if the registration was successful, or an error message on failure.
    fn register_identity(&mut self, account: &str, private_input: &str)
        -> Result<(), &'static str>;

    /// Verifies if an account's identity matches the provided identity information and increase
    /// nonce by +1.
    ///
    /// # Arguments
    ///
    /// * `account` - The address of the account as a string slice.
    /// * `private_input` - A string representing the identity information to verify against.
    ///
    /// # Returns
    ///
    /// * `Result<bool, &'static str>` - `Ok(true)` if the identity matches, `Ok(false)` if it does not, or an error message on failure.
    fn verify_identity(
        &mut self,
        account: &str,
        nonce: u32,
        private_input: &str,
    ) -> Result<bool, &'static str>;

    /// Retrieves the identity information associated with a given account.
    ///
    /// # Arguments
    ///
    /// * `account` - The address of the account as a string slice.
    ///
    /// # Returns
    ///
    /// * `Result<String, &'static str>` - The identity information on success, or an error message on failure.
    fn get_identity_info(&self, account: &str) -> Result<String, &'static str>;

    /// Executes an action on an object that implements the IdentityVerification based on the IdentityAction enum.
    ///
    /// # Arguments
    ///
    /// * `action` - The action to execute, represented as an IdentityAction enum.
    /// * `private_input` - A string representing the private input for the action.
    fn execute_identity_action(
        &mut self,
        action: IdentityAction,
        private_input: &str,
    ) -> Result<String, String> {
        match action {
            IdentityAction::RegisterIdentity { account } => {
                match self.register_identity(&account, private_input) {
                    Ok(()) => Ok(format!(
                        "Successfully registered identity for account: {}",
                        account
                    )),
                    Err(err) => Err(format!("Failed to register identity: {}", err)),
                }
            }
            IdentityAction::VerifyIdentity { account, nonce } => {
                match self.verify_identity(&account, nonce, private_input) {
                    Ok(true) => Ok(format!("Identity verified for account: {}", account)),
                    Ok(false) => Err(format!(
                        "Identity verification failed for account: {}",
                        account
                    )),
                    Err(err) => Err(format!("Error verifying identity: {}", err)),
                }
            }
            IdentityAction::GetIdentityInfo { account } => match self.get_identity_info(&account) {
                Ok(info) => Ok(format!(
                    "Retrieved identity info for account: {}: {}",
                    account, info
                )),
                Err(err) => Err(format!("Failed to get identity info: {}", err)),
            },
        }
    }
}

/// Enum representing the actions that can be performed by the IdentityVerification contract.
#[derive(Serialize, Deserialize, BorshSerialize, BorshDeserialize, Debug, Clone)]
pub enum IdentityAction {
    RegisterIdentity { account: String },
    VerifyIdentity { account: String, nonce: u32 },
    GetIdentityInfo { account: String },
}

impl IdentityAction {
    pub fn as_blob(&self, contract_name: ContractName) -> Blob {
        <Self as ContractAction>::as_blob(self, contract_name, None, None)
    }
}

impl ContractAction for IdentityAction {
    fn as_blob(
        &self,
        contract_name: ContractName,
        _caller: Option<BlobIndex>,
        _callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        Blob {
            contract_name,
            data: BlobData(borsh::to_vec(self).expect("failed to encode program inputs")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyle_model::Digestable;
    use mockall::{mock, predicate::*};

    mock! {
        IdentityVerification {}

        impl IdentityVerification for IdentityVerification {
            fn register_identity(&mut self, account: &str, private_input: &str) -> Result<(), &'static str>;
            fn verify_identity(&mut self, account: &str, nonce: u32, private_input: &str) -> Result<bool, &'static str>;
            fn get_identity_info(&self, account: &str) -> Result<String, &'static str>;
        }

        impl Digestable for IdentityVerification {
            fn as_digest(&self) -> crate::StateDigest;
        }
    }

    #[test]
    fn test_execute_action_register_identity() {
        let mut mock = MockIdentityVerification::new();
        let action = IdentityAction::RegisterIdentity {
            account: "test_account".to_string(),
        };
        let private_input = "test_identity";

        mock.expect_register_identity()
            .with(eq("test_account"), eq(private_input))
            .times(1)
            .returning(|_, _| Ok(()));

        let result = mock.execute_identity_action(action, private_input);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_action_verify_identity() {
        let mut mock = MockIdentityVerification::new();
        let account = "test_account".to_string();
        let private_input = "test_identity";

        mock.expect_verify_identity()
            .with(eq(account.clone()), eq(0), eq(private_input))
            .times(1)
            .returning(|_, _, _| Ok(true));

        let action = IdentityAction::VerifyIdentity {
            account: account.clone(),
            nonce: 0,
        };

        let result = mock.execute_identity_action(action, private_input);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_action_get_identity_info() {
        let mut mock = MockIdentityVerification::new();
        let account = "test_account".to_string();
        let private_input = "test_identity";

        mock.expect_get_identity_info()
            .with(eq(account.clone()))
            .times(1)
            .returning(|_| Ok(private_input.to_string()));

        let action = IdentityAction::GetIdentityInfo {
            account: account.clone(),
        };

        let result = mock.execute_identity_action(action, "");
        assert!(result.is_ok());
    }
}
