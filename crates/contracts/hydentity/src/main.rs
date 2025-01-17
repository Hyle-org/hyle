#![no_main]
#![no_std]

extern crate alloc;

use hydentity::execute;
use sdk::guest::GuestEnv;
use sdk::guest::Risc0Env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    env.commit(&execute(env.read()));
}
