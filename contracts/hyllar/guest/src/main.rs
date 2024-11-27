#![no_main]
#![no_std]

extern crate alloc;

use alloc::format;
use hyllar::{HyllarToken, HyllarTokenContract};
use sdk::erc20::ERC20Action;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob) = sdk::guest::init::<HyllarToken, ERC20Action>();

    let caller = match sdk::guest::check_caller_callees(&input, &parsed_blob) {
        Ok(caller) => caller,
        Err(err) => {
            return sdk::guest::fail(
                input,
                &format!("Incorrect Caller/Callees for this blob: {err}"),
            )
        }
    };

    let state = input.initial_state.clone();

    let mut contract = HyllarTokenContract::init(state, caller);

    let res = sdk::erc20::execute_action(&mut contract, parsed_blob.data.parameters);

    sdk::guest::commit(input, contract.state(), res);
}
