use account::Account;
use borsh::{BorshDeserialize, BorshSerialize};
use sdk::utils::parse_contract_input;
use sdk::{
    Blob, BlobData, BlobIndex, ContractAction, ContractInput, ContractName, StateCommitment,
    StructuredBlobData,
};
use sdk::{HyleContract, RunResult};
use serde::{Deserialize, Serialize};
use sparse_merkle_tree::traits::Value;
use state::SmtTokenState;
use utils::{BorshableMerkleProof, SHA256Hasher};

extern crate alloc;

pub mod account;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod indexer;
pub mod state;
pub mod utils;

pub const TOTAL_SUPPLY: u128 = 100_000_000_000;
pub const FAUCET_ID: &str = "faucet.hydentity";

/// Enum representing possible calls to Token contract functions.
#[derive(Clone, PartialEq, BorshDeserialize, BorshSerialize)]
pub enum SmtTokenAction {
    Transfer {
        proof: BorshableMerkleProof, // TODO: à mettre en private_input
        sender_account: Account,
        recipient_account: Account,
        amount: u128,
    },
    TransferFrom {
        proof: BorshableMerkleProof,
        owner_account: Account,
        spender: String,
        recipient_account: Account,
        amount: u128,
    },
    Approve {
        proof: BorshableMerkleProof,
        owner_account: Account,
        spender: String,
        amount: u128,
    },
}

/// Struct representing the SMT token.
#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone)]
pub struct SmtToken {
    pub commitment: sdk::StateCommitment,
}

impl Default for SmtToken {
    fn default() -> Self {
        let default_state = SmtTokenState::default();
        SmtToken {
            commitment: default_state.commit(),
        }
    }
}

impl SmtToken {
    pub fn new(commitment: sdk::StateCommitment) -> Self {
        SmtToken { commitment }
    }
}

impl HyleContract for SmtToken {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, execution_ctx) = parse_contract_input::<SmtTokenAction>(contract_input)?;
        let output = match action {
            SmtTokenAction::Transfer {
                proof,
                sender_account,
                recipient_account,
                amount,
            } => self.transfer(proof, sender_account, recipient_account, amount),
            SmtTokenAction::TransferFrom {
                proof,
                owner_account,
                spender,
                recipient_account,
                amount,
            } => self.transfer_from(proof, owner_account, spender, recipient_account, amount),
            SmtTokenAction::Approve {
                proof,
                owner_account,
                spender,
                amount,
            } => self.approve(proof, owner_account, spender, amount),
        };

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, execution_ctx, vec![])),
        }
    }

    fn commit(&self) -> sdk::StateCommitment {
        self.commitment.clone()
    }
}

impl SmtToken {
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }
}

impl SmtToken {
    pub fn transfer(
        &mut self,
        proof: BorshableMerkleProof,
        mut sender_account: Account,
        mut recipient_account: Account,
        amount: u128,
    ) -> Result<String, String> {
        let sender_key = sender_account.get_key();
        let recipient_key = recipient_account.get_key();

        let verified = proof
            .0
            .clone()
            .verify::<SHA256Hasher>(
                &TryInto::<[u8; 32]>::try_into(self.commitment.0.clone())
                    .unwrap()
                    .into(),
                vec![
                    (sender_key, sender_account.to_h256()),
                    (recipient_key, recipient_account.to_h256()),
                ],
            )
            .expect("Failed to verify proof");

        if !verified {
            return Err("Failed to verify proof".to_string());
        }

        // update sender and recipient balances
        sender_account.balance -= amount;
        recipient_account.balance += amount;

        let new_root = proof
            .0
            .compute_root::<SHA256Hasher>(vec![
                (sender_key, sender_account.to_h256()),
                (recipient_key, recipient_account.to_h256()),
            ])
            .expect("Failed to compute new root");

        self.commitment = StateCommitment(Into::<[u8; 32]>::into(new_root).to_vec());

        Ok(format!(
            "Transferred {} to {}",
            amount, recipient_account.address
        ))
    }

    pub fn transfer_from(
        &mut self,
        proof: BorshableMerkleProof,
        owner_account: Account,
        spender: String,
        recipient_account: Account,
        amount: u128,
    ) -> Result<String, String> {
        if owner_account.allowances.get(&spender).unwrap_or(&0) < &amount {
            return Err(format!(
                "Allowance exceeded for spender={} owner={} allowance={}",
                spender,
                owner_account.address,
                owner_account.allowances.get(&spender).unwrap_or(&0)
            ));
        }

        self.transfer(proof, owner_account, recipient_account, amount)
        // TODO: update allowance
    }

