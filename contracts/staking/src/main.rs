#![no_main]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::vec::Vec;
use sdk::caller::{CallerCallee, ExecutionContext};
use sdk::{info, StructuredBlobData};
use staking::state::OnChainState;
use staking::{state::Staking, StakingAction, StakingContract};
risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob, caller) =
        match sdk::guest::init_with_caller::<OnChainState, StakingAction>() {
            Ok(res) => res,
            Err(err) => {
                panic!("Staking contract initialization failed {}", err);
            }
        };

    // TODO: refactor this into ExecutionContext
    let mut callees_blobs = Vec::new();
    for blob in input.blobs.clone().into_iter() {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<Vec<u8>> = structured_blob; // for type inference
            if structured_blob.caller == Some(input.index.clone()) {
                callees_blobs.push(blob);
            }
        };
    }

    let (state, _): (Staking, _) =
        bincode::decode_from_slice(input.private_blob.0.as_slice(), bincode::config::standard())
            .expect("Failed to decode payload");

    info!("state: {:?}", state);
    info!("computed:: {:?}", state.on_chain_state());
    info!("given: {:?}", input.initial_state);
    if state.on_chain_state() != input.initial_state {
        panic!("State mismatch");
    }

    let ctx = ExecutionContext {
        callees_blobs: callees_blobs.into(),
        caller,
    };
    let mut contract = StakingContract::new(ctx, state);

    let action = parsed_blob.data.parameters;

    let res = contract.execute_action(action);

    assert!(contract.callee_blobs().is_empty());

    sdk::guest::commit(input, contract.on_chain_state(), res);
    info!("state: {:?}", contract.state());
}
