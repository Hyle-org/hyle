#![no_main]
#![no_std]

extern crate alloc;

use core::str::from_utf8;

use hydentity::{execute, Hydentity};
use sdk::{guest::Risc0Env, identity_provider::IdentityAction};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let input = env.read();
    env.commit(execute(input));
}
