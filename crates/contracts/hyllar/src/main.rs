#![no_main]
#![no_std]

extern crate alloc;

use hyllar::HyllarTokenContract;
use sdk::erc20::ERC20Action;
use sdk::guest::env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob, caller) = match sdk::guest::init_with_caller::<ERC20Action>() {
        Ok(res) => res,
        Err(err) => {
            panic!("Hyllar contract initialization failed {}", err);
        }
    };

    let state = input
        .initial_state
        .clone()
        .try_into()
        .expect("Failed to decode state");

    env::log("Init token contract");
    let mut contract = HyllarTokenContract::init(state, caller);

    env::log("execute action");
    let res = sdk::erc20::execute_action(&mut contract, parsed_blob.data.parameters);

    env::log(alloc::format!("commit {:?}", res).as_str());

    sdk::guest::commit(input, contract.state(), res);
}
