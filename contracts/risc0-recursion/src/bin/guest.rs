#![no_main]
#![no_std]

extern crate alloc;

use sdk::guest::env;

risc0_zkvm::guest::entry!(main);

fn main() {
    let input: alloc::vec::Vec<risc0_recursion::ProofInput> = env::read();
    input.iter().for_each(|input| {
        risc0_zkvm::guest::env::verify(input.image_id, &input.journal)
            .expect("Verification failed");
    });
}