    pub fn approve(
        &mut self,
        proof: BorshableMerkleProof,
        mut owner_account: Account,
        spender: String,
        amount: u128,
    ) -> Result<String, String> {
        let owner_key = owner_account.get_key();

        let verified = proof
            .0
            .clone()
            .verify::<SHA256Hasher>(
                &TryInto::<[u8; 32]>::try_into(self.commitment.0.clone())
                    .unwrap()
                    .into(),
                vec![(owner_key, owner_account.to_h256())],
            )
            .expect("Failed to verify proof");

        if !verified {
            return Err("Failed to verify proof".to_string());
        }

        owner_account.update_allowances(spender.clone(), amount);

        let new_root = proof
            .0
            .compute_root::<SHA256Hasher>(vec![(owner_key, owner_account.to_h256())])
            .expect("Failed to compute new root");

        self.commitment = StateCommitment(Into::<[u8; 32]>::into(new_root).to_vec());
        Ok(format!("Approved {} to {}", amount, spender))
    }
}

impl ContractAction for SmtTokenAction {
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
    use crate::account::AccountSMT;

    use super::*;

    #[test_log::test]
    fn test_smt_token_transfer() {
        // Create a new empty SMT
        let mut smt = AccountSMT::default();

        // Create some test accounts
        let mut account1 = Account::new("faucet".to_string(), 10000);
        let mut account2 = Account::new("alice".to_string(), 100);

        // Create keys for the accounts
        let key1 = account1.get_key();
        let key2 = account2.get_key();

        // Insert accounts into SMT
        smt.update(key1, account1.clone())
            .expect("Failed to update SMT");
        smt.update(key2, account2.clone())
            .expect("Failed to update SMT");

        // Generate merkle proof for account1
        let proof = smt
            .merkle_proof(vec![key1, key2])
            .expect("Failed to generate proof");

        // Compute initial root
        let root = *smt.root();
        let mut smt_token = SmtToken::new(StateCommitment(Into::<[u8; 32]>::into(root).to_vec()));

        // Verify the existence proof
        let verified = proof
            .clone()
            .verify::<SHA256Hasher>(
                &root,
                vec![(key1, account1.to_h256()), (key2, account2.to_h256())],
            )
            .expect("Failed to verify proof");

        assert!(verified);

        // Transfer 100 tokens from account1 to account2 in the contract
        smt_token
            .transfer(
                BorshableMerkleProof(proof),
                account1.clone(),
                account2.clone(),
                100,
            )
            .unwrap();

        // Transfer 100 tokens from account1 to account2
        account1.balance -= 100;
        account2.balance += 100;
        let expected_root = smt
            .update_all(vec![
                (account1.get_key(), account1),
                (account2.get_key(), account2),
            ])
            .unwrap();

        assert_eq!(
            StateCommitment(Into::<[u8; 32]>::into(*expected_root).to_vec()),
            smt_token.commit()
        );
    }

    #[test_log::test]
    fn test_smt_token_new_account_tranfer() {
        // Create a new empty SMT
        let mut smt = AccountSMT::default();

        // Create some test accounts
        let mut account1 = Account::new("faucet".to_string(), 10000);
        let mut account2 = Account::new("alice".to_string(), 0);

        // Create keys for the accounts
        let key1 = account1.get_key();
        let key2 = account2.get_key();

        // Insert accounts into SMT
        smt.update(key1, account1.clone())
            .expect("Failed to update SMT");

        // Generate merkle proof for account1
        let proof = smt
            .merkle_proof(vec![key1, key2])
            .expect("Failed to generate proof");

        // Compute initial root
        let root = *smt.root();
        let mut smt_token = SmtToken::new(StateCommitment(Into::<[u8; 32]>::into(root).to_vec()));

        // Verify the existence proof
        let verified = proof
            .clone()
            .verify::<SHA256Hasher>(
                smt.root(),
                vec![(key1, account1.to_h256()), (key2, account2.to_h256())],
            )
            .expect("Failed to verify proof");

        assert!(verified);

        // Double-check that the account really doesn't exist
        let value = smt.get(&key2).expect("Failed to get value");
        assert_eq!(value, Account::default());

        // Transfer 100 tokens from account1 to account2 in the contract
        smt_token
            .transfer(
                BorshableMerkleProof(proof),
                account1.clone(),
                account2.clone(),
                100,
            )
            .unwrap();

        // Transfer 100 tokens from account1 to account2
        account1.balance -= 100;
        account2.balance += 100;
        let expected_root = smt
            .update_all(vec![
                (account1.get_key(), account1),
                (account2.get_key(), account2),
            ])
            .unwrap();

        assert_eq!(
            StateCommitment(Into::<[u8; 32]>::into(*expected_root).to_vec()),
            smt_token.commit()
        );
    }

