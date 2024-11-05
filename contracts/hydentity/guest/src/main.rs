#![no_main]
#![no_std]

extern crate alloc;

use hydentity::model::ContractFunction;
use hydentity::model::Identities;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parameters) = sdk::guest::init::<Identities, ContractFunction>();

    let mut state = input.initial_state.clone();

    let res = hydentity::run(&mut state, parameters);

    sdk::guest::commit(input, state, res);
}
