//! # Hylé Contract SDK
//!
//! This crate contains tools to be used in smart contracts that runs on rust zkvm like Risc0 or
//! SP1.
//!
//! ## How to build a contract on Hyle ?
//!
//! To build a contract, you will need to create a contract lib, with a struct that implements
//! the [HyleContract] trait.
//!
//! Then you will need a zkvm binary that will execute this code. Take a look at the
//! [Guest module for contract zkvm](crate::guest).
//!
//! You can start from our templates for [Risc0](https://github.com/Hyle-org/template-risc0)
//! or [SP1](https://github.com/Hyle-org/template-sp1).
//!
//! If your contract needs to interact with other contracts, take a lookt at
//! [StructuredBlobData]. More is coming on that soon.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod caller;
pub mod guest;
pub mod utils;

use caller::ExecutionContext;
// re-export hyle-model
pub use hyle_model::*;

#[cfg(feature = "tracing")]
pub use tracing;

// Si la feature "tracing" est activée, on redirige vers `tracing::info!`
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::tracing::info!($($arg)*);
    }
}

// Si la feature "tracing" n’est pas activée, on redirige vers la fonction env::log
#[cfg(all(not(feature = "tracing"), feature = "risc0"))]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        risc0_zkvm::guest::env::log(&format!($($arg)*));
    }
}

#[cfg(all(not(feature = "tracing"), not(feature = "risc0")))]
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        println!($($arg)*);
    }
}

pub type RunResult = Result<(String, ExecutionContext, Vec<OnchainEffect>), String>;

/**
This trait is used to define the contract's entrypoint.
By using it and the [execute](function@crate::guest::execute) function, you let the sdk
generate for you the [HyleOutput] struct with correct fields.

The [ContractInput] struct is built by the application backend and given as input to
the zkvm.

The contract input is generic to any contract, and holds all the blobs of the blob transaction
being proved. These blobs are stored as vec of bytes, so contract need to parse them into the
expected type. For this, it can call either [utils::parse_raw_contract_input] or
[utils::parse_contract_input]. Check the [utils] documentation for details on these functions.

## Example of execute implementation:

```rust
struct MyContract{}
enum MyContractAction{
    DoSomething
}

impl HyleContract for MyContract {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, exec_ctx) = parse_raw_contract_input(contract_input)?;

        let output = self.execute_action(action)?;

        Ok((output, exec_ctx, vec![]))
    }
}

impl MyContract {
    fn execute_action(action: MyContractAction) -> Result<String, String> {
        /// Execute contract's logic
        Ok("Done.")
    }
}

```
*/
pub trait HyleContract {
    /// Entry point of the contract
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult;

    /// This function builds the on-chain state commitment of the contract
    /// It can compute a state hash, or a merkle root of the state, or any other commitment.
    /// The [StateCommitment] will be stored on chain, and will be used as initial_state for
    /// next contract execution.
    fn commit(&self) -> StateCommitment;
}

pub const fn to_u8_array(val: &[u32; 8]) -> [u8; 32] {
    [
        (val[0] & 0xFF) as u8,
        ((val[0] >> 8) & 0xFF) as u8,
        ((val[0] >> 16) & 0xFF) as u8,
        ((val[0] >> 24) & 0xFF) as u8,
        (val[1] & 0xFF) as u8,
        ((val[1] >> 8) & 0xFF) as u8,
        ((val[1] >> 16) & 0xFF) as u8,
        ((val[1] >> 24) & 0xFF) as u8,
        (val[2] & 0xFF) as u8,
        ((val[2] >> 8) & 0xFF) as u8,
        ((val[2] >> 16) & 0xFF) as u8,
        ((val[2] >> 24) & 0xFF) as u8,
        (val[3] & 0xFF) as u8,
        ((val[3] >> 8) & 0xFF) as u8,
        ((val[3] >> 16) & 0xFF) as u8,
        ((val[3] >> 24) & 0xFF) as u8,
        (val[4] & 0xFF) as u8,
        ((val[4] >> 8) & 0xFF) as u8,
        ((val[4] >> 16) & 0xFF) as u8,
        ((val[4] >> 24) & 0xFF) as u8,
        (val[5] & 0xFF) as u8,
        ((val[5] >> 8) & 0xFF) as u8,
        ((val[5] >> 16) & 0xFF) as u8,
        ((val[5] >> 24) & 0xFF) as u8,
        (val[6] & 0xFF) as u8,
        ((val[6] >> 8) & 0xFF) as u8,
        ((val[6] >> 16) & 0xFF) as u8,
        ((val[6] >> 24) & 0xFF) as u8,
        (val[7] & 0xFF) as u8,
        ((val[7] >> 8) & 0xFF) as u8,
        ((val[7] >> 16) & 0xFF) as u8,
        ((val[7] >> 24) & 0xFF) as u8,
    ]
}

