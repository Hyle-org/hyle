use anyhow::{bail, Error};
use borsh::to_vec;
use risc0_zkvm::sha::Digestible;
use sdk::{Digestable, HyleOutput};

use crate::{Cli, Contract};

pub fn init<State>(contract_name: &str, initial_state: State)
where
    State: Digestable + std::fmt::Debug,
{
    println!("Initial state: {:?}", initial_state);
    let initial_state = hex::encode(initial_state.as_digest().0);
    let file_path = format!("contracts/{}/{}.txt", contract_name, contract_name);
    let image_id = std::fs::read_to_string(file_path)
        .expect("Unable to read image id")
        .trim_end()
        .to_string();

    println!("You can register the contract by running:");
    println!(
        "hyled contract default risc0 {} {} {}",
        image_id, contract_name, initial_state
    );
}

pub fn run<ContractFunction, State, ContractInput, Builder>(
    cli: &Cli,
    contract_name: &str,
    program_inputs: ContractFunction,
    build_contract_input: Builder,
) where
    ContractFunction: bincode::Encode + std::fmt::Debug + Clone,
    State: Default + std::fmt::Debug + TryFrom<sdk::StateDigest, Error = Error>,
    ContractInput: serde::Serialize,
    State: Digestable,
    Builder: Fn(State) -> ContractInput,
{
    let initial_state = match fetch_current_state(cli, contract_name) {
        Ok(s) => s,
        Err(e) => {
            println!("fetch current state error: {}", e);
            return;
        }
    };
    println!("Inital state: {:?}", initial_state);

    let prove_info = prove(initial_state, build_contract_input, contract_name);

    let receipt = prove_info.receipt;
    let encoded_receipt = to_vec(&receipt).expect("Unable to encode receipt");
    std::fs::write("risc0.proof", encoded_receipt).unwrap();

    let claim = receipt.claim().unwrap().value().unwrap();

    let hyle_output = receipt
        .journal
        .decode::<HyleOutput>()
        .expect("Failed to decode journal");

    println!("{}", "-".repeat(20));
    let method_id = claim.pre.digest();
    let initial_state = hex::encode(&hyle_output.initial_state.0);
    println!("Method ID: {:?} (hex)", method_id);
    println!(
        "risc0.proof written, transition from {:?} to {:?}",
        initial_state,
        hex::encode(&hyle_output.next_state.0)
    );
    println!("{:?}", hyle_output);

    let hex_program_inputs = hex::encode(
        bincode::encode_to_vec(program_inputs.clone(), bincode::config::standard())
            .expect("failed to encode program inputs"),
    );
    println!("{}", "-".repeat(20));

    println!("You can send the blob tx:");
    println!(
        "hyled blob IDENTITY {} {}",
        contract_name, hex_program_inputs
    );
    println!("You can send the proof tx:");
    println!("hyled proof BLOB_TX_HASH 0 {} risc0.proof", contract_name);

    receipt
        .verify(claim.pre.digest())
        .expect("Verification 2 failed");
}

fn fetch_current_state<State>(cli: &Cli, contract_name: &str) -> Result<State, Error>
where
    State: TryFrom<sdk::StateDigest, Error = Error>,
{
    let url = format!("http://{}:{}", cli.host, cli.port);
    let resp = reqwest::blocking::get(format!("{}/v1/contract/{}", url, contract_name))?;

    let status = resp.status();
    let body = resp.text()?;

    if let Ok(contract) = serde_json::from_str::<Contract>(&body) {
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

fn prove<State, ContractInput, Builder>(
    balances: State,
    build_contract_input: Builder,
    contract_name: &str,
) -> risc0_zkvm::ProveInfo
where
    ContractInput: serde::Serialize,
    Builder: Fn(State) -> ContractInput,
{
    let contract_input = build_contract_input(balances);

    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(&contract_input)
        .unwrap()
        .build()
        .unwrap();

    let prover = risc0_zkvm::default_prover();
    let file_path = format!("contracts/{}/{}.img", contract_name, contract_name);
    if let Ok(binary) = std::fs::read(file_path.as_str()) {
        prover.prove(env, &binary).unwrap()
    } else {
        println!("Could not read ELF binary at {}.", file_path);
        println!("Please ensure that the ELF binary is built and located at the specified path.");
        println!("\x1b[93m--> Tip: Did you run build_contracts.sh ?\x1b[0m");
        panic!("Could not read ELF binary");
    }
}
