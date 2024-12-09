#![no_main]
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use amm::{AmmAction, AmmContract, AmmState};
use sdk::caller::{CallerCallee, ExecutionContext};
use sdk::erc20::ERC20Action;
use sdk::StructuredBlobData;

risc0_zkvm::guest::entry!(main);

fn main() {
    let (input, parsed_blob, caller) = match sdk::guest::init_with_caller::<AmmState, AmmAction>() {
        Ok(res) => res,
        Err(err) => {
            panic!("Amm contract initialization failed {}", err);
        }
    };

    // For convenience, we need to figure out the from/to amount
    let mut from_amount = 0;
    let mut to_amount = 0;

    // TODO: refactor this into ExecutionContext
    let mut callees_blobs = Vec::new();
    for blob in input.blobs.clone().into_iter() {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<Vec<u8>> = structured_blob; // for type inference
            if structured_blob.caller == Some(input.index.clone()) {
                match bincode::decode_from_slice(
                    &structured_blob.parameters,
                    bincode::config::standard(),
                ) {
                    Ok((
                        ERC20Action::TransferFrom {
                            sender,
                            recipient: _,
                            amount,
                        },
                        _,
                    )) => {
                        if sender == caller.0 {
                            from_amount = amount;
                        } else {
                            panic!("ERC20Action::TransferFrom sender is not the caller");
                        }
                    }
                    Ok((ERC20Action::Transfer { recipient, amount }, _)) => {
                        if recipient == caller.0 {
                            to_amount = amount;
                        } else {
                            panic!("ERC20Action::Transfer recipient is not the caller");
                        }
                    }
                    _ => {
                        // We should only be calling ERC20s, so we can fail already
                        panic!("Invalid ERC20 action");
                    }
                }
                callees_blobs.push(blob);
            }
        };
    }

    let execution_state = ExecutionContext {
        callees_blobs: callees_blobs.into(),
        caller,
    };
    let amm_state = input.initial_state.clone();
    let mut amm_contract = AmmContract::new(execution_state, parsed_blob.contract_name, amm_state);

    let amm_action = parsed_blob.data.parameters;

    let res = match amm_action {
        AmmAction::Swap { pair } => amm_contract.verify_swap(pair, from_amount, to_amount),
        AmmAction::NewPair { pair, amounts } => amm_contract.create_new_pair(pair, amounts),
    };

    assert!(amm_contract.callee_blobs().is_empty());

    sdk::guest::commit(input, amm_contract.state(), res);
}
