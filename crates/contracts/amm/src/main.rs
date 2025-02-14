#![no_main]
#![no_std]

extern crate alloc;

use amm::{execute, AmmState};
use borsh::BorshDeserialize;
use sdk::guest::{commit, GuestEnv, Risc0Env};
use sdk::{ContractInput, ProgramInput};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let program_inputs: ProgramInput = env.read();
    let amm_initial_state = borsh::from_slice(&program_inputs.serialized_initial_state)
        .expect("Failed to decode state");
    let res = execute(
        amm_initial_state.clone(),
        program_inputs.contract_input.clone(),
    );
    commit(env, amm_initial_state, input.clone(), res);
}
