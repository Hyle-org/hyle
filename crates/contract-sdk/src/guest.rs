use alloc::string::{String, ToString};
use bincode::{Decode, Encode};
use serde::de::DeserializeOwned;

use crate::{
    flatten_blobs,
    utils::{as_hyle_output, check_caller_callees, parse_blob, parse_structured_blob},
    ContractInput, Digestable, HyleOutput, Identity, StructuredBlob,
};

pub trait GuestEnv {
    fn log(&self, message: &str);
    fn commit(&self, output: &HyleOutput);
    fn read<T: DeserializeOwned + 'static>(&self) -> T;
}

pub struct Risc0Env;

#[cfg(feature = "risc0")]
impl GuestEnv for Risc0Env {
    fn log(&self, message: &str) {
        risc0_zkvm::guest::env::log(message);
    }

    fn commit(&self, output: &HyleOutput) {
        risc0_zkvm::guest::env::commit(output);
    }

    fn read<T: DeserializeOwned>(&self) -> T {
        risc0_zkvm::guest::env::read()
    }
}

pub struct SP1Env;

// For coverage tests, assume risc0 if both are active
#[cfg(all(feature = "sp1", not(feature = "risc0")))]
impl GuestEnv for SP1Env {
    fn log(&self, message: &str) {
        // TODO: this does nothing actually
        sp1_zkvm::io::hint(&message);
    }

    fn commit(&self, output: &HyleOutput) {
        sp1_zkvm::io::commit(output);
    }

    fn read<T: DeserializeOwned>(&self) -> T {
        sp1_zkvm::io::read()
    }
}

pub fn fail(env: impl GuestEnv, input: ContractInput, message: &str) {
    env.log(message);

    env.commit(&HyleOutput {
        version: 1,
        initial_state: input.initial_state.clone(),
        next_state: input.initial_state,
        identity: input.identity,
        tx_hash: input.tx_hash,
        index: input.index,
        blobs: flatten_blobs(&input.blobs),
        success: false,
        program_outputs: message.to_string().into_bytes(),
    });
}

pub fn panic(env: impl GuestEnv, message: &str) {
    env.log(message);
    // should we env::commit ?
    panic!("{}", message);
}

pub fn init_raw<Parameters>(input: ContractInput) -> (ContractInput, Parameters)
where
    Parameters: Decode,
{
    let parsed_blob = parse_blob::<Parameters>(&input.blobs, &input.index);

    (input, parsed_blob)
}

pub fn init_with_caller<Parameters>(
    input: ContractInput,
) -> Result<(ContractInput, StructuredBlob<Parameters>, Identity), String>
where
    Parameters: Encode + Decode,
{
    let parsed_blob = parse_structured_blob::<Parameters>(&input.blobs, &input.index);

    let caller = check_caller_callees::<Parameters>(&input, &parsed_blob)?;

    Ok((input, parsed_blob, caller))
}

pub fn commit<State>(
    env: impl GuestEnv,
    input: ContractInput,
    new_state: State,
    res: crate::RunResult,
) where
    State: Digestable,
{
    env.commit(&as_hyle_output(input, new_state, res));
}
