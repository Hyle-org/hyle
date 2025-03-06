#![allow(clippy::needless_doctest_main)]
/*!
The `guest` module provides the `GuestEnv` trait that defines a common interface for the guests accross different zkvm.
It provides `Risc0Env` and `SP1Env` structs that implement the `GuestEnv` trait for both zkvm.

The [execute] function is used to execute an action on a given contract using the provided state and contract input.

The `fail` function is used to generate a failure output for a contract action.

## Example usage of this module for risc0:

This is a code snippet of a Risc0 guest entrypoint (e.g. file `methods/guest/src/main.rs` in a risc0 template project).

`Hydentity` struct has to implemetns [HyleContract], [Digestable] and [BorshDeserialize] traits.

```rust,no_run
#![no_main]

use hyle_hydentity::Hydentity;
use sdk::guest::{execute, GuestEnv, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
   let env = Risc0Env {};
   let contract_input = env.read();
   let (_, output) = execute::<Hydentity>(&contract_input);
   env.commit(&output);
}
```

## Example usage of this module for sp1:

This is a code snippet of a SP1 guest entrypoint (e.g. file `program/src/main.rs` in a sp1 template project).


`IdentityContractState` struct has to implemetns [HyleContract], [Digestable] and [BorshDeserialize] traits.

```rust,no_run
#![no_main]

use contract::IdentityContractState;
use sdk::guest::execute;
use sdk::guest::GuestEnv;
use sdk::guest::SP1Env;

sp1_zkvm::entrypoint!(main);

fn main() {
    let env = SP1Env {};
    let input = env.read();
    let (_, output) = execute::<IdentityContractState>(&input);
    env.commit(&output);
}
```

*/

use alloc::string::ToString;
use alloc::vec;
use borsh::BorshDeserialize;
use hyle_model::StateDigest;

use crate::{flatten_blobs, utils::as_hyle_output, ContractInput, Digestable, HyleOutput};
use crate::{HyleContract, RunResult};

pub trait GuestEnv {
    fn log(&self, message: &str);
    fn commit(&self, output: &HyleOutput);
    fn read<T: BorshDeserialize + 'static>(&self) -> T;
}

pub struct Risc0Env;

#[cfg(feature = "risc0")]
impl GuestEnv for Risc0Env {
    fn log(&self, message: &str) {
        risc0_zkvm::guest::env::log(message);
    }

    fn commit(&self, output: &HyleOutput) {
        risc0_zkvm::guest::env::commit(output);
    }

    fn read<T: BorshDeserialize>(&self) -> T {
        let len: usize = risc0_zkvm::guest::env::read();
        let mut slice = vec![0u8; len];
        risc0_zkvm::guest::env::read_slice(&mut slice);
        borsh::from_slice(&slice).unwrap()
    }
}

pub struct SP1Env;

// For coverage tests, assume risc0 if both are active
#[cfg(all(feature = "sp1", not(feature = "risc0")))]
impl GuestEnv for SP1Env {
    fn log(&self, message: &str) {
        // TODO: this does nothing actually
        sp1_zkvm::io::hint(&message);
    }

    fn commit(&self, output: &HyleOutput) {
        let vec = borsh::to_vec(&output).unwrap();
        sp1_zkvm::io::commit_slice(&vec);
    }

    fn read<T: BorshDeserialize>(&self) -> T {
        let vec = sp1_zkvm::io::read_vec();
        borsh::from_slice(&vec).unwrap()
    }
}

pub fn fail(input: ContractInput, initial_state_digest: StateDigest, message: &str) -> HyleOutput {
    HyleOutput {
        version: 1,
        initial_state: initial_state_digest.clone(),
        next_state: initial_state_digest,
        identity: input.identity,
        index: input.index,
        blobs: flatten_blobs(&input.blobs),
        success: false,
        tx_hash: input.tx_hash,
        tx_ctx: input.tx_ctx,
        onchain_effects: vec![],
        program_outputs: message.to_string().into_bytes(),
    }
}

/// Executes an action on a given contract using the provided state and contract input.
///
/// # Arguments
///
/// * `contract_input` - A reference to the contract input that contains the current state, blobs, identity, etc.
///
/// # Type Parameters
///
/// * `State` - The type of the state that must implement the `Digestable`, `BorshDeserialize`, `HyleContract` traits.
///
/// # Returns
///
/// A pair containing the new state and the contract output as `HyleOutput`.
///
/// # Panics
///
/// Panics if the contract initialization fails.
pub fn execute<State>(contract_input: &ContractInput) -> (State, HyleOutput)
where
    State: HyleContract + Digestable + BorshDeserialize + 'static,
{
    let mut state: State =
        borsh::from_slice(&contract_input.state).expect("Failed to decode state");
    let initial_state_digest = state.as_digest();

    let mut res: RunResult = state.execute(contract_input);

    let next_state_digest = state.as_digest();

    let output = as_hyle_output::<State>(
        initial_state_digest,
        next_state_digest,
        contract_input.clone(),
        &mut res,
    );

    (state, output)
}
