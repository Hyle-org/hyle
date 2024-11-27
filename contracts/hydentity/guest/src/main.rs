#![no_main]
#![no_std]

extern crate alloc;

use core::str::from_utf8;

use hydentity::Hydentity;
use sdk::identity_provider::IdentityAction;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob) = sdk::guest::init::<Hydentity, IdentityAction>();

    let mut state = input.initial_state.clone();

    let password = from_utf8(&input.private_blob.0).unwrap();

    let res = sdk::identity_provider::execute_action(
        &mut state,
        input.identity.clone(),
        parsed_blob.data.parameters,
        password,
    );

    sdk::guest::commit(input, state, res);
}
