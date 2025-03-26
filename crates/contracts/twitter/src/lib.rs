use borsh::{BorshDeserialize, BorshSerialize};
use sdk::{utils::parse_raw_contract_input, Blob, ContractAction, ContractInput, ContractName};
use serde::{Deserialize, Serialize};

use sdk::{HyleContract, RunResult};
use sha2::{Digest, Sha256};

#[cfg(feature = "client")]
pub mod client;

impl HyleContract for Twitter {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, exec_ctx) = parse_raw_contract_input::<TwitterAction>(contract_input)?;

        let output = match action {
            TwitterAction::VerifyHandle { handle } => {
                Ok(format!("Twitter handle {handle} is verified"))
            }
        };

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, exec_ctx, vec![])),
        }
    }

    fn commit(&self) -> sdk::StateCommitment {
        let mut hasher = Sha256::new();
        hasher.update("cavaettoi?".as_bytes());
        sdk::StateCommitment(hasher.finalize().to_vec())
    }
}

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone, Default)]
pub struct Twitter {}

/// Enum representing the actions that can be performed by the IdentityVerification contract.
#[derive(Serialize, Deserialize, BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub enum TwitterAction {
    VerifyHandle { handle: String },
}

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct AccountInfo {
    pub hash: String,
    pub nonce: u32,
}

impl Twitter {
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }
}

impl TwitterAction {
    pub fn as_blob(&self, contract_name: ContractName) -> Blob {
        <Self as ContractAction>::as_blob(self, contract_name, None, None)
    }
}
impl ContractAction for TwitterAction {
    fn as_blob(
        &self,
        contract_name: ContractName,
        _caller: Option<sdk::BlobIndex>,
        _callees: Option<Vec<sdk::BlobIndex>>,
    ) -> Blob {
        Blob {
            contract_name,
            data: sdk::BlobData(borsh::to_vec(self).expect("failed to encode program inputs")),
        }
    }
}
