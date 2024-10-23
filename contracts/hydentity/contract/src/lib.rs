#![no_std]
extern crate alloc;

use alloc::{format, string::ToString};
use model::{ContractFunction, Identities};
use sdk::guest::RunResult;

pub mod model;

pub fn run(state: &mut Identities, parameters: ContractFunction) -> RunResult {
    match parameters {
        ContractFunction::Register { account, password } => {
            let success = match state.register(account.clone(), password) {
                Ok(()) => true,
                Err(_e) => {
                    //env::log(&format!("Failed to Mint: {:?}", e));
                    false
                }
            };
            let program_outputs = format!("Registered {} ", account).to_string().into_bytes();

            RunResult {
                success,
                identity: account,
                program_outputs,
            }
        }
        ContractFunction::CheckPassword { account, password } => {
            let success = match state.check_password(account.clone(), password) {
                Ok(()) => true,
                Err(_e) => {
                    //env::log(&format!("Failed to Mint: {:?}", e));
                    false
                }
            };
            let program_outputs = format!("Password checked {}: {} ", account, success)
                .to_string()
                .into_bytes();

            RunResult {
                success,
                identity: account,
                program_outputs,
            }
        }
    }
}
