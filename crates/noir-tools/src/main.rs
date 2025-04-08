use std::io::Read;

use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// Extracts the public inputs from the output of `reconstruct_honk_proof`.
fn extract_public_inputs(proof_with_public_inputs: &[u8]) -> &[u8] {
    // The first 4 bytes represent the proof size as a big-endian 32-bit unsigned integer.
    let proof_len = u32::from_be_bytes(proof_with_public_inputs[0..4].try_into().unwrap()) as usize;
    // The public inputs are located between the proof size and the proof itself.
    let public_inputs_end = proof_with_public_inputs.len() - proof_len;
    &proof_with_public_inputs[4..public_inputs_end]
}

/// Reverses the flattening process by splitting a `Vec<u8>` into a vector of sanitized hex-encoded strings
/// based on the provided lengths of the original fields.
fn deflatten_fields(flattened_fields: &[u8]) -> Vec<String> {
    const PUBLIC_INPUT_SIZE: usize = 32; // Each field is 32 bytes
    let mut result = Vec::new();

    for chunk in flattened_fields.chunks(PUBLIC_INPUT_SIZE) {
        let hex_string = hex::encode(chunk);
        let sanitised_hex = format!("{:0>64}", hex_string); // Pad to 64 characters
        result.push(sanitised_hex);
    }

    result
}

fn main() -> std::io::Result<()> {
    // setup tracing
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()
        .unwrap();
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let Some(file) = args.get(1) else {
        eprintln!("No arguments provided.");
        return Ok(());
    };

    let mut file = std::fs::File::open(file)?;
    let mut output = Vec::new();
    file.read_to_end(&mut output)?;

    println!("Output length: {}", output.len());

    let mut public_outputs = deflatten_fields(extract_public_inputs(&output));

    let ho = hyle_verifiers::noir_utils::parse_noir_output(&mut public_outputs).map_err(|e| {
        eprintln!("Error parsing output: {}", e);
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    })?;

    println!("Parsed output: {:?}", ho);

    Ok(())
}
