use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use borsh::{BorshDeserialize, BorshSerialize};
use sdk::caller::{CalleeBlobs, CallerCallee, CheckCalleeBlobs, ExecutionContext, MutCalleeBlobs};
use sdk::erc20::{ERC20BlobChecker, ERC20};
use sdk::{erc20::ERC20Action, Identity};
use sdk::{
    Blob, BlobIndex, ContractAction, ContractInput, Digestable, HyleContract, RunResult,
    StateDigest,
};
use sdk::{BlobData, ContractName, StructuredBlobData};
use serde::{Deserialize, Serialize};

#[cfg(feature = "client")]
pub mod client;

type TokenPair = (String, String);
type TokenPairAmount = (u128, u128);

#[derive(
    Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Ord, PartialOrd,
)]
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
    pub exec_ctx: ExecutionContext,
    state: AmmState,
}

impl CallerCallee for AmmContract {
    fn caller(&self) -> &Identity {
        &self.exec_ctx.caller
    }
    fn callee_blobs(&self) -> CalleeBlobs {
        CalleeBlobs(self.exec_ctx.callees_blobs.borrow())
    }
    fn mut_callee_blobs(&self) -> MutCalleeBlobs {
        MutCalleeBlobs(self.exec_ctx.callees_blobs.borrow_mut())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Default)]
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

    pub fn as_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode AmmState")
    }
}

impl HyleContract<AmmState, AmmAction> for AmmContract {
    fn execute_action(&mut self, action: AmmAction, _: &ContractInput) -> RunResult<AmmState>
    where
        Self: Sized,
    {
        let output = match action {
            AmmAction::Swap {
                pair,
                amounts: (from_amount, to_amount),
            } => self.verify_swap(pair, from_amount, to_amount),
            AmmAction::NewPair { pair, amounts } => self.create_new_pair(pair, amounts),
        };
        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, self.state.clone(), vec![])),
        }
    }

    fn init(state: AmmState, exec_ctx: ExecutionContext) -> Self {
        AmmContract { state, exec_ctx }
    }

    fn state(self) -> AmmState {
        self.state
    }
}

impl AmmContract {
    pub fn create_new_pair(
        &mut self,
        pair: (String, String),
        amounts: TokenPairAmount,
    ) -> Result<String, String> {
        // Check that new pair is about two different tokens and that there is one blob for each
        if pair.0 == pair.1 {
            return Err("Swap can only happen between two different tokens".to_string());
        }

        // Check that a blob exists matching the given action, pop it from the callee blobs.
        self.is_in_callee_blobs(
            &ContractName(pair.0.clone()),
            ERC20Action::TransferFrom {
                sender: self.caller().0.clone(),
                recipient: self.exec_ctx.contract_name.0.clone(),
                amount: amounts.0,
            },
        )?;

        // Sugared version using traits
        ERC20BlobChecker::new(&ContractName(pair.1.clone()), &self).transfer_from(
            &self.caller().0,
            &self.exec_ctx.contract_name.0,
            amounts.1,
        )?;

        let normalized_pair = UnorderedTokenPair::new(pair.0, pair.1);

        if self.state.pairs.contains_key(&normalized_pair) {
            return Err(format!("Pair {:?} already exists", normalized_pair));
        }

        let program_outputs = format!("Pair {:?} created", normalized_pair);

        self.state.pairs.insert(normalized_pair, amounts);

        Ok(program_outputs)
    }

