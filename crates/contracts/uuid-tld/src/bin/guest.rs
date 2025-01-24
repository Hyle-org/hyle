#![no_main]
#![no_std]

extern crate alloc;

use sdk::guest::GuestEnv;
use sdk::guest::Risc0Env;
use uuid_tld::execute;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    env.commit(&execute(env.read()));
}
