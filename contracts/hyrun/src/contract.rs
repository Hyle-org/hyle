use anyhow::{bail, Error};
use borsh::to_vec;
use risc0_zkvm::sha::Digestible;
use sdk::HyleOutput;

use crate::{Cli, Contract};

pub fn run<ContractFunction, State, ContractInput, Builder>(
    cli: &Cli,
    contract_name: &str,
    program_inputs: ContractFunction,
    build_contract_input: Builder,
) where
    ContractFunction: bincode::Encode + std::fmt::Debug + Clone,
    State: Default + std::fmt::Debug + TryFrom<sdk::StateDigest, Error = Error>,
    ContractInput: serde::Serialize,
    Builder: Fn(State) -> ContractInput,
{
    let initial_state = if cli.init {
        State::default()
    } else {
        match fetch_current_state(cli, contract_name) {
            Ok(s) => s,
            Err(e) => {
                println!("fetch current state error: {}", e);
                return;
            }
        }
    };
    println!("Inital state: {:?}", initial_state);

    let prove_info = prove(initial_state, build_contract_input);

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

    if cli.init {
        println!("You can register the contract by running:");
        println!(
            "hyled contract default risc0 {} {} {}",
            method_id, contract_name, initial_state
        );
    }
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
    if let Ok(binary) =
        std::fs::read("target/riscv-guest/riscv32im-risc0-zkvm-elf/docker/hyfi_guest/hyfi-guest")
    {
        prover.prove(env, &binary).unwrap()
    } else {
        println!("Could not read ELF binary at target/riscv-guest/riscv32im-risc0-zkvm-elf/docker/hyfi_guest/hyfi-guest.");
        println!("Please ensure that the ELF binary is built and located at the specified path.");
        println!("\x1b[93m--> Tip: Did you run build_contracts.sh ?\x1b[0m");
        panic!("Could not read ELF binary");
    }
}
