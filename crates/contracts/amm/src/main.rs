#![no_main]
#![no_std]

extern crate alloc;

use amm::execute;
use sdk::guest::{commit, GuestEnv, Risc0Env};
use sdk::ContractInput;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let input: ContractInput = env.read();
    commit(env, input.clone(), execute(input));
}