const fn byte_to_u8(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}

pub const fn str_to_u8(s: &str) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let chrs = s.as_bytes();
    let mut i = 0;
    while i < 32 {
        bytes[i] = (byte_to_u8(chrs[i * 2]) << 4) | byte_to_u8(chrs[i * 2 + 1]);
        i += 1;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, string::ToString, vec};

    #[test]
    fn test_identity_from_string() {
        let identity_str = "test_identity".to_string();
        let identity = Identity::from(identity_str.clone());
        assert_eq!(identity.0, identity_str);
    }

    #[test]
    fn test_identity_from_str() {
        let identity_str = "test_identity";
        let identity = Identity::from(identity_str);
        assert_eq!(identity.0, identity_str.to_string());
    }

    #[test]
    fn test_txhash_from_string() {
        let txhash_str = "test_txhash".to_string();
        let txhash = TxHash::from(txhash_str.clone());
        assert_eq!(txhash.0, txhash_str);
    }

    #[test]
    fn test_txhash_from_str() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::from(txhash_str);
        assert_eq!(txhash.0, txhash_str.to_string());
    }

    #[test]
    fn test_txhash_new() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::new(txhash_str);
        assert_eq!(txhash.0, txhash_str.to_string());
    }

    #[test]
    fn test_blobindex_from_u32() {
        let index = 42;
        let blob_index = BlobIndex::from(index);
        assert_eq!(blob_index.0, index);
    }

    #[test]
    fn test_txhash_display() {
        let txhash_str = "test_txhash";
        let txhash = TxHash::new(txhash_str);
        assert_eq!(format!("{}", txhash), txhash_str);
    }

    #[test]
    fn test_blobindex_display() {
        let index = 42;
        let blob_index = BlobIndex::from(index);
        assert_eq!(format!("{}", blob_index), index.to_string());
    }

    #[test]
    fn test_state_commitment_encoding() {
        let state_commitment = StateCommitment(vec![1, 2, 3, 4]);
        let encoded = borsh::to_vec(&state_commitment).expect("Failed to encode StateCommitment");
        let decoded: StateCommitment =
            borsh::from_slice(&encoded).expect("Failed to decode StateCommitment");
        assert_eq!(state_commitment, decoded);
    }

    #[test]
    fn test_identity_encoding() {
        let identity = Identity::new("test_identity");
        let encoded = borsh::to_vec(&identity).expect("Failed to encode Identity");
        let decoded: Identity = borsh::from_slice(&encoded).expect("Failed to decode Identity");
        assert_eq!(identity, decoded);
    }

    #[test]
    fn test_txhash_encoding() {
        let txhash = TxHash::new("test_txhash");
        let encoded = borsh::to_vec(&txhash).expect("Failed to encode TxHash");
        let decoded: TxHash = borsh::from_slice(&encoded).expect("Failed to decode TxHash");
        assert_eq!(txhash, decoded);
    }

    #[test]
    fn test_blobindex_encoding() {
        let blob_index = BlobIndex(42);
        let encoded = borsh::to_vec(&blob_index).expect("Failed to encode BlobIndex");
        let decoded: BlobIndex = borsh::from_slice(&encoded).expect("Failed to decode BlobIndex");
        assert_eq!(blob_index, decoded);
    }

    #[test]
    fn test_blobdata_encoding() {
        let blob_data = BlobData(vec![1, 2, 3, 4]);
        let encoded = borsh::to_vec(&blob_data).expect("Failed to encode BlobData");
        let decoded: BlobData = borsh::from_slice(&encoded).expect("Failed to decode BlobData");
        assert_eq!(blob_data, decoded);
    }
}
