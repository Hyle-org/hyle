use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use bincode::Decode;
use risc0_zkvm::guest::env;
use serde::de::DeserializeOwned;

use crate::{ContractInput, Digestable, HyleOutput};

pub struct RunResult {
    pub success: bool,
    pub identity: String,
    pub program_outputs: Vec<u8>,
}

pub fn fail<State>(input: ContractInput<State>, message: &str)
where
    State: Digestable,
{
    env::log(message);

    let flattened_blobs = input.blobs.into_iter().flat_map(|b| b.0).collect();
    env::commit(&HyleOutput {
        version: 1,
        initial_state: input.initial_state.as_digest(),
        next_state: input.initial_state.as_digest(),
        identity: crate::Identity("".to_string()),
        tx_hash: crate::TxHash(input.tx_hash),
        index: crate::BlobIndex(input.index as u32),
        blobs: flattened_blobs,
        success: false,
        program_outputs: message.to_string().into_bytes(),
    });
}

pub fn init<State, Parameters>() -> (ContractInput<State>, Parameters)
where
    State: Digestable + DeserializeOwned,
    Parameters: Decode,
{
    let input: ContractInput<State> = env::read();

    let payload = match input.blobs.get(input.index) {
        Some(v) => v,
        None => {
            fail(input, "Unable to find the payload");
            panic!("unable to find the payload");
        }
    };

    let (parameters, _) =
        bincode::decode_from_slice(payload.0.as_slice(), bincode::config::standard())
            .expect("Failed to decode payload");

    (input, parameters)
}

pub fn commit<State>(input: ContractInput<State>, new_state: State, res: RunResult)
where
    State: Digestable,
{
    let flattened_blobs = input.blobs.into_iter().flat_map(|b| b.0).collect();
    env::commit(&HyleOutput {
        version: 1,
        initial_state: input.initial_state.as_digest(),
        next_state: new_state.as_digest(),
        identity: crate::Identity(res.identity),
        tx_hash: crate::TxHash(input.tx_hash),
        index: crate::BlobIndex(input.index as u32),
        blobs: flattened_blobs,
        success: res.success,
        program_outputs: res.program_outputs,
    });
}
