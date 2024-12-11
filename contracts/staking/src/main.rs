#![no_main]
#![no_std]

extern crate alloc;

risc0_zkvm::guest::entry!(main);

fn main() {}
