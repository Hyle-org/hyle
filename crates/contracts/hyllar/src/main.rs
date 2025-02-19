#![no_main]
#![no_std]

extern crate alloc;

use alloc::string::String;
use hyllar::{execute, HyllarToken};
use sdk::guest::{commit, GuestEnv, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let mut logs = String::new();
    env.log(&logs);
    let (hyllar_initial_state, contract_input) = env.read::<HyllarToken>();

    let res = execute(
        &mut logs,
        hyllar_initial_state.clone(),
        contract_input.clone(),
    );
    commit(env, hyllar_initial_state, contract_input, res);
}
