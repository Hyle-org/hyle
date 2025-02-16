#![no_main]
#![no_std]

extern crate alloc;

use sdk::guest::{commit, GuestEnv, Risc0Env};
use sdk::ProgramInput;
use uuid_tld::execute;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let program_input: ProgramInput = env.read();

    let res = execute(program_input.clone());
    commit(env, program_input, res);
}
