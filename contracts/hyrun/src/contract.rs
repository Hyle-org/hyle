use anyhow::{bail, Error};
use borsh::to_vec;
use risc0_zkvm::sha::Digestible;
use sdk::{Blob, ContractInput, Digestable, HyleOutput, Identity};

use crate::{Cli, Contract};

pub fn init<State>(contract_name: &str, initial_state: State)
where
    State: Digestable + std::fmt::Debug,
{
    println!("Initial state: {:?}", initial_state);
    let initial_state = hex::encode(initial_state.as_digest().0);
    let program_name = get_image(contract_name);
    let file_path = format!("contracts/{}/{}.txt", program_name, program_name);
    let image_id = std::fs::read_to_string(file_path)
        .expect("Unable to read image id. Did you build contracts ?")
        .trim_end()
        .to_string();

    println!("You can register the contract by running:");
    println!(
        "\x1b[93mhyled contract default risc0 {} {} {} \x1b[0m",
        image_id, contract_name, initial_state
    );
}

pub fn print_hyled_blob_tx(identity: &Identity, blobs: &Vec<Blob>) {
    println!("You can send the blob tx:");
    print!("\x1b[93mhyled blobs {} ", identity.0);
    for blob in blobs {
        let hex_blob_data = hex::encode(&blob.data.0);
        print!("{} {} ", blob.contract_name.0, hex_blob_data);
    }
    println!("\x1b[0m");
    println!("{}", "-".repeat(20));
}

pub fn run<State, Builder>(cli: &Cli, contract_name: &str, build_contract_input: Builder)
where
    State: TryFrom<sdk::StateDigest, Error = Error>,
    State: Digestable + std::fmt::Debug + serde::Serialize,
    Builder: Fn(State) -> ContractInput<State>,
{
    let initial_state = match fetch_current_state(cli, contract_name) {
        Ok(s) => s,
        Err(e) => {
            println!("fetch current state error: {}", e);
            return;
        }
    };
    println!("Fetched current state: {:?}", initial_state);

    let contract_input = build_contract_input(initial_state);

    println!("{}", "-".repeat(20));
    println!("Checking transition for {contract_name}...");
    let execute_info = execute(contract_name, &contract_input);
    let output = execute_info.journal.decode::<HyleOutput>().unwrap();
    if !output.success {
        let program_error = std::str::from_utf8(&output.program_outputs).unwrap();
        println!(
            "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
            program_error
        );
        return;
    } else {
        let next_state: State = output.next_state.try_into().unwrap();
        println!("New state: {:?}", next_state);
    }

    println!("{}", "-".repeat(20));
    println!("Proving transition for {contract_name}...");
    let prove_info = prove(contract_name, &contract_input);

    let receipt = prove_info.receipt;
    let encoded_receipt = to_vec(&receipt).expect("Unable to encode receipt");
    std::fs::write(
        format!("{}.risc0.proof", contract_input.index),
        encoded_receipt,
    )
    .unwrap();

    let claim = receipt.claim().unwrap().value().unwrap();

    let hyle_output = receipt
        .journal
        .decode::<HyleOutput>()
        .expect("Failed to decode journal");

    if !hyle_output.success {
        let program_error = std::str::from_utf8(&hyle_output.program_outputs).unwrap();
        println!(
            "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
            program_error
        );
        return;
    }

    println!("{}", "-".repeat(20));
    let method_id = claim.pre.digest();
    let initial_state = hex::encode(&hyle_output.initial_state.0);
    println!("Method ID: {:?} (hex)", method_id);
    println!(
        "{}.risc0.proof written, transition from {:?} to {:?}",
        contract_input.index,
        initial_state,
        hex::encode(&hyle_output.next_state.0)
    );
    println!("{:?}", hyle_output);

    println!("{}", "-".repeat(20));

    println!("You can send the proof tx:");
    println!(
        "\x1b[93mhyled proof $BLOB_TX_HASH {contract_name} {}.risc0.proof \x1b[0m",
        contract_input.index
    );

    receipt
        .verify(claim.pre.digest())
        .expect("Verification 2 failed");
}

pub fn fetch_current_state<State>(cli: &Cli, contract_name: &str) -> Result<State, Error>
where
    State: TryFrom<sdk::StateDigest, Error = Error>,
{
    let url = format!("http://{}:{}", cli.host, cli.port);
    let resp = reqwest::blocking::get(format!("{}/v1/contract/{}", url, contract_name))?;

    let status = resp.status();
    let body = resp.text()?;

    if let Ok(contract) = serde_json::from_str::<Contract>(&body) {
        println!("{}", "-".repeat(20));
        println!("Fetched contract: {:?}", contract);
        Ok(contract.state.try_into()?)
    } else {
        bail!(
            "Failed to parse JSON response, status: {}, body: {}",
            status,
            body
        );
    }
}

fn execute<State>(
    contract_name: &str,
    contract_input: &ContractInput<State>,
) -> risc0_zkvm::SessionInfo
where
    State: Digestable + serde::Serialize,
{
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(contract_input)
        .unwrap()
        .build()
        .unwrap();

    let prover = risc0_zkvm::default_executor();
    let program_name = get_image(contract_name);
    let file_path = format!("contracts/{}/{}.img", program_name, program_name);
    println!("file_path: {}", file_path);
    if let Ok(binary) = std::fs::read(file_path.as_str()) {
        prover.execute(env, &binary).unwrap()
    } else {
        println!("Could not read ELF binary at {}.", file_path);
        println!("Please ensure that the ELF binary is built and located at the specified path.");
        println!("\x1b[93m--> Tip: Did you run build_contracts.sh ?\x1b[0m");
        panic!("Could not read ELF binary");
    }
}

fn prove<State>(contract_name: &str, contract_input: &ContractInput<State>) -> risc0_zkvm::ProveInfo
where
    State: Digestable + serde::Serialize,
{
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(contract_input)
        .unwrap()
        .build()
        .unwrap();

    let prover = risc0_zkvm::default_prover();
    let program_name = get_image(contract_name);
    let file_path = format!("contracts/{}/{}.img", program_name, program_name);
    if let Ok(binary) = std::fs::read(file_path.as_str()) {
        prover.prove(env, &binary).unwrap()
    } else {
        println!("Could not read ELF binary at {}.", file_path);
        println!("Please ensure that the ELF binary is built and located at the specified path.");
        println!("\x1b[93m--> Tip: Did you run build_contracts.sh ?\x1b[0m");
        panic!("Could not read ELF binary");
    }
}

fn get_image(contract_name: &str) -> &str {
    match contract_name {
        "hydentity" => "hydentity",
        "hyllar" | "hyllar2" => "hyllar",
        "amm" | "amm2" => "amm",
        _ => panic!("Unknown contract name"),
    }
}
