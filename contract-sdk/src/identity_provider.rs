use alloc::{format, string::String, vec::Vec};
use bincode::{Decode, Encode};

use crate::{guest::RunResult, BlobData, Identity};

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

    /// Verifies if an account's identity matches the provided identity information.
    ///
    /// # Arguments
    ///
    /// * `account` - The address of the account as a string slice.
    /// * `blobs_hash` - The list of blobs hash the identity agrees to run
    /// * `private_input` - A string representing the identity information to verify against.
    ///
    /// # Returns
    ///
    /// * `Result<bool, &'static str>` - `Ok(true)` if the identity matches, `Ok(false)` if it does not, or an error message on failure.
    fn verify_identity(
        &self,
        account: &str,
        blobs_hash: Vec<String>,
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
}

/// Enum representing the actions that can be performed by the IdentityVerification contract.
#[derive(Encode, Decode, Debug, Clone)]
pub enum IdentityAction {
    RegisterIdentity {
        account: String,
    },
    VerifyIdentity {
        account: String,
        blobs_hash: Vec<String>,
    },
    GetIdentityInfo {
        account: String,
    },
}

impl From<IdentityAction> for BlobData {
    fn from(val: IdentityAction) -> Self {
        BlobData(
            bincode::encode_to_vec(val, bincode::config::standard())
                .expect("failed to encode program inputs"),
        )
    }
}

pub fn execute_action<T: IdentityVerification>(
    state: &mut T,
    caller: Identity,
    action: IdentityAction,
    private_input: &str,
) -> RunResult {
    let (success, res) = match action {
        IdentityAction::RegisterIdentity { account } => {
            match state.register_identity(&account, private_input) {
                Ok(()) => (
                    true,
                    format!("Successfully registered identity for account: {}", account),
                ),
                Err(err) => (false, format!("Failed to register identity: {}", err)),
            }
        }
        IdentityAction::VerifyIdentity {
            account,
            blobs_hash,
        } => match state.verify_identity(&account, blobs_hash, private_input) {
            Ok(true) => (true, format!("Identity verified for account: {}", account)),
            Ok(false) => (
                false,
                format!("Identity verification failed for account: {}", account),
            ),
            Err(err) => (false, format!("Error verifying identity: {}", err)),
        },
        IdentityAction::GetIdentityInfo { account } => match state.get_identity_info(&account) {
            Ok(info) => (
                true,
                format!("Retrieved identity info for account: {}: {}", account, info),
            ),
            Err(err) => (false, format!("Failed to get identity info: {}", err)),
        },
    };

    let program_outputs = res.as_bytes().to_vec();

    RunResult {
        success,
        identity: caller,
        program_outputs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::{mock, predicate::*};

    mock! {
        IdentityVerification {}

        impl IdentityVerification for IdentityVerification {
            fn register_identity(&mut self, account: &str, private_input: &str) -> Result<(), &'static str>;
            fn verify_identity(&self, account: &str, blobs_hash: Vec<String>, private_input: &str) -> Result<bool, &'static str>;
            fn get_identity_info(&self, account: &str) -> Result<String, &'static str>;
        }
    }

    #[test]
    fn test_execute_action_register_identity() {
        let mut mock = MockIdentityVerification::new();
        let caller = Identity("caller_identity".to_string());
        let caller_identified = Identity("caller_identity.test_contract".to_string());
        let action = IdentityAction::RegisterIdentity {
            account: "test_account".to_string(),
        };
        let private_input = "test_identity";

        mock.expect_register_identity()
            .with(eq("test_account"), eq(private_input))
            .times(1)
            .returning(|_, _| Ok(()));

        let result = execute_action(&mut mock, caller.clone(), action, private_input);
        assert!(result.success);
        assert_eq!(result.identity, caller_identified);
    }

    #[test]
    fn test_execute_action_verify_identity() {
        let mut mock = MockIdentityVerification::new();
        let caller = Identity("caller_identity".to_string());
        let caller_identified = Identity("caller_identity.test_contract".to_string());
        let account = "test_account".to_string();
        let private_input = "test_identity";

        mock.expect_verify_identity()
            .with(eq(account.clone()), eq(vec![]), eq(private_input))
            .times(1)
            .returning(|_, _, _| Ok(true));

        let action = IdentityAction::VerifyIdentity {
            account: account.clone(),
            blobs_hash: vec![],
        };

        let result = execute_action(&mut mock, caller.clone(), action, private_input);
        assert!(result.success);
        assert_eq!(result.identity, caller_identified);
    }

    #[test]
    fn test_execute_action_get_identity_info() {
        let mut mock = MockIdentityVerification::new();
        let caller = Identity("caller_identity".to_string());
        let caller_identified = Identity("caller_identity.test_contract".to_string());
        let account = "test_account".to_string();
        let private_input = "test_identity";

        mock.expect_get_identity_info()
            .with(eq(account.clone()))
            .times(1)
            .returning(|_| Ok(private_input.to_string()));

        let action = IdentityAction::GetIdentityInfo {
            account: account.clone(),
        };

        let result = execute_action(&mut mock, caller.clone(), action, "");
        assert!(result.success);
        assert_eq!(result.identity, caller_identified);
    }
}
