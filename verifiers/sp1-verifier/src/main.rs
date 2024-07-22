use std::env;

use base64::prelude::*;

use sp1_sdk::{ProverClient, SP1Proof, SP1VerifyingKey};

use hyle_contract::HyleOutput;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <b64_encoded_verification_key> <receipt_path>", args[0]);
        std::process::exit(1);
    }

    let vk_json = String::from_utf8(BASE64_STANDARD.decode(&args[1]).expect("vk decoding failed")).expect("Fail to cast vk to json string");

    let vk: SP1VerifyingKey = SP1VerifyingKey{
        vk: serde_json::from_str(&vk_json).unwrap()
    };
    let mut proof = SP1Proof::load(&args[2]).expect("loading proof failed");

    // TODO: check that vk is correct ?
    let prover_client = ProverClient::new();
    prover_client.verify(&proof, &vk).expect("verification failed");

    // Outputs to stdout for the caller to read.
    let output: HyleOutput<()> = proof.public_values.read::<HyleOutput<()>>();
    println!("{}", serde_json::to_string(&output).expect("Failed to serialize output"));

}
