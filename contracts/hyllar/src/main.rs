#![no_main]
#![no_std]

extern crate alloc;

use hyllar::{HyllarToken, HyllarTokenContract};
use risc0_zkvm::guest::env;
use sdk::erc20::ERC20Action;

#[cfg(feature = "risc0")]
risc0_zkvm::guest::entry!(main);

#[cfg(all(feature = "sp1", not(feature = "risc0")))]
sp1_zkvm::entrypoint!(main);

fn main() {
    let (input, parsed_blob, caller) =
        match sdk::guest::init_with_caller::<HyllarToken, ERC20Action>() {
            Ok(res) => res,
            Err(err) => {
                panic!("Hyllar contract initialization failed {}", err);
            }
        };

    let state = input.initial_state.clone();

    env::log("Init token contract");
    let mut contract = HyllarTokenContract::init(state, caller);

    env::log("execute action");
    let res = sdk::erc20::execute_action(&mut contract, parsed_blob.data.parameters);

    env::log(alloc::format!("commit {:?}", res).as_str());

    sdk::guest::commit(input, contract.state(), res);
}
