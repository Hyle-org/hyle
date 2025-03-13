use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use borsh::{BorshDeserialize, BorshSerialize};
use ipa_multipoint::committer::DefaultCommitter;
use sdk::utils::parse_contract_input;
use sdk::{
    Blob, BlobData, BlobIndex, ContractAction, ContractInput, ContractName, StructuredBlobData,
};
use sdk::{HyleContract, RunResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use verkle_trie::constants::new_crs;
use verkle_trie::database::memory_db::MemoryDb;
use verkle_trie::proof::stateless_updater::verify_and_update;
use verkle_trie::proof::VerkleProof;
use verkle_trie::{DefaultConfig, Element, Trie, TrieTrait};

extern crate alloc;

pub const TOTAL_SUPPLY: u128 = 100_000_000_000;
pub const FAUCET_ID: &str = "faucet.hydentity";

// FIXME: c de la merde
impl BorshSerialize for TokenAction {
    fn serialize<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        match self {
            TokenAction::Transfer {
                proof,
                sender_account,
                recipient_account,
                amount,
            } => {
                let mut proof_bytes = Vec::new();
                proof
                    .write(&mut proof_bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

                BorshSerialize::serialize(&0, writer)?;
                BorshSerialize::serialize(&proof_bytes, writer)?;
                BorshSerialize::serialize(&sender_account, writer)?;
                BorshSerialize::serialize(&recipient_account, writer)?;
                BorshSerialize::serialize(&amount, writer)?;
            }
            TokenAction::Approve {
                proof,
                owner,
                spender,
                amount,
            } => {
                let mut proof_bytes = Vec::new();
                proof
                    .write(&mut proof_bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

                BorshSerialize::serialize(&1, writer)?;
                BorshSerialize::serialize(&proof_bytes, writer)?;
                BorshSerialize::serialize(&owner, writer)?;
                BorshSerialize::serialize(&spender, writer)?;
                BorshSerialize::serialize(&amount, writer)?;
            }
        }
        Ok(())
    }
}

// FIXME: c de la merde
impl BorshDeserialize for TokenAction {
    fn deserialize_reader<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let variant_tag = u8::deserialize_reader(reader)?;
        match variant_tag {
            0 => {
                let proof_bytes: Vec<u8> = BorshDeserialize::deserialize_reader(reader)?;
                let sender_account: Account = BorshDeserialize::deserialize_reader(reader)?;
                let recipient_account: Account = BorshDeserialize::deserialize_reader(reader)?;
                let amount: u128 = BorshDeserialize::deserialize_reader(reader)?;

                let proof = VerkleProof::read(&mut Cursor::new(proof_bytes)).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid proof")
                })?;

                Ok(TokenAction::Transfer {
                    proof,
                    sender_account,
                    recipient_account,
                    amount,
                })
            }
            1 => {
                let proof_bytes: Vec<u8> = BorshDeserialize::deserialize_reader(reader)?;
                let owner: Account = BorshDeserialize::deserialize_reader(reader)?;
                let spender: String = BorshDeserialize::deserialize_reader(reader)?;
                let amount: u128 = BorshDeserialize::deserialize_reader(reader)?;

                let proof = VerkleProof::read(&mut Cursor::new(proof_bytes)).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid proof")
                })?;

                Ok(TokenAction::Approve {
                    proof,
                    owner,
                    spender,
                    amount,
                })
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unknown variant",
            )),
        }
    }
}

/// Enum representing possible calls to Token contract functions.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenAction {
    Transfer {
        proof: VerkleProof,
        sender_account: Account,
        recipient_account: Account,
        amount: u128,
    },
    Approve {
        proof: VerkleProof,
        owner: Account,
        spender: String,
        amount: u128,
    },
}

