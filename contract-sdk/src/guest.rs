use alloc::{string::ToString, vec::Vec};
use bincode::Decode;
use risc0_zkvm::guest::env;
use serde::de::DeserializeOwned;

use crate::{flatten_blobs, Blob, ContractInput, Digestable, HyleOutput, Identity};

pub struct RunResult {
    pub success: bool,
    pub identity: Identity,
    pub program_outputs: Vec<u8>,
}

pub fn fail<State>(input: ContractInput<State>, message: &str)
where
    State: Digestable,
{
    env::log(message);

    env::commit(&HyleOutput {
        version: 1,
        initial_state: input.initial_state.as_digest(),
        next_state: input.initial_state.as_digest(),
        identity: crate::Identity("".to_string()),
        tx_hash: crate::TxHash(input.tx_hash),
        index: crate::BlobIndex(input.index as u32),
        blobs: flatten_blobs(&input.blobs),
        success: false,
        program_outputs: message.to_string().into_bytes(),
    });
}

pub fn panic(message: &str) {
    env::log(message);
    // should we env::commit ?
    panic!("{}", message);
}

pub fn init<State, Parameters>() -> (ContractInput<State>, Parameters)
where
    State: Digestable + DeserializeOwned,
    Parameters: Decode,
{
    let input: ContractInput<State> = env::read();

    let parameters = parse_blob::<Parameters>(&input.blobs, input.index);

    (input, parameters)
}

pub fn parse_blob<Parameters>(blobs: &[Blob], index: usize) -> Parameters
where
    Parameters: Decode,
{
    let blob = match blobs.get(index) {
        Some(v) => v,
        None => {
            //fail(input, "Unable to find the payload");
            panic!("unable to find the payload");
        }
    };
    let (parameters, _) =
        bincode::decode_from_slice(blob.data.0.as_slice(), bincode::config::standard())
            .expect("Failed to decode payload");
    parameters
}

pub fn commit<State>(input: ContractInput<State>, new_state: State, res: RunResult)
where
    State: Digestable,
{
    env::commit(&HyleOutput {
        version: 1,
        initial_state: input.initial_state.as_digest(),
        next_state: new_state.as_digest(),
        identity: res.identity,
        tx_hash: crate::TxHash(input.tx_hash),
        index: crate::BlobIndex(input.index as u32),
        blobs: flatten_blobs(&input.blobs),
        success: res.success,
        program_outputs: res.program_outputs,
    });
}
