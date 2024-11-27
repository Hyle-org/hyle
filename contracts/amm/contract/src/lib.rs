use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use anyhow::Error;
use bincode::{Decode, Encode};
use sdk::{erc20::ERC20Action, Identity};
use sdk::{guest::RunResult, Blob, BlobIndex, Digestable};
use serde::Deserialize;

type TokenPair = (String, String);
type TokenPairAmount = (u128, u128);

#[derive(Debug, Deserialize, Clone, Encode, Decode)]
struct UnorderedTokenPair {
    a: String,
    b: String,
}

impl UnorderedTokenPair {
    fn new(x: String, y: String) -> Self {
        if x <= y {
            UnorderedTokenPair { a: x, b: y }
        } else {
            UnorderedTokenPair { a: y, b: x }
        }
    }
}

impl PartialEq for UnorderedTokenPair {
    fn eq(&self, other: &Self) -> bool {
        self.a == other.a && self.b == other.b
    }
}

impl Eq for UnorderedTokenPair {}

impl Hash for UnorderedTokenPair {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.a.hash(state);
        self.b.hash(state);
    }
}

pub struct AmmContract {
    state: AmmState,
    caller: Identity,
}

#[derive(Deserialize, Clone, Encode, Decode)]
pub struct AmmState {
    pairs: HashMap<UnorderedTokenPair, TokenPairAmount>,
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

        let from_amount = match blob_from.data.parameters {
            ERC20Action::TransferFrom {
                sender,
                amount,
                recipient,
            } => {
                // Check that blob_from 'from' field matches the caller and that 'to' field is the AMM
                if sender != from.0 || recipient != *"amm" {
                    return RunResult {
                        success: false,
                        identity: self.caller.clone(),
                        program_outputs:
                            "Blob 'from' field is not the User or 'to' field is not the AMM"
                                .to_string()
                                .into_bytes(),
                    };
                }
                amount
            }
            _ => {
                // Check the blobs are TransferFrom
                return RunResult {
                    success: false,
                    identity: self.caller.clone(),
                    program_outputs: "Transfer blobs do not call the correct function"
                        .to_string()
                        .into_bytes(),
                };
            }
        };

        let to_amount = match blob_to.data.parameters {
            ERC20Action::TransferFrom {
                sender,
                amount,
                recipient,
            } => {
                // Check that blob_to 'from' field is the AMM and that 'to' field matches the caller
                if sender != *"amm" || recipient != from.0 {
                    return RunResult {
                        success: false,
                        identity: self.caller.clone(),
                        program_outputs:
                            "Blob 'from' field is not the AMM or 'to' field is not the caller"
                                .to_string()
                                .into_bytes(),
                    };
                }
                amount
            }
            _ => {
                // Check the blobs are TransferFrom
                return RunResult {
                    success: false,
                    identity: self.caller.clone(),
                    program_outputs: "Transfer blobs do not call the correct function"
                        .to_string()
                        .into_bytes(),
                };
            }
        };

        // Compute x,y and check swap is legit (x*y=k)
        let normalized_pair = UnorderedTokenPair::new(pair.0.clone(), pair.1.clone());
        let is_normalized_order = pair.0 <= pair.1;
        match self.state.pairs.get_mut(&normalized_pair) {
            Some((prev_x, prev_y)) => {
                if is_normalized_order {
                    if (*prev_x + from_amount) * (*prev_y - to_amount) != *prev_x * *prev_y {
                        return RunResult {
                            success: false,
                            identity: self.caller.clone(),
                            program_outputs: format!(
                                "Swap formula is not respected: {} * {} != {} * {}",
                                (*prev_x + from_amount),
                                (*prev_y - to_amount),
                                prev_x,
                                prev_y
                            )
                            .into_bytes(),
                        };
                    }
                    *prev_x += from_amount;
                    *prev_y -= to_amount;
                } else {
                    if (*prev_y + from_amount) * (*prev_x - to_amount) != *prev_y * *prev_x {
                        return RunResult {
                            success: false,
                            identity: self.caller.clone(),
                            program_outputs: format!(
                                "Swap formula is not respected: {} * {} != {} * {}",
                                (*prev_y + from_amount),
                                (*prev_x - to_amount),
                                prev_y,
                                prev_x
                            )
                            .into_bytes(),
                        };
                    }
                    *prev_y += from_amount;
                    *prev_x -= to_amount;
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

    fn create_test_blob(contract_name: &str, sender: &str, recipient: &str, amount: u128) -> Blob {
        let cf = ERC20Action::TransferFrom {
            sender: sender.to_owned(),
            recipient: recipient.to_owned(),
            amount,
        };
        cf.as_blob(ContractName(contract_name.to_owned()), None, None)
    }

    #[test]
    fn test_verify_swap_success() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test", "amm", 5),
            create_test_blob("token2", "amm", "test", 10),
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(result.success);
        // Assert that the amounts for the pair token1/token2 have been updated
        assert!(contract.state.pairs.get(&normalized_token_pair) == Some(&(25, 40)));
    }

    #[test]
    fn test_verify_opposite_swap_success() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        // Swaping from token2 to token1
        let blobs = vec![
            create_test_blob("token2", "test", "amm", 50),
            create_test_blob("token1", "amm", "test", 10),
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token2".to_string(), "token1".to_string()),
        );

        assert!(result.success);
        // Assert that the amounts for the pair token1/token2 have been updated
        println!("{:?}", contract.state.pairs.get(&normalized_token_pair));
        assert!(contract.state.pairs.get(&normalized_token_pair) == Some(&(10, 100)));
    }

    #[test]
    fn test_verify_swap_caller_mismatch() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };
        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test", "amm", 5),
            create_test_blob("token2", "amm", "test", 10),
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
    fn test_verify_swap_invalid_sender() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test_hack", "amm", 5), // incorrect sender
            create_test_blob("token2", "amm", "test", 10),
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_amm_sender() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test", "amm", 5),
            create_test_blob("token2", "amm_hack", "test", 10), // incorrect sender
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_recipient() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test", "amm", 5),
            create_test_blob("token2", "amm", "test_hack", 10), // incorrect recipient
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_amm_recipient() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test", "amm_hack", 5), // incorrect recipient
            create_test_blob("token2", "amm", "test", 10),
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_pair() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test", "amm", 5),
            create_test_blob("token2", "amm", "test", 10),
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
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test", "amm", 5),
            create_test_blob("token1", "amm", "test", 10), // Invalid pair, same token
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
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![create_test_blob("token1", "test", "amm", 5)];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_swap_formula() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: HashMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let blobs = vec![
            create_test_blob("token1", "test", "amm", 5),
            create_test_blob("token2", "test", "amm", 15), // Invalid amount
        ];

        let result = contract.verify_swap(
            blobs,
            caller.clone(),
            ("token1".to_string(), "token2".to_string()),
        );
        assert!(!result.success);
    }
}
