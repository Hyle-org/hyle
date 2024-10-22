#![no_main]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::ToString;
use hyfi::model::{ContractFunction, ContractInput};
use risc0_zkvm::guest::env;
use sdk::HyleOutput;

risc0_zkvm::guest::entry!(main);

fn main() {
    let mut input: ContractInput = env::read();

    let initial_balances = input.balances.clone();

    let payload = match input.blobs.get(input.index) {
        Some(v) => v,
        None => {
            env::log("Unable to find the payload");
            let flattened_blobs = input.blobs.into_iter().flatten().collect();
            env::commit(&HyleOutput {
                version: 1,
                initial_state: initial_balances.as_state(),
                next_state: initial_balances.as_state(),
                identity: sdk::Identity("".to_string()),
                tx_hash: sdk::TxHash(input.tx_hash),
                index: sdk::BlobIndex(input.index as u32),
                blobs: flattened_blobs,
                success: false,
                program_outputs: "Payload not found".to_string().into_bytes(),
            });
            return;
        }
    };

    let contract_function =
        ContractFunction::decode(payload).expect("Failed to decode contract function");
    let res = hyfi::run(&mut input.balances, contract_function);

    env::log(&format!("New balances: {:?}", input.balances));
    let next_balances = input.balances;

    let flattened_blobs = input.blobs.into_iter().flatten().collect();
    env::commit(&HyleOutput {
        version: 1,
        initial_state: initial_balances.as_state(),
        next_state: next_balances.as_state(),
        identity: sdk::Identity(res.identity),
        tx_hash: sdk::TxHash(input.tx_hash),
        index: sdk::BlobIndex(input.index as u32),
        blobs: flattened_blobs,
        success: res.success,
        program_outputs: res.program_outputs,
    })
}
