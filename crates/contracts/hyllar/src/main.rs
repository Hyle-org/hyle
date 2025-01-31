#![no_main]
#![no_std]

extern crate alloc;

use alloc::string::String;
use hyllar::execute;
use sdk::guest::{GuestEnv, Risc0Env, commit};
use sdk::ContractInput;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let mut logs = String::new();
    env.log(&logs);
    let input: ContractInput = env.read();
    commit(env, input.clone(), execute(&mut logs, input));
}
