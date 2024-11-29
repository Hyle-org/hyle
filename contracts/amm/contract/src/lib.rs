use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use anyhow::Error;
use bincode::{Decode, Encode};
use sdk::{erc20::ERC20Action, Identity};
use sdk::{guest::RunResult, Blob, BlobIndex, Digestable};
use sdk::{BlobData, ContractName, StructuredBlobData};
use serde::{Deserialize, Serialize};

type TokenPair = (String, String);
type TokenPairAmount = (u128, u128);

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Ord, PartialOrd)]
pub struct UnorderedTokenPair {
    a: String,
    b: String,
}

impl UnorderedTokenPair {
    pub fn new(x: String, y: String) -> Self {
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

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Default)]
pub struct AmmState {
    pairs: BTreeMap<UnorderedTokenPair, TokenPairAmount>,
}

impl AmmState {
    pub fn new(pairs: BTreeMap<UnorderedTokenPair, TokenPairAmount>) -> Self {
        AmmState { pairs }
    }

    pub fn get_paired_amount(
        &self,
        token_a: String,
        token_b: String,
        amount_a: u128,
    ) -> Option<u128> {
        let pair = UnorderedTokenPair::new(token_a, token_b);
        if let Some((k, x, y)) = self.pairs.get(&pair).map(|(x, y)| (x * y, *x, *y)) {
            let amount_b = y - (k / (x + amount_a));
            return Some(amount_b);
        }
        None
    }
}

impl AmmContract {
    pub fn new(state: AmmState, caller: Identity) -> Self {
        AmmContract { state, caller }
    }

    pub fn state(self) -> AmmState {
        self.state
    }

    pub fn create_new_pair(
        &mut self,
        pair: (String, String),
        amounts: TokenPairAmount,
        callees_blobs: Vec<Blob>,
    ) -> RunResult {
        // Check that new pair is about two different tokens and that there is one blob for each
        if callees_blobs.len() != 2 || pair.0 == pair.1 {
            return RunResult::failure("Swap can only happen between two different tokens");
        }

        let first_blob_index = match callees_blobs
            .iter()
            .position(|blob| blob.contract_name.0 == pair.0)
        {
            Some(index) => BlobIndex(index as u32),
            None => {
                return RunResult::failure("Blob with contract name {} not found in callees");
            }
        };

        let second_blob_index = match callees_blobs
            .iter()
            .position(|blob| blob.contract_name.0 == pair.1)
        {
            Some(index) => BlobIndex(index as u32),
            None => {
                return RunResult::failure("Blob with contract name {} not found in callees");
            }
        };

        // For each blob:
        // Check that from is the caller
        let first_blob =
            sdk::guest::parse_structured_blob::<ERC20Action>(&callees_blobs, &first_blob_index);
        match first_blob.data.parameters {
            ERC20Action::Transfer { amount, recipient } => {
                // Check that recipient is AMM
                if recipient != *"amm" {
                    return RunResult::failure("Transfer blob has incorrect sender or recipient");
                }
                // Check that the amount is the same as the one given
                if amount != amounts.0 {
                    return RunResult::failure("Amounts do not match");
                }
            }
            _ => {
                // Check the blobs are Transfer
                return RunResult::failure("Transfer blobs do not call the correct function");
            }
        };

        let second_blob =
            sdk::guest::parse_structured_blob::<ERC20Action>(&callees_blobs, &second_blob_index);
        match second_blob.data.parameters {
            ERC20Action::Transfer { amount, recipient } => {
                // Check that recipient is AMM
                if recipient != *"amm" {
                    return RunResult::failure("Transfer blob has incorrect sender or recipient");
                }
                // Check that the amount is the same as the one given
                if amount != amounts.1 {
                    return RunResult::failure("Amounts do not match");
                }
            }
            _ => {
                // Check the blobs are Transfer
                return RunResult::failure("Transfer blobs do not call the correct function");
            }
        };

        let normalized_pair = UnorderedTokenPair::new(pair.0, pair.1);

        if self.state.pairs.contains_key(&normalized_pair) {
            return RunResult::failure(&format!("Pair {:?} already exists", normalized_pair));
        }

        let program_outputs = format!("Pair {:?} created", normalized_pair);

        self.state.pairs.insert(normalized_pair, amounts);

        RunResult::success(&program_outputs)
    }

