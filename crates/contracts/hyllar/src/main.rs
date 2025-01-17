#![no_main]
#![no_std]

extern crate alloc;

use alloc::string::String;
use hyllar::execute;
use sdk::guest::GuestEnv;
use sdk::guest::Risc0Env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let mut logs = String::new();
    env.commit(&execute(&mut logs, env.read()));
    env.log(&logs);
}
