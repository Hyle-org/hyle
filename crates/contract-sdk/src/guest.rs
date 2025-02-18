use alloc::string::{String, ToString};
use alloc::vec;
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_model::{DropEndOfReader, StateDigest, StructuredBlobData};

use crate::caller::{CallerCallee, ExecutionContext};
use crate::{
    flatten_blobs,
    utils::{as_hyle_output, check_caller_callees, parse_blob, parse_structured_blob},
    ContractInput, Digestable, HyleOutput,
};
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
        borsh::from_slice(&slice).unwrap()
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
        registered_contracts: vec![],
        program_outputs: message.to_string().into_bytes(),
    }
}

pub fn init<Parameters>(input: &ContractInput) -> Result<(Parameters, ExecutionContext), String>
where
    Parameters: BorshSerialize + BorshDeserialize,
{
    let parsed_blob = parse_structured_blob::<Parameters>(&input.blobs, &input.index);

    let parsed_blob = parsed_blob.ok_or("Failed to parse input blob".to_string())?;

    let caller = check_caller_callees::<Parameters>(input, &parsed_blob)?;

    let mut callees_blobs = vec::Vec::new();
    for blob in input.blobs.clone().into_iter() {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<DropEndOfReader> = structured_blob; // for type inference
            if structured_blob.caller == Some(input.index) {
                callees_blobs.push(blob);
            }
        };
    }

    let ctx = ExecutionContext {
        callees_blobs: callees_blobs.into(),
        caller,
        contract_name: parsed_blob.contract_name.clone(),
    };

    Ok((parsed_blob.data.parameters, ctx))
}

/// Executes an action on a given contract using the provided state and contract input.
///
/// # Arguments
///
/// * `contract_input` - A reference to the contract input that contains the current state, blobs, identity, etc.
///
/// # Type Parameters
///
/// * `Contract` - The type of the contract that implements the `HyleContract` and `CallerCallee` traits.
/// * `State` - The type of the state that must implement the `Digestable` and `BorshDeserialize` traits.
/// * `Action` - The type of the action that must implement the `BorshSerialize` and `BorshDeserialize` traits.
///
/// # Returns
///
/// A pair containing the new state and the contract output as `HyleOutput`.
///
/// # Panics
///
/// Panics if the contract initialization or raw blob parsing fails.
pub fn execute<Contract, State, Action>(contract_input: &ContractInput) -> (State, HyleOutput)
where
    Action: BorshSerialize + BorshDeserialize,
    State: Digestable + BorshDeserialize + 'static,
    Contract: HyleContract<State, Action> + CallerCallee,
{
    let state: State = borsh::from_slice(&contract_input.state).expect("Failed to decode state");
    let initial_state_digest = state.as_digest();

    // Attempts to initialize the contract with the given input.
    let (action, execution_ctx) = match init::<Action>(contract_input) {
        Ok(res) => res,
        // If failing, falls back to parsing the raw blob from the contract input.
        Err(err) => match parse_blob::<Action>(&contract_input.blobs, &contract_input.index) {
            Some(action) => (action, ExecutionContext::default()),
            // Panics if both the initialization and the raw blob parsing fail.
            None => panic!("Hyllar contract initialization failed {}", err),
        },
    };

    let mut contract = Contract::init(state, execution_ctx);

    let mut res: RunResult<State> = contract.execute_action(action, contract_input);

    assert!(contract.callee_blobs().is_empty());

    let state = contract.state();

    let output = as_hyle_output::<State>(initial_state_digest, contract_input.clone(), &mut res);
    (state, output)
}
