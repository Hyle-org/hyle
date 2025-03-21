#![no_main]
#![no_std]

extern crate alloc;

use hyle_staking::state::Staking;
use sdk::guest::{execute, GuestEnv, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let program_input = env.read();

    let (_, output) = execute::<Staking>(&program_input);
    env.commit(&output);
}