    pub fn verify_swap(&mut self, callees_blobs: Vec<Blob>, pair: TokenPair) -> RunResult {
        // Check that swap is only about two different tokens and that there is one blob for each
        if callees_blobs.len() != 2 || pair.0 == pair.1 {
            return RunResult::failure("Swap can only happen between two different tokens");
        }

        // Extract "blob_from" out of the blobs. This is the blob that has the first token of the pair as contract name
        let blob_from_index = match callees_blobs
            .iter()
            .position(|blob| blob.contract_name.0 == pair.0)
        {
            Some(index) => BlobIndex(index as u32),
            None => {
                return RunResult::failure(&format!(
                    "Blob with contract name {} not found in callees",
                    pair.0
                ));
            }
        };
        let blob_from =
            sdk::guest::parse_structured_blob::<ERC20Action>(&callees_blobs, &blob_from_index);

        // Extract "blob_to" out of the blobs. This is the blob that has the second token of the pair as contract name
        let blob_to_index = match callees_blobs
            .iter()
            .position(|blob| blob.contract_name.0 == pair.1)
        {
            Some(index) => BlobIndex(index as u32),
            None => {
                return RunResult::failure(&format!(
                    "Blob with contract name {} not found in callees",
                    pair.0
                ));
            }
        };
        let blob_to =
            sdk::guest::parse_structured_blob::<ERC20Action>(&callees_blobs, &blob_to_index);

        let from_amount = match blob_from.data.parameters {
            ERC20Action::TransferFrom {
                sender,
                amount,
                recipient,
            } => {
                // Check that blob_from 'from' field matches the caller and that 'to' field is the AMM
                if sender != self.caller.0 || recipient != *"amm" {
                    return RunResult::failure(
                        "Blob 'from' field is not the User or 'to' field is not the AMM",
                    );
                }
                amount
            }
            _ => {
                // Check the blobs are TransferFrom
                return RunResult::failure("Transfer blobs do not call the correct function");
            }
        };

        let to_amount = match blob_to.data.parameters {
            ERC20Action::Transfer { amount, recipient } => {
                // Check that blob_to 'from' field is the AMM and that 'to' field matches the caller
                if recipient != self.caller.0 {
                    return RunResult::failure(
                        "Blob 'from' field is not the AMM or 'to' field is not the caller",
                    );
                }
                amount
            }
            _ => {
                // Check the blobs are TransferFrom
                return RunResult::failure("Transfer blobs do not call the correct function");
            }
        };

        // Compute x,y and check swap is legit (x*y=k)
        let normalized_pair = UnorderedTokenPair::new(pair.0.clone(), pair.1.clone());
        let is_normalized_order = pair.0 <= pair.1;
        match self.state.pairs.get_mut(&normalized_pair) {
            Some((prev_x, prev_y)) => {
                if is_normalized_order {
                    let amount_b = *prev_y - (*prev_x * *prev_y / (*prev_x + from_amount));
                    if amount_b != to_amount {
                        return RunResult::failure(&format!(
                            "Swap formula is not respected: {} * {} != {} * {}",
                            (*prev_x + from_amount),
                            (*prev_y - to_amount),
                            prev_x,
                            prev_y,
                        ));
                    }
                    *prev_x += from_amount;
                    *prev_y -= to_amount;
                } else {
                    let amount_b = *prev_x - (*prev_y * *prev_x / (*prev_y + from_amount));
                    if amount_b != to_amount {
                        return RunResult::failure(&format!(
                            "Swap formula is not respected: {} * {} != {} * {}",
                            (*prev_y + from_amount),
                            (*prev_x - to_amount),
                            prev_y,
                            prev_x
                        ));
                    }
                    *prev_y += from_amount;
                    *prev_x -= to_amount;
                }
            }
            None => {
                return RunResult::failure(&format!("Pair {:?} not found in AMM state", pair));
            }
        };
        RunResult::success(&format!(
            "Swap of {} {} for {} {} is valid",
            self.caller, pair.0, self.caller, pair.1
        ))
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
        pair: TokenPair, // User swaps the first token of the pair for the second token
    },
    NewPair {
        pair: TokenPair,
        amounts: TokenPairAmount,
    },
}

