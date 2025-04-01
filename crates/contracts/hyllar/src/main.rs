#![no_main]
#![no_std]

extern crate alloc;

use hyle_hyllar::Hyllar;
use sdk::guest::{execute, GuestEnv, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let zk_program_input = env.read();

    let output = execute::<Hyllar>(&zk_program_input);
    env.commit(&output);
}
