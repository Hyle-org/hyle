#![no_main]
#![no_std]

extern crate alloc;

use staking::execute;
use sdk::guest::{GuestEnv, Risc0Env, commit};
use sdk::ContractInput;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let input: ContractInput = env.read();
    commit(env, input.clone(), execute(input));
}
