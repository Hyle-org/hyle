#![no_main]
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use sdk::guest::env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let mut input: Vec<risc0_recursion::ProofInput> = env::read();
    let mut outputs: Vec<Vec<u8>> = Vec::new();
    input.iter_mut().for_each(|input| {
        risc0_zkvm::guest::env::verify(input.image_id, &input.journal)
            .expect("Verification failed");
        outputs.push(core::mem::take(&mut input.journal));
    });
    risc0_zkvm::guest::env::commit(&outputs);
}
