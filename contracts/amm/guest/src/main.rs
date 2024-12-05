#![no_main]
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use amm::{AmmAction, AmmContract, AmmState};
use sdk::caller::{CallerCallee, ExecutionState};
use sdk::StructuredBlobData;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob, caller) = match sdk::guest::init_with_caller::<AmmState, AmmAction>() {
        Ok(res) => res,
        Err(err) => {
            panic!("Amm contract initialization failed {}", err);
        }
    };

    // TODO: refactor this into ExecutionState
    let mut callees_blobs = Vec::new();
    for blob in input.blobs.clone().into_iter() {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<Vec<u8>> = structured_blob; // for compiler
            if structured_blob.caller == Some(input.index.clone()) {
                callees_blobs.push(blob);
            }
        };
    }

    let execution_state = ExecutionState {
        callees_blobs: callees_blobs.into(),
        caller,
    };
    let amm_state = input.initial_state.clone();
    let mut amm_contract = AmmContract::new(execution_state, parsed_blob.contract_name, amm_state);

    let amm_action = parsed_blob.data.parameters;

    let res = match amm_action {
        AmmAction::Swap { pair } => amm_contract.verify_swap(pair),
        AmmAction::NewPair { pair, amounts } => amm_contract.create_new_pair(pair, amounts),
    };

    assert!(amm_contract.callees_blobs().is_empty());

    sdk::guest::commit(input, amm_contract.state(), res);
}