    pub fn verify_swap(
        &mut self,
        pair: TokenPair,
        from_amount: u128,
        to_amount: u128,
    ) -> Result<String, String> {
        // Check that swap is only about two different tokens
        if pair.0 == pair.1 {
            return Err("Swap can only happen between two different tokens".to_string());
        }

        // Check that we were transferred the correct amount of tokens
        self.is_in_callee_blobs(
            &ContractName(pair.0.clone()),
            ERC20Action::TransferFrom {
                sender: self.caller().0.clone(),
                recipient: self.exec_ctx.contract_name.0.clone(),
                amount: from_amount,
            },
        )?;

        // Compute x,y and check swap is legit (x*y=k)
        let normalized_pair = UnorderedTokenPair::new(pair.0.clone(), pair.1.clone());
        let is_normalized_order = pair.0 <= pair.1;
        let Some((prev_x, prev_y)) = self.state.pairs.get_mut(&normalized_pair) else {
            return Err(format!("Pair {:?} not found in AMM state", pair));
        };
        let expected_to_amount = if is_normalized_order {
            let amount = *prev_y - (*prev_x * *prev_y / (*prev_x + from_amount));
            *prev_x += from_amount;
            *prev_y -= amount; // we need to remove the full amount to avoid slipping
            amount
        } else {
            let amount = *prev_x - (*prev_y * *prev_x / (*prev_y + from_amount));
            *prev_y += from_amount;
            *prev_x -= amount; // we need to remove the full amount to avoid slipping
            amount
        };

        // Assert that we transferred less than that, within 2%
        if to_amount > expected_to_amount || to_amount < expected_to_amount * 98 / 100 {
            return Err(format!(
                "Invalid swap: expected to receive between {} and {} {}",
                expected_to_amount * 100 / 102,
                expected_to_amount,
                pair.1
            ));
        }

        // At this point the contract has effectively taken some fees but we don't actually count them.

        // Check that we transferred the correct amount of tokens
        self.is_in_callee_blobs(
            &ContractName(pair.1.clone()),
            ERC20Action::Transfer {
                recipient: self.caller().0.clone(),
                amount: to_amount,
            },
        )?;

        Ok(format!(
            "Swap of {} {} for {} {} is valid",
            self.caller(),
            pair.0,
            self.caller(),
            pair.1
        ))
    }
}

impl Digestable for AmmState {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(self.as_bytes())
    }
}

impl TryFrom<StateDigest> for AmmState {
    type Error = anyhow::Error;

    fn try_from(state: StateDigest) -> Result<Self, Self::Error> {
        borsh::from_slice(&state.0).map_err(|_| anyhow::anyhow!("Could not decode amm state"))
    }
}

/// Enum representing the actions that can be performed by the Amm contract.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub enum AmmAction {
    Swap {
        pair: TokenPair, // User swaps the first token of the pair for the second token
        amounts: TokenPairAmount,
    },
    NewPair {
        pair: TokenPair,
        amounts: TokenPairAmount,
    },
}

impl ContractAction for AmmAction {
    fn as_blob(
        &self,
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
    use sdk::{erc20::ERC20Action, Blob, ContractAction, ContractName, Identity};
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
        cf.as_blob(ContractName::new(contract_name), None, None)
    }
    fn create_test_blob(contract_name: &str, recipient: &str, amount: u128) -> Blob {
        let cf = ERC20Action::Transfer {
            recipient: recipient.to_owned(),
            amount,
        };
        cf.as_blob(ContractName::new(contract_name), None, None)
    }

