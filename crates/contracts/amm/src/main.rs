#![no_main]
#![no_std]

extern crate alloc;

use hyle_amm::Amm;
use sdk::guest::{execute, GuestEnv, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let zk_program_input = env.read();

    let output = execute::<Amm>(&zk_program_input);
    env.commit(&output);
}
