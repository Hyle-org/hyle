#![no_main]
#![no_std]

extern crate alloc;

use core::str::from_utf8;

use hydentity::Hydentity;
use sdk::identity_provider::IdentityAction;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob) = sdk::guest::init_raw::<IdentityAction>();

    let mut state: Hydentity = input
        .initial_state
        .clone()
        .try_into()
        .expect("Failed to decode state");

    let password = from_utf8(&input.private_blob.0).unwrap();

    let res = sdk::identity_provider::execute_action(&mut state, parsed_blob, password);

    sdk::guest::commit(input, state, res);
}
