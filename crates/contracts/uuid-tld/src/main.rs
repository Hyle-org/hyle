#![no_main]
#![no_std]

extern crate alloc;

use sdk::guest::{execute, GuestEnv, Risc0Env};
use uuid_tld::UuidTld;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let contract_input = env.read();

    let (_, output) = execute::<UuidTld>(&contract_input);
    env.commit(&output);
}
