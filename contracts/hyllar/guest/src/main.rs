#![no_main]
#![no_std]

extern crate alloc;

use hyllar::{HyllarToken, HyllarTokenContract};
use sdk::erc20::ERC20Action;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob, caller) =
        match sdk::guest::init_with_caller::<HyllarToken, ERC20Action>() {
            Ok(res) => res,
            Err(err) => {
                panic!("Hyllar contract initialization failed {}", err);
            }
        };

    let state = input.initial_state.clone();

    let mut contract = HyllarTokenContract::init(state, caller);

    let res = sdk::erc20::execute_action(&mut contract, parsed_blob.data.parameters);

    sdk::guest::commit(input, contract.state(), res);
}
