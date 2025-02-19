#![no_main]
#![no_std]

extern crate alloc;

use amm::{execute, AmmState};
use sdk::guest::{commit, GuestEnv, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let (initial_state, contract_input) = env.read::<AmmState>();

    let res = execute(initial_state.clone(), contract_input.clone());
    commit(env, initial_state, contract_input, res);
}