    #[test]
    fn test_verify_swap_success() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };
        println!(
            "default state: {:?}",
            AmmState {
                pairs: BTreeMap::default(),
            }
            .as_digest()
        );

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob("token2", "test", 10),
        ];
        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token1".to_string(), "token2".to_string()), 5, 10);
        assert!(result.is_ok());
        // Assert that the amounts for the pair token1/token2 have been updated
        assert_eq!(
            contract.state.pairs.get(&normalized_token_pair),
            Some(&(25, 40))
        );
    }

    #[test]
    fn test_verify_opposite_swap_success() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        // Swaping from token2 to token1
        let callees_blobs = vec![
            create_test_blob_from("token2", "test", "amm", 50),
            create_test_blob("token1", "test", 10),
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token2".to_string(), "token1".to_string()), 50, 10);

        assert!(result.is_ok());
        // Assert that the amounts for the pair token1/token2 have been updated
        println!("{:?}", contract.state.pairs.get(&normalized_token_pair));
        assert!(contract.state.pairs.get(&normalized_token_pair) == Some(&(10, 100)));
    }

    #[test]
    fn test_verify_swap_success_with_slippage() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (2000, 5000))]),
        };
        println!(
            "default state: {:?}",
            AmmState {
                pairs: BTreeMap::default(),
            }
            .as_digest()
        );

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 500),
            create_test_blob("token2", "test", 980),
        ];
        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token1".to_string(), "token2".to_string()), 500, 980);
        assert!(result.is_ok());
        assert_eq!(
            contract.state.pairs.get(&normalized_token_pair),
            Some(&(2500, 4000)) // 20 of token2 taken as 'fees'
        );
    }

    #[test]
    fn test_verify_swap_invalid_sender() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test_hack", "amm", 5), // incorrect sender
            create_test_blob("token2", "test", 10),
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token1".to_string(), "token2".to_string()), 5, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_swap_invalid_recipient() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob("token2", "test_hack", 10), // incorrect recipient
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token1".to_string(), "token2".to_string()), 5, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_swap_invalid_amm_recipient() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm_hack", 5), // incorrect recipient
            create_test_blob("token2", "test", 10),
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token1".to_string(), "token2".to_string()), 5, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_swap_invalid_pair() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob("token2", "test", 10),
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(
            ("token1".to_string(), "rubbish".to_string()), // Invalid pair
            5,
            10,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_swap_invalid_pair_in_blobs() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 5),
            create_test_blob("token1", "test", 10), // Invalid pair, same token
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token1".to_string(), "token2".to_string()), 5, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_swap_blob_not_found() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let callees_blobs = vec![create_test_blob_from("token1", "test", "amm", 5)];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token1".to_string(), "token2".to_string()), 5, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_swap_invalid_swap_formula() {
        let normalized_token_pair =
            UnorderedTokenPair::new("token1".to_string(), "token2".to_string());
        let state = AmmState {
            pairs: BTreeMap::from([(normalized_token_pair.clone(), (20, 50))]),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 0), // Invalid amount
            create_test_blob("token2", "test", 50),            // Invalid amount
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.verify_swap(("token1".to_string(), "token2".to_string()), 0, 50);
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .contains("Invalid swap: expected to receive"));
    }

    #[test]
    fn test_create_new_pair_success() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 20),
            create_test_blob_from("token2", "test", "amm", 50),
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };

        let mut contract = AmmContract::init(state, exec_ctx);
        let result =
            contract.create_new_pair(("token1".to_string(), "token2".to_string()), (20, 50));
        assert_eq!(contract.callee_blobs().len(), 0);

        println!("result: {:?}", result);
        assert!(result.is_ok());
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

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 20),
            create_test_blob_from("token2", "test", "amm", 50),
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result =
            contract.create_new_pair(("token1".to_string(), "token2".to_string()), (20, 50));

        assert!(result.is_err());
        assert!(contract.state.pairs.get(&normalized_token_pair) == Some(&(100, 200)));
    }

    #[test]
    fn test_create_new_pair_invalid_recipient() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "invalid_recipient", 20), // incorrect recipient
            create_test_blob_from("token2", "test", "amm", 50),
        ];
        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result =
            contract.create_new_pair(("token1".to_string(), "token2".to_string()), (20, 50));

        assert!(result.is_err());
    }

    #[test]
    fn test_create_new_pair_invalid_amounts() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 10), // incorrect amount
            create_test_blob_from("token2", "test", "amm", 50),
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result =
            contract.create_new_pair(("token1".to_string(), "token2".to_string()), (20, 50));

        assert!(result.is_err());
    }

    #[test]
    fn test_create_new_pair_same_tokens() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let callees_blobs = vec![
            create_test_blob_from("token1", "test", "amm", 20),
            create_test_blob_from("token1", "test", "amm", 50),
        ];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result = contract.create_new_pair(
            ("token1".to_string(), "token1".to_string()), // same tokens
            (20, 50),
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_create_new_pair_blob_not_found() {
        let state = AmmState {
            pairs: BTreeMap::new(),
        };

        let callees_blobs = vec![create_test_blob_from("token1", "test", "amm", 20)];

        let exec_ctx = ExecutionContext {
            callees_blobs: callees_blobs.into(),
            caller: Identity::new("test"),
            contract_name: ContractName("amm".to_string()),
        };
        let mut contract = AmmContract::init(state, exec_ctx);

        let result =
            contract.create_new_pair(("token1".to_string(), "token2".to_string()), (20, 50));

        assert!(result.is_err());
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
