#![no_main]
#![no_std]

extern crate alloc;

use alloc::string::String;
use hyllar::execute;
use sdk::guest::{commit, GuestEnv, Risc0Env};
use sdk::ProgramInput;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let mut logs = String::new();
    env.log(&logs);
    let program_input: ProgramInput = env.read();

    let res = execute(&mut logs, program_input.clone());
    commit(env, program_input, res);
}
