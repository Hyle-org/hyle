#![no_main]
#![no_std]

extern crate alloc;

use core::str::from_utf8;

use hydentity::Hydentity;
use sdk::identity_provider::IdentityAction;

#[cfg(feature = "risc0")]
risc0_zkvm::guest::entry!(main);

#[cfg(all(feature = "sp1", not(feature = "risc0")))]
sp1_zkvm::entrypoint!(main);

fn main() {
    let (input, parsed_blob) = sdk::guest::init_raw::<Hydentity, IdentityAction>();

    let mut state = input.initial_state.clone();

    let password = from_utf8(&input.private_blob.0).unwrap();

    let res = sdk::identity_provider::execute_action(&mut state, parsed_blob, password);

    sdk::guest::commit(input, state, res);
}