impl HyleContract for Token {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, execution_ctx) = parse_contract_input::<TokenAction>(contract_input)?;
        let output = match action {
            TokenAction::Transfer {
                proof,
                sender_account,
                recipient_account,
                amount,
            } => self.transfer(proof, sender_account, recipient_account, amount),
            TokenAction::Approve {
                proof,
                owner,
                spender,
                amount,
            } => self.approve(proof, owner, spender, amount),
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

/// Struct representing the Token token.
#[derive(Default, BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone)]
pub struct Token {
    commitment: sdk::StateCommitment,
}

#[derive(Debug, Clone, PartialEq, BorshDeserialize, BorshSerialize)]
pub struct Account {
    address: String,
    pub balance: u128,
    pub allowance: BTreeMap<String, u128>,
}

impl Account {
    pub fn pedersen_key_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.address.as_bytes());
        let mut result: [u8; 32] = hasher.finalize().into();
        // HACK: We add a suffix to the key to force every key to be in same subtree...
        result[0] = 0x00;
        result
    }

    pub fn value_hash(&self) -> Option<[u8; 32]> {
        if self.balance == 0 && self.allowance.is_empty() {
            // There is a distinction that exists by using the suffix in the key to differentiate 0 from empty
            // cf https://notes.ethereum.org/@vbuterin/verkle_tree_eip#Verkle-tree-definition
            return None;
        }
        let mut value_bytes = [0u8; 32];

        let mut hasher = Sha256::new();
        hasher.update(self.balance.to_le_bytes());
        hasher.update(self.allowance.len().to_le_bytes());
        for (spender, allowance) in self.allowance.iter() {
            hasher.update(spender.as_bytes());
            hasher.update(allowance.to_le_bytes());
        }

        let result = hasher.finalize();
        value_bytes.copy_from_slice(&result[..32]);

        Some(value_bytes)
    }

    pub fn key_value_hash(&self) -> ([u8; 32], Option<[u8; 32]>) {
        (self.pedersen_key_hash(), self.value_hash())
    }

    // FIXME: c de la merde
    pub fn update_balance(&mut self, amount: i64) {
        if amount < 0 {
            self.balance -= (-amount) as u128;
        } else {
            self.balance += amount as u128;
        }
    }

    pub fn update_allowance(&mut self, spender: String, amount: u128) {
        self.allowance.insert(spender, amount);
    }
}

impl Token {
    pub fn init_trie(faucet_account: Account) -> Trie<MemoryDb, DefaultCommitter> {
        let db = MemoryDb::new();
        let mut trie = Trie::new(DefaultConfig::new(db));

        let (key_bytes, value_bytes) = faucet_account.key_value_hash();

        trie.insert_single(key_bytes, value_bytes.unwrap());

        trie
    }

    pub fn new(trie: Trie<MemoryDb, DefaultCommitter>) -> Self {
        let root_hash = trie.root_commitment().to_bytes().to_vec();
        Self {
            commitment: sdk::StateCommitment(root_hash),
        }
    }

    pub fn transfer(
        &mut self,
        proof: VerkleProof,
        mut sender_account: Account,
        mut recipient_account: Account,
        amount: u128,
    ) -> Result<String, String> {
        if sender_account.balance < amount {
            return Err("Insufficient balance".to_string());
        }

        let (sender_key, sender_value) = sender_account.key_value_hash();
        let (recipient_key, recipient_value) = recipient_account.key_value_hash();

        let keys = vec![sender_key, recipient_key];
        let values = vec![sender_value, recipient_value];

        sender_account.balance -= amount;
        recipient_account.balance += amount;

        let updated_sender_value = sender_account.value_hash();
        let updated_recipient_value = recipient_account.value_hash();

        let updated_values = vec![updated_sender_value, updated_recipient_value];

        let root = Element::from_bytes(&self.commitment.0).unwrap();

        match verify_and_update(
            proof,
            root,
            keys,
            values,
            updated_values,
            DefaultCommitter::new(&new_crs().G),
        ) {
            Ok(update_root) => {
                self.commitment = sdk::StateCommitment(update_root.to_bytes().to_vec());
                Ok(format!(
                    "Transferred {} to {}",
                    amount, recipient_account.address
                ))
            }
            Err(err) => Err(format!("Verification or update of the proof failed: {err}")),
        }
    }

    fn approve(
        &mut self,
        proof: VerkleProof,
        mut owner: Account,
        spender: String,
        amount: u128,
    ) -> Result<String, String> {
        let (owner_key, owner_value) = owner.key_value_hash();

        let keys = vec![owner_key];
        let values = vec![owner_value];

        owner.update_allowance(spender.clone(), amount);

        let updated_owner_value = owner.value_hash();

        let updated_values = vec![updated_owner_value];

        let root = Element::from_bytes(&self.commitment.0).unwrap();

        match verify_and_update(
            proof,
            root,
            keys,
            values,
            updated_values,
            DefaultCommitter::new(&new_crs().G),
        ) {
            Ok(update_root) => {
                self.commitment = sdk::StateCommitment(update_root.to_bytes().to_vec());
                Ok(format!("Approved {} to {}", amount, spender))
            }
            Err(err) => Err(format!("Verification or update of the proof failed: {err}")),
        }
    }
}

impl ContractAction for TokenAction {
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
    use std::vec;

    use super::*;

