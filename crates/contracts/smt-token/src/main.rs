#![no_main]
#![no_std]

extern crate alloc;

use hyle_smt_token::SmtTokenContract;
use sdk::guest::{execute, GuestEnv, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let contract_input = env.read();

    let output = execute::<SmtTokenContract>(&contract_input);
    env.commit(&output);
}
