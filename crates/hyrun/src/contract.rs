use anyhow::{bail, Error};
use risc0_zkvm::sha::Digestible;
use sdk::{Blob, ContractInput, Digestable, HyleOutput, Identity, StateDigest};

use crate::{Context, Contract};

pub fn init<State>(context: &Context, contract_name: &str, initial_state: State)
where
    State: Digestable + std::fmt::Debug,
{
    println!("Initial state: {:?}", initial_state);
    let initial_state = hex::encode(initial_state.as_digest().0);

    println!("You can register the contract by running:");
    println!(
        "\x1b[93mhyled contract default risc0 {} {} {} \x1b[0m",
        hex::encode(get_image_id(context, contract_name)),
        contract_name,
        initial_state
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

pub fn run<State, Builder>(context: &Context, contract_name: &str, build_contract_input: Builder)
where
    State: TryFrom<StateDigest, Error = Error>,
    State: Digestable + std::fmt::Debug + serde::Serialize,
    Builder: Fn(State) -> ContractInput,
{
    let initial_state =
        get_initial_state(context, contract_name).expect("Cannot get contract state");
    println!("Fetched current state: {:?}", initial_state);

    let contract_input = build_contract_input(initial_state);

    println!("{}", "-".repeat(20));
    println!("Checking transition for {contract_name}...");
    let execute_info = execute(context, contract_name, &contract_input);
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
    let prove_info = prove(context, contract_name, &contract_input);

    let receipt = prove_info.receipt;
    let encoded_receipt = borsh::to_vec(&receipt).expect("Unable to encode receipt");
    std::fs::write(
        format!(
            "{}/{}.risc0.proof",
            context.cli.proof_path, contract_input.index
        ),
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

pub fn get_initial_state<State>(context: &Context, contract_name: &str) -> Result<State, Error>
where
    State: TryFrom<StateDigest, Error = Error>,
{
    context
        .hardcoded_initial_states
        .get(contract_name)
        .cloned()
        .unwrap_or_else(|| {
            fetch_current_state(context, contract_name)
                .expect("Cannot fetch current state from node")
        })
        .try_into()
}

pub fn fetch_current_state(context: &Context, contract_name: &str) -> Result<StateDigest, Error> {
    let url = format!("http://{}:{}", context.cli.host, context.cli.port);
    let resp = reqwest::blocking::get(format!("{}/v1/contract/{}", url, contract_name))?;

    let status = resp.status();
    let body = resp.text()?;

    if let Ok(contract) = serde_json::from_str::<Contract>(&body) {
        println!("{}", "-".repeat(20));
        println!("Fetched contract: {:?}", contract);
        Ok(contract.state)
    } else {
        bail!(
            "Failed to parse JSON response, status: {}, body: {}",
            status,
            body
        );
    }
}

fn execute(
    context: &Context,
    contract_name: &str,
    contract_input: &ContractInput,
) -> risc0_zkvm::SessionInfo {
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(contract_input)
        .unwrap()
        .build()
        .unwrap();

    let prover = risc0_zkvm::default_executor();
    prover
        .execute(env, get_elf(context, contract_name))
        .unwrap()
}

fn prove(
    context: &Context,
    contract_name: &str,
    contract_input: &ContractInput,
) -> risc0_zkvm::ProveInfo {
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(contract_input)
        .unwrap()
        .build()
        .unwrap();

    let prover = risc0_zkvm::default_prover();
    prover.prove(env, get_elf(context, contract_name)).unwrap()
}

fn get_elf<'a>(context: &'a Context, contract_name: &str) -> &'a Vec<u8> {
    match contract_name {
        "hydentity" => &context.contract_data.hydentity_elf,
        "hyllar" | "hyllar2" => &context.contract_data.hyllar_elf,
        "amm" | "amm2" => &context.contract_data.amm_elf,
        _ => panic!("Unknown contract name"),
    }
}

fn get_image_id<'a>(context: &'a Context, contract_name: &str) -> &'a Vec<u8> {
    match contract_name {
        "hydentity" => &context.contract_data.hydentity_id,
        "hyllar" | "hyllar2" => &context.contract_data.hyllar_id,
        "amm" | "amm2" => &context.contract_data.amm_id,
        _ => panic!("Unknown contract name"),
    }
}
