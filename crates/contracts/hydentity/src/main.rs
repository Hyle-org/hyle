#![no_main]
#![no_std]

extern crate alloc;

use hydentity::{execute, Hydentity};
use sdk::guest::{commit, GuestEnv, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let (hydentity_initial_state, contract_input) = env.read::<Hydentity>();

    let res = execute(hydentity_initial_state.clone(), contract_input.clone());
    commit(env, hydentity_initial_state, contract_input, res);
}
