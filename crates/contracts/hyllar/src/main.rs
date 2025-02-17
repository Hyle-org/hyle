#![no_main]
#![no_std]

extern crate alloc;

use alloc::string::String;
use hyllar::execute;
use sdk::guest::{commit, GuestEnv, Risc0Env};
use sdk::ContractInput;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let mut logs = String::new();
    env.log(&logs);
    let contract_input: ContractInput = env.read();

    let res = execute(&mut logs, contract_input.clone());
    commit(env, contract_input, res);
}
