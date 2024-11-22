#![no_main]
#![no_std]

extern crate alloc;

use hyllar::{HyllarToken, HyllarTokenContract};
use sdk::erc20::ERC20Action;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parameters) = sdk::guest::init::<HyllarToken, ERC20Action>();

    let state = input.initial_state.clone();

    let mut contract = HyllarTokenContract::init(state, input.identity.clone());

    let res = sdk::erc20::execute_action(&mut contract, parameters);

    sdk::guest::commit(input, contract.state(), res);
}
