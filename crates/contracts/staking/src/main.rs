#![no_main]
#![no_std]

extern crate alloc;

use sdk::guest::{execute, GuestEnv, Risc0Env};
use sdk::StakingAction;
use staking::state::StakingState;
use staking::StakingContract;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let contract_input = env.read();

    let (_, output) = execute::<StakingContract, StakingState, StakingAction>(&contract_input);
    env.commit(&output);
}
