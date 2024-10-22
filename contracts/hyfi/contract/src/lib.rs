#![no_std]
extern crate alloc;

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use model::{Balances, ContractFunction};

pub mod model;

pub struct RunResult {
    pub success: bool,
    pub identity: String,
    pub program_outputs: Vec<u8>,
}

pub fn run(state: &mut Balances, parameters: ContractFunction) -> RunResult {
    match parameters {
        ContractFunction::Transfer { from, to, amount } => {
            let success = match state.send(&from, &to, amount) {
                Ok(()) => true,
                Err(_e) => {
                    //env::log(&format!("Failed to Transfer: {:?}", e));
                    false
                }
            };
            let program_outputs = format!("Transferred {} from {} to {}", amount, from, to)
                .to_string()
                .into_bytes();

            RunResult {
                success,
                identity: from,
                program_outputs,
            }
        }
        ContractFunction::Mint { to, amount } => {
            let success = match state.mint(&to, amount) {
                Ok(()) => true,
                Err(_e) => {
                    //env::log(&format!("Failed to Mint: {:?}", e));
                    false
                }
            };
            let program_outputs = format!("Minted {} to {}", amount, to)
                .to_string()
                .into_bytes();

            RunResult {
                success,
                identity: to,
                program_outputs,
            }
        }
        ContractFunction::PayFees { from, amount } => {
            let success = match state.pay_fees(&from, amount) {
                Ok(()) => true,
                Err(_e) => {
                    //env::log(&format!("Failed to PayFees: {:?}", e));
                    false
                }
            };
            let program_outputs = format!("{} payed {} for fees", from, amount)
                .to_string()
                .into_bytes();

            RunResult {
                success,
                identity: from,
                program_outputs,
            }
        }
    }
}