impl AmmAction {
    pub fn as_blob(
        self,
        contract_name: ContractName,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        Blob {
            contract_name,
            data: BlobData::from(StructuredBlobData {
                caller,
                callees,
                parameters: self.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdk::{erc20::ERC20Action, Blob, ContractName, Identity};
    use std::collections::BTreeMap;

    fn create_test_blob_from(
        contract_name: &str,
        sender: &str,
        recipient: &str,
        amount: u128,
    ) -> Blob {
        let cf = ERC20Action::TransferFrom {
            sender: sender.to_owned(),
            recipient: recipient.to_owned(),
            amount,
        };
        cf.as_blob(ContractName(contract_name.to_owned()), None, None)
    }
    fn create_test_blob(contract_name: &str, recipient: &str, amount: u128) -> Blob {
        let cf = ERC20Action::Transfer {
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
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob("token2", "test", 10),
        ];

        let result =
            contract.verify_swap(callees_blobs, ("token1".to_string(), "token2".to_string()));
        assert!(result.success);
        // Assert that the amounts for the pair token1/token2 have been updated
        assert!(contract.state.pairs.get(&normalized_token_pair) == Some(&(25, 40)));
    }

    #[test]
    fn test_verify_opposite_swap_success() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        // Swaping from token2 to token1
        let callees_blobs = vec![
            create_test_blob_from("token2", "test", "amm", 50),
            create_test_blob("token1", "test", 10),
        ];

        let result =
            contract.verify_swap(callees_blobs, ("token2".to_string(), "token1".to_string()));

        assert!(result.success);
        // Assert that the amounts for the pair token1/token2 have been updated
        println!("{:?}", contract.state.pairs.get(&normalized_token_pair));
        assert!(contract.state.pairs.get(&normalized_token_pair) == Some(&(10, 100)));
    }

    #[test]
    fn test_verify_swap_invalid_sender() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test_hack", "amm", 5), // incorrect sender
            create_test_blob("token2", "test", 10),
        ];

        let result =
            contract.verify_swap(callees_blobs, ("token1".to_string(), "token2".to_string()));
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_recipient() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob("token2", "test_hack", 10), // incorrect recipient
        ];

        let result =
            contract.verify_swap(callees_blobs, ("token1".to_string(), "token2".to_string()));
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_amm_recipient() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm_hack", 5), // incorrect recipient
            create_test_blob("token2", "test", 10),
        ];