    #[test_log::test]
    fn test_smt_token_transfer_from() {
        // Create a new empty SMT
        let mut smt = AccountSMT::default();

        // Create some test accounts
        let mut owner_account = Account::new("owner".to_string(), 10000);
        let mut recipient_account = Account::new("recipient".to_string(), 0);
        let spender = "spender".to_string();

        // Set allowance for spender
        owner_account.update_allowances(spender.clone(), 500);

        // Create keys for the accounts
        let owner_key = owner_account.get_key();
        let recipient_key = recipient_account.get_key();

        // Insert account into SMT
        smt.update(owner_key, owner_account.clone())
            .expect("Failed to update SMT");

        // Generate merkle proof for the accounts
        let proof = smt
            .merkle_proof(vec![owner_key, recipient_key])
            .expect("Failed to generate proof");

        // Compute initial root
        let root = *smt.root();
        let mut smt_token = SmtToken::new(StateCommitment(Into::<[u8; 32]>::into(root).to_vec()));

        // Verify the existence proof
        let verified = proof
            .clone()
            .verify::<SHA256Hasher>(
                &root,
                vec![
                    (owner_key, owner_account.to_h256()),
                    (recipient_key, recipient_account.to_h256()),
                ],
            )
            .expect("Failed to verify proof");

        assert!(verified);

        // Transfer 200 tokens from owner to recipient via spender in the contract
        smt_token
            .transfer_from(
                BorshableMerkleProof(proof),
                owner_account.clone(),
                spender.clone(),
                recipient_account.clone(),
                200,
            )
            .unwrap();

        // Update balances and allowance
        owner_account.balance -= 200;
        recipient_account.balance += 200;

        let expected_root = smt
            .update_all(vec![
                (owner_account.get_key(), owner_account),
                (recipient_account.get_key(), recipient_account),
            ])
            .unwrap();

        assert_eq!(
            StateCommitment(Into::<[u8; 32]>::into(*expected_root).to_vec()),
            smt_token.commit()
        );
    }

    #[test_log::test]
    fn test_smt_token_approve() {
        // Create a new empty SMT
        let mut smt = AccountSMT::default();

        // Create a test account
        let mut owner_account = Account::new("owner".to_string(), 10000);
        let spender = "spender".to_string();

        // Create key for the account
        let owner_key = owner_account.get_key();

        // Insert account into SMT
        smt.update(owner_key, owner_account.clone())
            .expect("Failed to update SMT");

        // Generate merkle proof for the account
        let proof = smt
            .merkle_proof(vec![owner_key])
            .expect("Failed to generate proof");

        // Compute initial root
        let root = *smt.root();
        let mut smt_token = SmtToken::new(StateCommitment(Into::<[u8; 32]>::into(root).to_vec()));

        // Verify the existence proof
        let verified = proof
            .clone()
            .verify::<SHA256Hasher>(&root, vec![(owner_key, owner_account.to_h256())])
            .expect("Failed to verify proof");

        assert!(verified);

        // Approve 500 tokens for spender in the contract
        smt_token
            .approve(
                BorshableMerkleProof(proof),
                owner_account.clone(),
                spender.clone(),
                500,
            )
            .unwrap();

        // Update allowance
        owner_account.update_allowances(spender.clone(), 500);

        let expected_root = smt
            .update_all(vec![(owner_account.get_key(), owner_account)])
            .unwrap();

        assert_eq!(
            StateCommitment(Into::<[u8; 32]>::into(*expected_root).to_vec()),
            smt_token.commit()
        );
    }
}
