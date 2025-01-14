#![no_main]
#![no_std]

extern crate alloc;

use hyllar::HyllarTokenContract;
use sdk::erc20::ERC20Action;
use sdk::guest::{env, Risc0Env};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    env.commit(execute(env));
}
