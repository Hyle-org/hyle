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
    /// * `identity_info` - A string representing the identity information to be registered.
    ///
    /// # Returns
    ///
    /// * `Result<(), &'static str>` - `Ok(())` if the registration was successful, or an error message on failure.
    fn register_identity(&mut self, account: &str, identity_info: &str)
        -> Result<(), &'static str>;

    /// Verifies if an account's identity matches the provided identity information.
    ///
    /// # Arguments
    ///
    /// * `account` - The address of the account as a string slice.
    /// * `blobs_hash` - The list of blobs hash the identity agrees to run
    /// * `identity_info` - A string representing the identity information to verify against.
    ///
    /// # Returns
    ///
    /// * `Result<bool, &'static str>` - `Ok(true)` if the identity matches, `Ok(false)` if it does not, or an error message on failure.
    fn verify_identity(
        &self,
        account: &str,
        blobs_hash: Vec<String>,
        identity_info: &str,
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
        identity_info: String,
    },
    VerifyIdentity {
        account: String,
        blobs_hash: Vec<String>,
        identity_info: String,
    },
    GetIdentityInfo {
        account: String,
    },
}

pub fn execute_action<T: IdentityVerification>(
    state: &mut T,
    identity: Identity,
    action: IdentityAction,
) -> RunResult {
    let (success, res) = match action {
        IdentityAction::RegisterIdentity {
            account,
            identity_info,
        } => match state.register_identity(&account, &identity_info) {
            Ok(()) => (
                true,
                format!("Successfully registered identity for account: {}", account),
            ),
            Err(err) => (false, format!("Failed to register identity: {}", err)),
        },
        IdentityAction::VerifyIdentity {
            account,
            blobs_hash,
            identity_info,
        } => match state.verify_identity(&account, blobs_hash, &identity_info) {
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
        identity,
        program_outputs,
    }
}

pub fn verify_identity(blobs: &[BlobData], blob_index: usize, identity: &Identity) {
    let identity_action = crate::guest::parse_blob::<IdentityAction>(blobs, blob_index);

    if let IdentityAction::VerifyIdentity { account, .. } = identity_action {
        if account != identity.0 {
            crate::guest::panic("Verify identity blob not match provided identity");
        }
    } else {
        crate::guest::panic(&format!(
            "Blob index {blob_index} must be a VerifyIdentity action"
        ));
    }
}
