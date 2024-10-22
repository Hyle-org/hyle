#![no_main]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::ToString;
use hydentity::model::{ContractFunction, ContractInput};
use risc0_zkvm::guest::env;
use sdk::Digestable;
use sdk::HyleOutput;

risc0_zkvm::guest::entry!(main);

fn main() {
    let mut input: ContractInput = env::read();

    let initial_balances = input.identities.clone();

    let payload = match input.blobs.get(input.index) {
        Some(v) => v,
        None => {
            env::log("Unable to find the payload");
            let flattened_blobs = input.blobs.into_iter().flat_map(|b| b.0).collect();
            env::commit(&HyleOutput {
                version: 1,
                initial_state: initial_balances.as_digest(),
                next_state: initial_balances.as_digest(),
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
    let res = hydentity::run(&mut input.identities, contract_function);

    env::log(&format!("New identities: {:?}", input.identities));
    let next_balances = input.identities;

    let flattened_blobs = input.blobs.into_iter().flat_map(|b| b.0).collect();
    env::commit(&HyleOutput {
        version: 1,
        initial_state: initial_balances.as_digest(),
        next_state: next_balances.as_digest(),
        identity: sdk::Identity(res.identity),
        tx_hash: sdk::TxHash(input.tx_hash),
        index: sdk::BlobIndex(input.index as u32),
        blobs: flattened_blobs,
        success: res.success,
        program_outputs: res.program_outputs,
    })
}
