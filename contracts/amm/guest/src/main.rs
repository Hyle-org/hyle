#![no_main]
#![no_std]

extern crate alloc;

use alloc::{format, vec::Vec};
use amm::{AmmAction, AmmContract, AmmState};
use sdk::{StructuredBlob, StructuredBlobData};

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob) = sdk::guest::init::<AmmState, StructuredBlob<AmmAction>>();

    let caller = match sdk::guest::check_caller_callees(&input, &parsed_blob) {
        Ok(caller) => caller,
        Err(err) => {
            return sdk::guest::fail(
                input,
                &format!("Incorrect Caller/Callees for this blob: {err}"),
            )
        }
    };

    let amm_state = input.initial_state.clone();
    let mut amm_contract = AmmContract::new(amm_state, caller);

    let amm_action = parsed_blob.data.parameters;

    let mut callees_blob = Vec::new();
    for blob in input.blobs.clone().into_iter() {
        let structured_blob: StructuredBlobData<Vec<u8>> =
            blob.data.clone().try_into().expect("Failed to decode blob");
        if structured_blob.caller == Some(input.index.clone()) {
            callees_blob.push(blob);
        }
    }

    let res = match amm_action {
        AmmAction::Swap { from, pair } => amm_contract.verify_swap(callees_blob, from, pair),
    };

    sdk::guest::commit(input, amm_contract.state(), res);
}
