use serde_json;
use std::env;

use hyle_contract::HyleOutput;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <image_id> <receipt_path>", args[0]);
        std::process::exit(1);
    }
    
    // Image ID is the hexademical representation of the method ID, without leading prefix.
    let image_id = &args[1];

    // Parse the proof from file
    let receipt_path = &args[2];
    let receipt_content = std::fs::read_to_string(receipt_path).expect("Failed to read receipt file");
    let receipt: risc0_zkvm::Receipt = serde_json::from_str(&receipt_content).expect("Failed to parse receipt file");

    let mut decoded_image_id: [u8; 32] = [0; 32];
    for i in 0..image_id.len() / 2 {
        decoded_image_id[i] = u8::from_str_radix(
            &image_id
                .get(i*2..i*2 + 2)
                .expect("Invalid method ID string"),
            16,
        )
        .expect("Invalid method ID string")
    }
    // Rotate to pad 0s in front.
    decoded_image_id.rotate_right((64 - image_id.len()) / 2);

    // perform verification
    receipt
        .verify(decoded_image_id)
        .expect("Verification failed");

    // Outputs to stdout for the caller to read.
    let output: HyleOutput<()> = receipt.journal.decode().expect("Failed to decode receipt journal");
    println!("{}", serde_json::to_string(&output).expect("Failed to serialize output"));
}
