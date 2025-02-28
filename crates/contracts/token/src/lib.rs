use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use sdk::{utils::parse_contract_input, ContractInput, Digestable, HyleContract, RunResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
pub mod qvac;

/// Struct representing the Hyllar token.
#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone)]
pub struct Token {
    balances_commitment: [u8; 32],
}

#[derive(Serialize, Deserialize, BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq)]
pub enum TokenAction {
    Transfer {
        sender: (String, u128),
        sender_proof: BTreeSet<[u8; 32]>,
        receiver: (String, u128),
        receiver_proof: BTreeSet<[u8; 32]>,
        amount: u128,
    },
}

impl Token {
    pub fn new(initial_supply: u128, faucet_id: String) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(faucet_id.as_bytes());
        hasher.update(initial_supply.to_le_bytes());
        let balances_commitment = hasher.finalize().into();

        Token {
            balances_commitment,
        }
    }

    pub fn verify_balance_in_commitment(
        &self,
        identity: &String,
        amount: u128,
        inclusion_proof: BTreeSet<[u8; 32]>,
    ) -> bool {
        let mut hash = Sha256::new();
        hash.update(identity.as_bytes());
        hash.update(amount.to_le_bytes());
        let mut computed_hash: [u8; 32] = hash.finalize().into();

        for sibling in inclusion_proof {
            let mut hasher = Sha256::new();
            hasher.update(computed_hash);
            hasher.update(sibling);
            computed_hash = hasher.finalize().into();
        }

        computed_hash.to_vec() == self.balances_commitment
    }

    // TODO: generalize for k values to update in the merkle tree
    // TODO: make it a tooling function
    pub fn compute_new_merkle_root(
        sender_name: &String,
        sender_new_leaf: [u8; 32],
        mut sender_proof: BTreeSet<[u8; 32]>,
        receiver_name: &String,
        receiver_new_leaf: [u8; 32],
        mut receiver_proof: BTreeSet<[u8; 32]>,
    ) -> [u8; 32] {
        while let (Some(&last_sender), Some(&last_receiver)) =
            (sender_proof.last(), receiver_proof.last())
        {
            if last_sender == last_receiver {
                sender_proof.pop_last();
                receiver_proof.pop_last();
            } else {
                break;
            }
        }

        // Remove last element as they will be hashed together
        sender_proof.pop_last();
        receiver_proof.pop_last();

        // Compute the new root hash after removing common elements
        let mut receiver_hash = receiver_new_leaf;
        while let Some(hash) = receiver_proof.pop_first() {
            let mut receiver_hasher = Sha256::new();
            receiver_hasher.update(receiver_hash);
            receiver_hasher.update(hash);
            receiver_hash = receiver_hasher.finalize().into();
        }

        let mut sender_hash = sender_new_leaf;
        while let Some(hash) = sender_proof.pop_first() {
            let mut sender_hasher = Sha256::new();
            sender_hasher.update(sender_hash);
            sender_hasher.update(hash);
            sender_hash = sender_hasher.finalize().into();
        }

        let mut root_hasher = Sha256::new();
        if sender_name < receiver_name {
            root_hasher.update(sender_hash);
            root_hasher.update(receiver_hash);
        } else {
            root_hasher.update(receiver_hash);
            root_hasher.update(sender_hash);
        }
        root_hasher.finalize().into()
    }

    pub fn transfer(
        &mut self,
        sender: (String, u128),
        sender_proof: BTreeSet<[u8; 32]>,
        receiver: (String, u128),
        receiver_proof: BTreeSet<[u8; 32]>,
        amount: u128,
    ) -> Result<String, String> {
        let (sender_name, sender_old_balance) = sender;
        let (receiver_name, receiver_old_balance) = receiver;
        if sender_old_balance < amount {
            return Err("Insufficient balance".to_string());
        }

        if !self.verify_balance_in_commitment(
            &sender_name,
            sender_old_balance,
            sender_proof.clone(),
        ) {
            return Err("Invalid sender balance proof".to_string());
        }
        if !self.verify_balance_in_commitment(
            &receiver_name,
            receiver_old_balance,
            receiver_proof.clone(),
        ) {
            return Err("Invalid receiver balance proof".to_string());
        }

        let mut sender_hasher = Sha256::new();
        sender_hasher.update(sender_name.as_bytes());
        sender_hasher.update(amount.to_le_bytes());
        let sender_new_leaf: [u8; 32] = sender_hasher.finalize().into();

        let mut receiver_hasher = Sha256::new();
        receiver_hasher.update(sender_name.as_bytes());
        receiver_hasher.update(amount.to_le_bytes());
        let receiver_new_leaf: [u8; 32] = receiver_hasher.finalize().into();

        // Compute new balances_commitment
        let new_root = Self::compute_new_merkle_root(
            &sender_name,
            sender_new_leaf,
            sender_proof,
            &receiver_name,
            receiver_new_leaf,
            receiver_proof,
        );

        self.balances_commitment = new_root;
        Ok(format!(
            "Transfered {} from {} to {}",
            amount, sender_name, receiver_name
        ))
    }
}

impl HyleContract for Token {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, execution_ctx) = parse_contract_input::<TokenAction>(contract_input)?;

        let output = match action {
            TokenAction::Transfer {
                sender,
                sender_proof,
                receiver,
                receiver_proof,
                amount,
            } => self.transfer(sender, sender_proof, receiver, receiver_proof, amount),
        };

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, execution_ctx, vec![])),
        }
    }
}

impl Digestable for Token {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(self.balances_commitment.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use crate::qvac::Commitment;
    use crypto_bigint::U256;

    #[test]
    fn test_transfer() {
        // let initial_supply = 1000;
        // let mut token = Token::new(initial_supply, "faucet".to_string());

        // assert!(token.transfer("faucet", "recipient", 500).is_ok());
        // assert_eq!(token.balance_of("faucet").unwrap(), 500);
        // assert_eq!(token.balance_of("recipient").unwrap(), 500);

        // assert!(token.transfer("faucet", "recipient", 600).is_err());
    }

    #[test]
    fn test_keygen_insert_update_verify() {
        // Générer les paramètres de KeyGen
        let mut commitment = Commitment::keygen(128);

        // Insérer une nouvelle paire (k, v)
        let key = "test_key";
        let value = U256::from_u32(42);
        let proof = commitment.insert(key, &value);

        // Vérifier l'engagement initial
        assert!(
            commitment.verify(key, &value, proof.clone()),
            "Verification failed"
        );

        // Mettre à jour la valeur associée à la clé
        let delta = U256::from_u32(10);
        let updated_proof = commitment.update(key, &delta, proof);

        // Nouvelle valeur après mise à jour
        let new_value = value + delta;

        // Vérifier l'engagement après mise à jour
        assert!(commitment.verify(key, &new_value, updated_proof));
    }
}
