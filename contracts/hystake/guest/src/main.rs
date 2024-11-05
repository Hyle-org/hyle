#![no_main]
#![no_std]

extern crate alloc;

use alloc::format;
use hystake::model::{Balances, ContractFunction};
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parameters) = sdk::guest::init::<Balances, ContractFunction>();

    let mut state = input.initial_state.clone();

    let res = hyfi::run(&mut state, parameters);

    env::log(&format!("New balances: {:?}", state));

    sdk::guest::commit(input, state, res);
}
