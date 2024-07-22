use std::fs;

use crate::utils::error::VerifierError;
use clap::Parser;

mod commands;
mod utils;

#[derive(Parser)]
struct Cli {
    proof_path: String,
}

fn main() -> Result<(), VerifierError> {
    let args: commands::ProverArgs = commands::ProverArgs::parse();

    let res = match args.entity {
        commands::ProverEntity::Verify(args) => {
            let Ok(program_content) = std::fs::read(&args.proof_path) else {
                return Err(VerifierError(format!(
                    "Error opening {} file",
                    &args.proof_path
                )));
            };
            utils::verify_proof(&program_content)
        }
        commands::ProverEntity::Prove(args) => {
            let program_output_str: String =
                fs::read_to_string(&args.output_path).expect("Failed to read output file");

            let trace_data = fs::read(&args.trace_bin_path).expect("failed to load trace file");
            let memory_data = fs::read(&args.memory_bin_path).expect("failed to load memory file");
            let proof = utils::prove(&trace_data, &memory_data, &program_output_str)?;
            std::fs::write(&args.proof_path, proof)?;
            Ok(format!("Proof written to {}", &args.proof_path))
        }
    };
    match res {
        Result::Ok(output) => println!("{}", output),
        Result::Err(err) => {
            return Err(err);
        }
    };
    Ok(())
}
