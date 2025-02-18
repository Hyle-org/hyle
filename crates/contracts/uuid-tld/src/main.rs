#![no_main]
#![no_std]

extern crate alloc;

use sdk::guest::{execute, GuestEnv, Risc0Env};
use sdk::ContractInput;
use uuid_tld::{UuidTldAction, UuidTldContract, UuidTldState};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let contract_input = env.read();

    let (_, output) = execute::<UuidTldContract, UuidTldState, UuidTldAction>(&contract_input);
    env.commit(&output);
}
