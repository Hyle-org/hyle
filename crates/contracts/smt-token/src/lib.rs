use account::Account;
use borsh::{BorshDeserialize, BorshSerialize};
use sdk::utils::parse_calldata;
use sdk::{
    Blob, BlobData, BlobIndex, Calldata, ContractAction, ContractName, StateCommitment,
    StructuredBlobData,
};
use sdk::{RunResult, ZkContract};
use sparse_merkle_tree::traits::Value;
use utils::{BorshableMerkleProof, SHA256Hasher};

extern crate alloc;

pub mod account;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod indexer;
pub mod utils;

pub const TOTAL_SUPPLY: u128 = 100_000_000_000;
pub const FAUCET_ID: &str = "faucet.hydentity";

/// Enum representing possible calls to Token contract functions.
#[derive(Clone, PartialEq, BorshDeserialize, BorshSerialize)]
pub enum SmtTokenAction {
    Transfer {
        sender_account: Account,
        recipient_account: Account,
        amount: u128,
    },
    TransferFrom {
        owner_account: Account,
        spender: String,
        recipient_account: Account,
        amount: u128,
    },
    Approve {
        owner_account: Account,
        spender: String,
        amount: u128,
    },
}

/// Struct representing the SMT token.
/// Each attributes of this struct is what is needed in order to verify the state of the contract, and update it.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct SmtTokenContract {
    pub commitment: sdk::StateCommitment,
    pub proof: BorshableMerkleProof,
}

impl SmtTokenContract {
    pub fn new(commitment: sdk::StateCommitment, proof: BorshableMerkleProof) -> Self {
        SmtTokenContract { commitment, proof }
    }
}

impl ZkContract for SmtTokenContract {
    fn execute(&mut self, calldata: &Calldata) -> RunResult {
        let (action, execution_ctx) = parse_calldata::<SmtTokenAction>(calldata)?;
        let output = match action {
            SmtTokenAction::Transfer {
                sender_account,
                recipient_account,
                amount,
            } => self.transfer(sender_account, recipient_account, amount),
            SmtTokenAction::TransferFrom {
                owner_account,
                spender,
                recipient_account,
                amount,
            } => self.transfer_from(owner_account, spender, recipient_account, amount),
            SmtTokenAction::Approve {
                owner_account,
                spender,
                amount,
            } => self.approve(owner_account, spender, amount),
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

impl SmtTokenContract {
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }
}

impl SmtTokenContract {
    pub fn transfer(
        &mut self,
        mut sender_account: Account,
        mut recipient_account: Account,
        amount: u128,
    ) -> Result<String, String> {
        let sender_key = sender_account.get_key();
        let recipient_key = recipient_account.get_key();

        let verified = self
            .proof
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

        let new_root = self
            .proof
            .0
            .clone()
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

        self.transfer(owner_account, recipient_account, amount)
        // TODO: update allowance
    }

    pub fn approve(
        &mut self,
        mut owner_account: Account,
        spender: String,
        amount: u128,
    ) -> Result<String, String> {
        let owner_key = owner_account.get_key();

        let verified = self
            .proof
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

        let new_root = self
            .proof
            .0
            .clone()
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
        smt.0
            .update(key1, account1.clone())
            .expect("Failed to update SMT");
        smt.0
            .update(key2, account2.clone())
            .expect("Failed to update SMT");

        // Generate merkle proof for account1
        let proof = smt
            .0
            .merkle_proof(vec![key1, key2])
            .expect("Failed to generate proof");

        // Compute initial root
        let root = *smt.0.root();
        let mut smt_token = SmtTokenContract::new(
            StateCommitment(Into::<[u8; 32]>::into(root).to_vec()),
            BorshableMerkleProof(proof.clone()),
        );

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
            .transfer(account1.clone(), account2.clone(), 100)
            .unwrap();

        // Transfer 100 tokens from account1 to account2
        account1.balance -= 100;
        account2.balance += 100;
        let expected_root = smt
            .0
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
        smt.0
            .update(key1, account1.clone())
            .expect("Failed to update SMT");

        // Generate merkle proof for account1
        let proof = smt
            .0
            .merkle_proof(vec![key1, key2])
            .expect("Failed to generate proof");

        // Compute initial root
        let root = *smt.0.root();
        let mut smt_token = SmtTokenContract::new(
            StateCommitment(Into::<[u8; 32]>::into(root).to_vec()),
            BorshableMerkleProof(proof.clone()),
        );

        // Verify the existence proof
        let verified = proof
            .clone()
            .verify::<SHA256Hasher>(
                smt.0.root(),
                vec![(key1, account1.to_h256()), (key2, account2.to_h256())],
            )
            .expect("Failed to verify proof");

        assert!(verified);

        // Double-check that the account really doesn't exist
        let value = smt.0.get(&key2).expect("Failed to get value");
        assert_eq!(value, Account::default());

        // Transfer 100 tokens from account1 to account2 in the contract
        smt_token
            .transfer(account1.clone(), account2.clone(), 100)
            .unwrap();

        // Transfer 100 tokens from account1 to account2
        account1.balance -= 100;
        account2.balance += 100;
        let expected_root = smt
            .0
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
        smt.0
            .update(owner_key, owner_account.clone())
            .expect("Failed to update SMT");

        // Generate merkle proof for the accounts
        let proof = smt
            .0
            .merkle_proof(vec![owner_key, recipient_key])
            .expect("Failed to generate proof");

        // Compute initial root
        let root = *smt.0.root();
        let mut smt_token = SmtTokenContract::new(
            StateCommitment(Into::<[u8; 32]>::into(root).to_vec()),
            BorshableMerkleProof(proof.clone()),
        );

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
            .0
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
        smt.0
            .update(owner_key, owner_account.clone())
            .expect("Failed to update SMT");

        // Generate merkle proof for the account
        let proof = smt
            .0
            .merkle_proof(vec![owner_key])
            .expect("Failed to generate proof");

        // Compute initial root
        let root = *smt.0.root();
        let mut smt_token = SmtTokenContract::new(
            StateCommitment(Into::<[u8; 32]>::into(root).to_vec()),
            BorshableMerkleProof(proof.clone()),
        );

        // Verify the existence proof
        let verified = proof
            .clone()
            .verify::<SHA256Hasher>(&root, vec![(owner_key, owner_account.to_h256())])
            .expect("Failed to verify proof");

        assert!(verified);

        // Approve 500 tokens for spender in the contract
        smt_token
            .approve(owner_account.clone(), spender.clone(), 500)
            .unwrap();

        // Update allowance
        owner_account.update_allowances(spender.clone(), 500);

        let expected_root = smt
            .0
            .update_all(vec![(owner_account.get_key(), owner_account)])
            .unwrap();

        assert_eq!(
            StateCommitment(Into::<[u8; 32]>::into(*expected_root).to_vec()),
            smt_token.commit()
        );
    }
}
