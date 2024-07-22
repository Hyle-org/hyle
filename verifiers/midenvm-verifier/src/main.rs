// TODO: some of the helpers.rs is currently only used in tests, ignore the warnings for now.
#![allow(
    dead_code,
    unused_imports
)]

mod helpers;

use miden_verifier::{verify, ProgramInfo, Kernel};
use miden_vm::utils::Serializable;
use std::env;
use std::path::PathBuf;
use std::path::Path;
use crate::helpers::ProgramHash;
use crate::helpers::InputFile;
use crate::helpers::OutputFile;
use crate::helpers::ProofFile;
use hyle_contract::HyleOutput;


fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 5 {
        eprintln!("Usage: {} <program_hash> <proof_path> <stack_inputs> <stack_outputs>", args[0]);
        std::process::exit(1);
    }

    // Read program hash from the input.
    let program_hash = ProgramHash::read(&args[1]).unwrap();
    // Make required types for reading files.
    let mut input_path = PathBuf::new();
    input_path.push(args[3].clone()); 
    let mut output_path = PathBuf::new();
    output_path.push(args[4].clone());
    let proof_path = Path::new(&args[2]);

    // Load files.
    let input_data = InputFile::read(&Some(input_path), proof_path).unwrap();
    let outputs_data = OutputFile::read(&Some(output_path), proof_path).unwrap();

    // Fetch the stack inputs and outputs from the arguments
    let stack_inputs = input_data.parse_stack_inputs().unwrap();
    let stack_outputs = outputs_data.stack_outputs().unwrap();

    // Load the proof from file.
    let proof = ProofFile::read(&Some(proof_path.to_path_buf()), proof_path).unwrap();

    // This is copied from core midenvm verifier.
    // TODO accept kernel as CLI argument -- this is not done in core midenVM
    let kernel = Kernel::default();
    let program_info = ProgramInfo::new(program_hash, kernel);

    // verify proof
    let result = verify(program_info, stack_inputs, stack_outputs, proof)
        .map_err(|err| format!("Program failed verification! - {}", err));

    eprintln!("result: {:?}", result);
    // TODO: what to put for other fields?
    // let output: HyleOutput<()> = HyleOutput { initial_state: stack_inputs.to_bytes(), next_state: stack_outputs.to_bytes(), origin: (), caller: (), block_number: (), block_time: (), tx_hash: (), program_outputs: () };
}
