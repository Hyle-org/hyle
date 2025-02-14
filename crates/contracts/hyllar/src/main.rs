#![no_main]
#![no_std]

extern crate alloc;

use alloc::string::String;
use hyllar::execute;
use sdk::guest::{commit, GuestEnv, Risc0Env};
use sdk::{ContractInput, ProgramInput};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let mut logs = String::new();
    env.log(&logs);
    let program_inputs: ProgramInput = env.read();
    let hyllar_initial_state = borsh::from_slice(&program_inputs.serialized_initial_state)
        .expect("Failed to decode state");
    let res = execute(
        &mut logs,
        hyllar_initial_state.clone(),
        program_inputs.contract_input.clone(),
    );
    commit(env, hyllar_initial_state, input.clone(), res);
}