        let result =
            contract.verify_swap(callees_blobs, ("token1".to_string(), "token2".to_string()));
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_pair() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob("token2", "test", 10),
        ];

        let result = contract.verify_swap(
            callees_blobs,
            ("token1".to_string(), "rubbish".to_string()), // Invalid pair
        );
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_pair_in_blobs() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob("token1", "test", 10), // Invalid pair, same token
        ];

        let result =
            contract.verify_swap(callees_blobs, ("token1".to_string(), "token2".to_string()));
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_blob_not_found() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![create_test_blob_from("token1", "test", "amm", 5)];

        let result =
            contract.verify_swap(callees_blobs, ("token1".to_string(), "token2".to_string()));
        assert!(!result.success);
    }

    #[test]
    fn test_verify_swap_invalid_swap_formula() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob_from("token2", "test", "amm", 15), // Invalid amount
        ];

        let result =
            contract.verify_swap(callees_blobs, ("token1".to_string(), "token2".to_string()));
        assert!(!result.success);
    }

    #[test]
    fn test_create_new_pair_success() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob("token1", "amm", 20),
            create_test_blob("token2", "amm", 50),
        ];

        let result = contract.create_new_pair(
            ("token1".to_string(), "token2".to_string()),
            (20, 50),
            callees_blobs,
        );

        assert!(result.success);
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        assert!(contract.state.pairs.get(&normalized_token_pair) == Some(&(20, 50)));
    }

    #[test]
    fn test_create_new_pair_already_exists() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(
                UnorderedTokenPair::new("token1".to_string(), "token2".to_string()),
                (100, 200),
            )]),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 20),
            create_test_blob_from("token2", "test", "amm", 50),
        ];
        let result = contract.create_new_pair(
            ("token1".to_string(), "token2".to_string()),
            (20, 50),
            callees_blobs,
        );

        assert!(!result.success);
        assert!(contract.state.pairs.get(&normalized_token_pair) == Some(&(100, 200)));
    }

    #[test]
    fn test_create_new_pair_invalid_sender() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "invalid_sender", "amm", 20), // incorrect sender
            create_test_blob_from("token2", "test", "amm", 50),
        ];

        let result = contract.create_new_pair(
            ("token1".to_string(), "token2".to_string()),
            (20, 50),
            callees_blobs,
        );

        assert!(!result.success);
    }

    #[test]
    fn test_create_new_pair_invalid_recipient() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "invalid_recipient", 20), // incorrect recipient
            create_test_blob_from("token2", "test", "amm", 50),
        ];

        let result = contract.create_new_pair(
            ("token1".to_string(), "token2".to_string()),
            (20, 50),
            callees_blobs,
        );

        assert!(!result.success);
    }

    #[test]
    fn test_create_new_pair_invalid_amounts() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 10), // incorrect amount
            create_test_blob_from("token2", "test", "amm", 50),
        ];

        let result = contract.create_new_pair(
            ("token1".to_string(), "token2".to_string()),
            (20, 50),
            callees_blobs,
        );

        assert!(!result.success);
    }

    #[test]
    fn test_create_new_pair_same_tokens() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 20),
            create_test_blob_from("token1", "test", "amm", 50),
        ];

        let result = contract.create_new_pair(
            ("token1".to_string(), "token1".to_string()), // same tokens
            (20, 50),
            callees_blobs,
        );

        assert!(!result.success);
    }

    #[test]
    fn test_create_new_pair_blob_not_found() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let caller = Identity("test".to_owned());
        let mut contract = AmmContract::new(state, caller.clone());

        let callees_blobs = vec![create_test_blob_from("token1", "test", "amm", 20)];

        let result = contract.create_new_pair(
            ("token1".to_string(), "token2".to_string()),
            (20, 50),
            callees_blobs,
        );

        assert!(!result.success);
    }

    #[test]
    fn test_get_paired_amount_existing_pair() {
        let pair = UnorderedTokenPair::new("token1".into(), "token2".into());
        let mut pairs = BTreeMap::new();
        pairs.insert(pair.clone(), (10, 20));
        let state = AmmState { pairs };

        let result = state.get_paired_amount("token1".to_string(), "token2".to_string(), 5);

        assert!(result.is_some());
        let amount_b = result.unwrap();
        assert_eq!(amount_b, 20 - (10 * 20 / (10 + 5)));
    }

    #[test]
    fn test_get_paired_amount_non_existing_pair() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let result = state.get_paired_amount("token1".to_string(), "token2".to_string(), 5);

        assert!(result.is_none());
    }

    #[test]
    fn test_get_paired_amount_zero_amount_a() {
        let pair = UnorderedTokenPair::new("token1".into(), "token2".into());
        let mut pairs = BTreeMap::new();
        pairs.insert(pair.clone(), (10, 20));
        let state = AmmState { pairs };

        let result = state.get_paired_amount("token1".to_string(), "token2".to_string(), 0);

        assert!(result.is_some());
        let amount_b = result.unwrap();
        assert_eq!(amount_b, 20 - (10 * 20 / 10));
    }

    #[test]
    fn test_get_paired_amount_division_by_zero() {
        let pair = UnorderedTokenPair::new("token1".into(), "token2".into());
        let mut pairs = BTreeMap::new();
        pairs.insert(pair.clone(), (0, 20));
        let state = AmmState { pairs };

        let result = state.get_paired_amount("token1".to_string(), "token2".to_string(), 5);

        assert!(result.is_some());
        let amount_b = result.unwrap();
        assert_eq!(amount_b, 20);
    }
}
