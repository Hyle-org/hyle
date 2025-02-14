#![no_main]
#![no_std]

extern crate alloc;

use sdk::guest::{commit, GuestEnv, Risc0Env};
use sdk::{ContractInput, ProgramInput};
use uuid_tld::{execute, UuidTldState};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let program_inputs: ProgramInput = env.read();
    let uuid_tld_initial_state =
        UuidTldState::deserialize(&program_inputs.serialized_initial_state)
            .expect("Failed to decode state");
    let res = execute(
        uuid_tld_initial_state.clone(),
        program_inputs.contract_input.clone(),
    );
    commit(env, uuid_tld_initial_state, input.clone(), res);
}