    #[test_log::test]
    fn test_token_verkle_tree_transfer() {
        let mut faucet_account = Account {
            address: FAUCET_ID.to_string(),
            balance: TOTAL_SUPPLY,
            allowance: BTreeMap::new(),
        };
        let mut token_trie = Token::init_trie(faucet_account.clone());
        let (faucet_key, faucet_value) = faucet_account.key_value_hash();

        // Adding 100 token to alice
        let mut alice_account = Account {
            address: "alice".to_string(),
            balance: 100,
            allowance: BTreeMap::new(),
        };
        let (alice_key, alice_value) = alice_account.key_value_hash();
        token_trie.insert_single(alice_key, alice_value.unwrap());

        assert_eq!(token_trie.get(faucet_key), faucet_value);
        assert_eq!(token_trie.get(alice_key), alice_value);

        let proof = token_trie
            .create_verkle_proof(vec![faucet_key, alice_key].into_iter())
            .unwrap();

        let mut token = Token::new(token_trie.clone());

        // Transfer 100 token to alice on commitment
        let res = token.transfer(proof, faucet_account.clone(), alice_account.clone(), 10);
        assert!(res.is_ok(), "{}", res.unwrap_err());

        // Send 10 token to alice on trie
        alice_account.update_balance(10);
        faucet_account.update_balance(-10);
        let alice_new_value = alice_account.value_hash().unwrap();
        let faucet_new_value = faucet_account.value_hash().unwrap();
        token_trie
            .insert(vec![(alice_key, alice_new_value), (faucet_key, faucet_new_value)].into_iter());

        // Assert commitment is updated accordingly to trie
        assert_eq!(
            token.commitment.0,
            token_trie.root_commitment().to_bytes().to_vec()
        );
    }

    #[test_log::test]
    fn test_token_verkle_tree_transfer_to_unknown_address() {
        let mut faucet_account = Account {
            address: FAUCET_ID.to_string(),
            balance: TOTAL_SUPPLY,
            allowance: BTreeMap::new(),
        };
        let mut token_trie = Token::init_trie(faucet_account.clone());
        let (faucet_key, faucet_value) = faucet_account.key_value_hash();

        let mut alice_account = Account {
            address: "alice".to_string(),
            balance: 0,
            allowance: BTreeMap::new(),
        };
        let alice_key = alice_account.pedersen_key_hash();

        assert_eq!(token_trie.get(faucet_key), faucet_value);
        assert_eq!(token_trie.get(alice_key), None);

        let proof = token_trie
            .create_verkle_proof(vec![faucet_key, alice_key].into_iter())
            .unwrap();

        let mut token = Token::new(token_trie.clone());

        // Transfer 100 token to alice on commitment
        let res = token.transfer(proof, faucet_account.clone(), alice_account.clone(), 10);
        assert!(res.is_ok(), "{}", res.unwrap_err());

        // Send 10 token to alice on trie
        alice_account.update_balance(10);
        faucet_account.update_balance(-10);
        let alice_new_value = alice_account.value_hash().unwrap();
        let faucet_new_value = faucet_account.value_hash().unwrap();
        token_trie
            .insert(vec![(alice_key, alice_new_value), (faucet_key, faucet_new_value)].into_iter());

        // Assert commitment is updated accordingly to trie
        assert_eq!(
            token.commitment.0,
            token_trie.root_commitment().to_bytes().to_vec()
        );
    }

    #[test_log::test]
    fn test_token_verkle_tree_approve() {
        let faucet_account = Account {
            address: FAUCET_ID.to_string(),
            balance: TOTAL_SUPPLY,
            allowance: BTreeMap::new(),
        };
        let mut token_trie = Token::init_trie(faucet_account);
        let mut faucet_account = Account {
            address: FAUCET_ID.to_string(),
            balance: TOTAL_SUPPLY,
            allowance: BTreeMap::new(),
        };
        let faucet_key = faucet_account.pedersen_key_hash();

        let proof = token_trie
            .create_verkle_proof(vec![faucet_key].into_iter())
            .unwrap();

        let mut token = Token::new(token_trie.clone());

        // Approve 10 token to alice on faucet account in commitment
        let res = token.approve(proof, faucet_account.clone(), "alice".to_string(), 10);
        assert!(res.is_ok(), "{}", res.unwrap_err());

        // Approve 10 token to alice on trie
        faucet_account.update_allowance("alice".to_string(), 10);
        let faucet_new_value = faucet_account.value_hash().unwrap();
        token_trie.insert(vec![(faucet_key, faucet_new_value)].into_iter());

        // Assert commitment is updated accordingly to trie
        assert_eq!(
            token.commitment.0,
            token_trie.root_commitment().to_bytes().to_vec()
        );
    }
}
