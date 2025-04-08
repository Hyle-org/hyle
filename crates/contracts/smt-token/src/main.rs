#![no_main]
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use hyle_smt_token::SmtTokenContract;
use sdk::{
    guest::{execute, GuestEnv, Risc0Env},
    Calldata,
};

risc0_zkvm::guest::entry!(main);

fn main() {
    let env = Risc0Env {};
    let (commitment_metadata, calldata): (Vec<u8>, Calldata) = env.read();

    let output = execute::<SmtTokenContract>(&commitment_metadata, &calldata);
    env.commit(&output);
}
