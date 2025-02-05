use alloc::string::{String, ToString};
use alloc::vec;
use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    flatten_blobs,
    utils::{as_hyle_output, check_caller_callees, parse_blob, parse_structured_blob},
    ContractInput, Digestable, HyleOutput, Identity, StructuredBlob,
};

pub trait GuestEnv {
    fn log(&self, message: &str);
    fn commit(&self, output: &HyleOutput);
    fn read<T: BorshDeserialize + 'static>(&self) -> T;
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

    fn read<T: BorshDeserialize>(&self) -> T {
        let len: usize = risc0_zkvm::guest::env::read();
        let mut slice = vec![0u8; len];
        risc0_zkvm::guest::env::read_slice(&mut slice);
        borsh::from_slice(&slice).unwrap()
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
        let vec = borsh::to_vec(&output).unwrap();
        sp1_zkvm::io::commit_slice(&vec);
    }

    fn read<T: BorshDeserialize>(&self) -> T {
        let vec = sp1_zkvm::io::read_vec();
        borsh::from_slice(&vec).unwrap()
    }
}

pub fn fail(input: ContractInput, message: &str) -> HyleOutput {
    HyleOutput {
        version: 1,
        initial_state: input.initial_state.clone(),
        next_state: input.initial_state,
        identity: input.identity,
        index: input.index,
        blobs: flatten_blobs(&input.blobs),
        success: false,
        tx_hash: input.tx_hash,
        tx_ctx: input.tx_ctx,
        registered_contracts: vec![],
        program_outputs: message.to_string().into_bytes(),
    }
}

pub fn panic(env: impl GuestEnv, message: &str) {
    env.log(message);
    // should we env::commit ?
    panic!("{}", message);
}

pub fn init_raw<Parameters>(input: ContractInput) -> (ContractInput, Option<Parameters>)
where
    Parameters: BorshDeserialize,
{
    let parsed_blob = parse_blob::<Parameters>(&input.blobs, &input.index);

    (input, parsed_blob)
}

pub fn init_with_caller<Parameters>(
    input: ContractInput,
) -> Result<(ContractInput, StructuredBlob<Parameters>, Identity), String>
where
    Parameters: BorshSerialize + BorshDeserialize,
{
    let parsed_blob = parse_structured_blob::<Parameters>(&input.blobs, &input.index);

    let parsed_blob = parsed_blob.ok_or("Failed to parse input blob".to_string())?;

    let caller = check_caller_callees::<Parameters>(&input, &parsed_blob)?;

    Ok((input, parsed_blob, caller))
}

pub fn commit<State>(env: impl GuestEnv, input: ContractInput, mut res: crate::RunResult<State>)
where
    State: Digestable,
{
    env.commit(&as_hyle_output(input, &mut res));
}
