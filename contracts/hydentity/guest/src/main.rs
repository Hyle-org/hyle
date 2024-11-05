#![no_main]
#![no_std]

extern crate alloc;

use hydentity::Hydentity;
use sdk::identity_provider::IdentityAction;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parameters) = sdk::guest::init::<Hydentity, IdentityAction>();

    let mut state = input.initial_state.clone();

    let res =
        sdk::identity_provider::execute_action(&mut state, input.identity.clone(), parameters);

    sdk::guest::commit(input, state, res);
}
