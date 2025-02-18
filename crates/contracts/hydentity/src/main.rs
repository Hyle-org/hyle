#![no_main]
#![no_std]

extern crate alloc;

use hydentity::{HydentityContract, HydentityState};
use sdk::guest::{execute, GuestEnv, Risc0Env};
use sdk::identity_provider::IdentityAction;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let contract_input = env.read();

    let (_, output) = execute::<HydentityContract, HydentityState, IdentityAction>(&contract_input);
    env.commit(&output);
}
