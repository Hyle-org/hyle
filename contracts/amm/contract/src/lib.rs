use std::collections::HashMap;

use anyhow::Error;
use bincode::{Decode, Encode};
use sdk::{erc20::ERC20Action, Identity};
use sdk::{guest::RunResult, Blob, BlobIndex, Digestable};
use serde::Deserialize;

type TokenPair = (String, String);
type TokenPairAmount = (u128, u128);

pub struct AmmContract {
    state: AmmState,
    caller: Identity,
}

#[derive(Deserialize, Clone, Encode, Decode)]
pub struct AmmState {
    pairs: HashMap<TokenPair, TokenPairAmount>,
}

impl AmmContract {
    pub fn new(state: AmmState, caller: Identity) -> Self {
        AmmContract { state, caller }
    }

    pub fn state(self) -> AmmState {
        self.state
    }

    pub fn verify_swap(
        &mut self,
        callees_blobs: Vec<Blob>,
        from: Identity,
        pair: TokenPair,
    ) -> RunResult {
        // Check that from field correspond to caller
        if from != self.caller {
            return RunResult {
                success: false,
                identity: self.caller.clone(),
                program_outputs: "Caller is not the same as the 'from' field"
                    .to_string()
                    .into_bytes(),
            };
        }

        // Check that swap is only about two different tokens and that there is one blob for each
        if callees_blobs.len() != 2 || pair.0 == pair.1 {
            return RunResult {
                success: false,
                identity: self.caller.clone(),
                program_outputs: "Swap can only happen between two different tokens"
                    .to_string()
                    .into_bytes(),
            };
        }

        // Extract "blob_from" out of the blobs. This is the blob that has the first token of the pair as contract name
        let blob_from_index = match callees_blobs
            .iter()
            .position(|blob| blob.contract_name.0 == pair.0)
        {
            Some(index) => BlobIndex(index as u32),
            None => {
                return RunResult {
                    success: false,
                    identity: self.caller.clone(),
                    program_outputs: format!(
                        "Blob with contract name {} not found in callees",
                        pair.0
                    )
                    .into_bytes(),
                }
            }
        };
        let blob_from = sdk::guest::parse_blob::<ERC20Action>(&callees_blobs, &blob_from_index);

        // Extract "blob_to" out of the blobs. This is the blob that has the second token of the pair as contract name
        let blob_to_index = match callees_blobs
            .iter()
            .position(|blob| blob.contract_name.0 == pair.1)
        {
            Some(index) => BlobIndex(index as u32),
            None => {
                return RunResult {
                    success: false,
                    identity: self.caller.clone(),
                    program_outputs: format!(
                        "Blob with contract name {} not found in callees",
                        pair.0
                    )
                    .into_bytes(),
                }
            }
        };
        let blob_to = sdk::guest::parse_blob::<ERC20Action>(&callees_blobs, &blob_to_index);

        // Check that blobs are Transfers
        if !matches!(blob_from, ERC20Action::TransferFrom { .. })
            || !matches!(blob_to, ERC20Action::TransferFrom { .. })
        {
            return RunResult {
                success: false,
                identity: self.caller.clone(),
                program_outputs: "Transfer blobs do not call the correct function"
                    .to_string()
                    .into_bytes(),
            };
        }

        // Compute x,y and check swap is legit (x*y=k)
        let from_amount = match blob_from {
            ERC20Action::TransferFrom { amount, .. } => amount,
            _ => unreachable!(),
        };
        let to_amount = match blob_to {
            ERC20Action::TransferFrom { amount, .. } => amount,
            _ => unreachable!(),
        };
        match self.state.pairs.get(&pair) {
            Some((prev_x, prev_y)) => {
                if (prev_x + from_amount) * (prev_y - to_amount) != prev_x * prev_y {
                    return RunResult {
                        success: false,
                        identity: self.caller.clone(),
                        program_outputs: format!(
                            "Swap formula is not respected: {} * {} != {} * {}",
                            prev_x, prev_y, from_amount, to_amount
                        )
                        .into_bytes(),
                    };
                }
            }
            None => {
                return RunResult {
                    success: false,
                    identity: self.caller.clone(),
                    program_outputs: format!("Pair {:?} not found in AMM state", pair).into_bytes(),
                }
            }
        };

        RunResult {
            success: true,
            identity: self.caller.clone(),
            program_outputs: format!(
                "Swap of {} {} for {} {} is valid",
                from, pair.0, from, pair.1
            )
            .into_bytes(),
        }
    }
}

impl Digestable for AmmState {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(
            bincode::encode_to_vec(self, bincode::config::standard())
                .expect("Failed to encode AmmState"),
        )
    }
}
impl TryFrom<sdk::StateDigest> for AmmState {
    type Error = Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        let (amm_state, _) = bincode::decode_from_slice(&state.0, bincode::config::standard())
            .map_err(|_| anyhow::anyhow!("Could not decode start height"))?;
        Ok(amm_state)
    }
}

/// Enum representing the actions that can be performed by the Amm contract.
#[derive(Encode, Decode, Debug, Clone)]
pub enum AmmAction {
    Swap {
        from: Identity,
        pair: TokenPair, // User swaps the first token of the pair for the second token
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdk::{erc20::ERC20Action, Blob, ContractName, Identity};
    use std::collections::HashMap;

    fn create_test_blob(contract_name: &str, amount: u128) -> Blob {
        let cf = ERC20Action::TransferFrom {
            sender: "test".to_owned(),
            recipient: "test".to_owned(),
            amount,
        };
        (ContractName(contract_name.to_owned()), cf).into()
    }

    #[test]
    fn test_verify_swap_success() {
        let state = AmmState {
            pairs: HashMap::from([(("token1".to_string(), "token2".to_string()), (20, 50))]),
        };
        let caller = Identity::default();
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", 5),
            create_test_blob("token2", 10),
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(result.success);
    }

    #[test]
    fn test_verify_swap_caller_mismatch() {
        let state = AmmState {
            pairs: HashMap::from([(("token1".to_string(), "token2".to_string()), (20, 50))]),
        };
        let caller = Identity::default();
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", 5),
            create_test_blob("token2", 10),
        ];

        let other_caller = Identity("heehee".to_owned()); // Different identity
        let result = contract.verify_swap(
            blobs,
            other_caller,
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_pair() {
        let state = AmmState {
            pairs: HashMap::from([(("token1".to_string(), "token2".to_string()), (20, 50))]),
        };
        let caller = Identity::default();
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", 20),
            create_test_blob("token2", 50),
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "rubbish".to_string()), // Invalid pair
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_pair_in_blobs() {
        let state = AmmState {
            pairs: HashMap::from([(("token1".to_string(), "token2".to_string()), (20, 50))]),
        };
        let caller = Identity::default();
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", 20),
            create_test_blob("token1", 50), // Invalid pair, same token
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_blob_not_found() {
        let state = AmmState {
            pairs: HashMap::from([(("token1".to_string(), "token2".to_string()), (20, 50))]),
        };
        let caller = Identity::default();
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![create_test_blob("token1", 20)];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_swap_formula() {
        let state = AmmState {
            pairs: HashMap::from([(("token1".to_string(), "token2".to_string()), (20, 50))]),
        };
        let caller = Identity::default();
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", 25), // Invalid amount
            create_test_blob("token2", 50),
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }
}
