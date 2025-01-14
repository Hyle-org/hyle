#![no_main]
#![no_std]

extern crate alloc;

#[allow(unused_imports)]
use alloc::format;
use alloc::vec::Vec;
use sdk::caller::{CallerCallee, ExecutionContext};
use sdk::guest::Risc0Env;
use sdk::{info, StructuredBlobData};
use staking::execute;
use staking::{state::Staking, StakingAction, StakingContract};
risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let contract_input = env.read();
    env.commit(execute(contract_input));
}
