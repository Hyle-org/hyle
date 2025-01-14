use core::panic;
use std::collections::HashMap;

use amm::{AmmAction, AmmState};
use client_sdk::{BlobTransaction, Hashable};
use hydentity::Hydentity;
use hyllar::HyllarToken;
use sdk::{
    erc20::ERC20Action, identity_provider::IdentityAction, BlobData, BlobIndex, ContractAction,
    ContractInput, ContractName, Digestable, StateDigest,
};
use serde::Deserialize;

use clap::{Parser, Subcommand};

mod contract;

#[derive(Deserialize, Debug)]
pub struct Contract {
    pub name: ContractName,
    pub program_id: Vec<u8>,
    pub state: sdk::StateDigest,
    pub verifier: String,
}

#[derive(Subcommand, Clone)]
pub enum HydentityArgs {
    Init,
    Register { account: String },
}
impl From<HydentityArgs> for sdk::identity_provider::IdentityAction {
    fn from(cmd: HydentityArgs) -> Self {
        match cmd {
            HydentityArgs::Register { account } => Self::RegisterIdentity { account },
            HydentityArgs::Init => panic!("Init is not a valid contract function"),
        }
    }
}
#[derive(Subcommand, Clone)]
pub enum HyllarArgs {
    Init { initial_supply: u128 },
    Approve { spender: String, amount: u128 },
    Transfer { recipient: String, amount: u128 },
}
impl From<HyllarArgs> for ERC20Action {
    fn from(cmd: HyllarArgs) -> Self {
        match cmd {
            HyllarArgs::Transfer { recipient, amount } => Self::Transfer { recipient, amount },
            HyllarArgs::Approve { spender, amount } => Self::Approve { spender, amount },
            HyllarArgs::Init { .. } => panic!("Init is not a valid contract function"),
        }
    }
}
#[derive(Subcommand, Clone)]
pub enum AmmArgs {
    NewPair {
        token_a: String,
        token_b: String,
        amount_a: u128,
        amount_b: u128,
    },
    Swap {
        token_a: String,
        token_b: String,
        amount_a: u128,
        amount_b: u128,
    },
}

#[derive(Subcommand, Clone)]
pub enum CliCommand {
    State {
        contract: String,
    },
    Hydentity {
        #[command(subcommand)]
        command: HydentityArgs,
    },
    Hyllar {
        #[command(subcommand)]
        command: HyllarArgs,
        hyllar_contract_name: String,
    },
    Amm {
        #[command(subcommand)]
        command: AmmArgs,
        amm_contract_name: String,
    },
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: CliCommand,

    #[arg(long, short)]
    pub user: Option<String>,

    #[arg(long, short)]
    pub password: Option<String>,

    #[arg(long, short)]
    pub nonce: Option<u32>,

    #[arg(long, default_value = "localhost")]
    pub host: String,

    #[arg(long, default_value = "4321")]
    pub port: u32,

    #[arg(long, default_value = ".")]
    pub proof_path: String,
}

pub struct Context {
    pub cli: Cli,
    pub contract_data: ContractData,
    pub hardcoded_initial_states: HashMap<String, StateDigest>,
}

pub struct ContractData {
    pub amm_elf: Vec<u8>,
    pub amm_id: Vec<u8>,
    pub hydentity_elf: Vec<u8>,
    pub hydentity_id: Vec<u8>,
    pub hyllar_elf: Vec<u8>,
    pub hyllar_id: Vec<u8>,
}

impl Default for ContractData {
    fn default() -> Self {
        Self {
            amm_elf: amm::metadata::AMM_ELF.to_vec(),
            amm_id: amm::metadata::PROGRAM_ID.to_vec(),
            hydentity_elf: hydentity::metadata::HYDENTITY_ELF.to_vec(),
            hydentity_id: hydentity::metadata::PROGRAM_ID.to_vec(),
            hyllar_elf: hyllar::metadata::HYLLAR_ELF.to_vec(),
            hyllar_id: hyllar::metadata::PROGRAM_ID.to_vec(),
        }
    }
}

// Public because it's used in integration tests.
pub fn run_command(context: &Context) {
    match context.cli.command.clone() {
        CliCommand::State { contract } => match contract.as_str() {
            "hydentity" => {
                let state = contract::get_initial_state::<Hydentity>(context, &contract)
                    .expect("failed to fetch state");
                println!("State: {:?}", state);
            }
            "hyllar" | "hyllar2" => {
                let state = contract::get_initial_state::<HyllarToken>(context, &contract)
                    .expect("failed to fetch state");
                println!("State: {:?}", state);
            }
            "amm" | "amm2" => {
                let state = contract::get_initial_state::<AmmState>(context, &contract)
                    .expect("failed to fetch state");
                println!("State: {:?}", state);
            }
            _ => panic!("Unknown contract"),
        },
        CliCommand::Hydentity { command } => {
            if matches!(command, HydentityArgs::Init) {
                contract::init(context, "hydentity", Hydentity::new());
                return;
            }
            let cf: IdentityAction = command.into();
            let identity = sdk::Identity(
                context
                    .cli
                    .user
                    .clone()
                    .unwrap_or_else(|| panic!("Missing user argument")),
            );
            let password = context
                .cli
                .password
                .clone()
                .unwrap_or_else(|| panic!("Missing password argument"))
                .as_bytes()
                .to_vec();
            let blobs = vec![cf.as_blob(ContractName::new("hydentity"))];
            contract::print_hyled_blob_tx(&identity, &blobs);

            let tx_hash = BlobTransaction {
                identity: identity.clone(),
                blobs: blobs.clone(),
            }
            .hash();

            contract::run(
                context,
                "hydentity",
                |identities: Hydentity| -> ContractInput {
                    ContractInput {
                        initial_state: identities.as_digest(),
                        identity: identity.clone(),
                        tx_hash: tx_hash.clone(),
                        private_blob: BlobData(password.clone()),
                        blobs: blobs.clone(),
                        index: BlobIndex(0),
                    }
                },
            );
        }
        CliCommand::Hyllar {
            hyllar_contract_name,
            command,
        } => {
            if let HyllarArgs::Init { initial_supply } = command {
                contract::init(
                    context,
                    &hyllar_contract_name,
                    HyllarToken::new(initial_supply, "faucet.hydentity".to_string()),
                );
                return;
            }
            let cf: ERC20Action = command.into();
            let identity = context
                .cli
                .user
                .clone()
                .unwrap_or_else(|| panic!("Missing user argument"));
            let nonce = context
                .cli
                .nonce
                .unwrap_or_else(|| panic!("Missing nonce argument"));
            let password = context
                .cli
                .password
                .clone()
                .unwrap_or_else(|| panic!("Missing password argument"))
                .as_bytes()
                .to_vec();
            let identity_cf: IdentityAction = IdentityAction::VerifyIdentity {
                account: identity.clone(),
                nonce,
            };

            let blobs = vec![
                identity_cf.as_blob(ContractName::new("hydentity")),
                cf.as_blob(ContractName(hyllar_contract_name.clone()), None, None),
            ];
            contract::print_hyled_blob_tx(&identity.clone().into(), &blobs);

            let tx_hash = BlobTransaction {
                identity: identity.clone().into(),
                blobs: blobs.clone(),
            }
            .hash();

            contract::run(
                context,
                "hydentity",
                |token: hydentity::Hydentity| -> ContractInput {
                    ContractInput {
                        initial_state: token.as_digest(),
                        identity: identity.clone().into(),
                        tx_hash: tx_hash.clone(),
                        private_blob: BlobData(password.clone()),
                        blobs: blobs.clone(),
                        index: BlobIndex(0),
                    }
                },
            );
            contract::run(
                context,
                &hyllar_contract_name,
                |token: hyllar::HyllarToken| -> ContractInput {
                    ContractInput {
                        initial_state: token.as_digest(),
                        identity: identity.clone().into(),
                        tx_hash: tx_hash.clone(),
                        private_blob: BlobData(vec![]),
                        blobs: blobs.clone(),
                        index: BlobIndex(1),
                    }
                },
            );
        }
        CliCommand::Amm {
            amm_contract_name,
            command,
        } => {
            match command {
                AmmArgs::NewPair {
                    token_a,
                    token_b,
                    amount_a,
                    amount_b,
                } => {
                    let identity = context
                        .cli
                        .user
                        .clone()
                        .unwrap_or_else(|| panic!("Missing user argument"));
                    let nonce = context
                        .cli
                        .nonce
                        .unwrap_or_else(|| panic!("Missing nonce argument"));
                    let password = context
                        .cli
                        .password
                        .clone()
                        .unwrap_or_else(|| panic!("Missing password argument"))
                        .as_bytes()
                        .to_vec();
                    let identity_cf: IdentityAction = IdentityAction::VerifyIdentity {
                        account: identity.clone(),
                        nonce,
                    };

                    let blobs = vec![
                        identity_cf.as_blob(ContractName::new("hydentity")),
                        AmmAction::NewPair {
                            pair: (token_a.clone(), token_b.clone()),
                            amounts: (amount_a, amount_b),
                        }
                        .as_blob(
                            ContractName::new(amm_contract_name.clone()),
                            None,
                            Some(vec![BlobIndex(2), BlobIndex(3)]),
                        ),
                        ERC20Action::TransferFrom {
                            sender: identity.clone(),
                            recipient: amm_contract_name.to_string(),
                            amount: amount_a,
                        }
                        .as_blob(
                            ContractName::new(token_a.clone()),
                            Some(BlobIndex(1)),
                            None,
                        ),
                        ERC20Action::TransferFrom {
                            sender: identity.clone(),
                            recipient: amm_contract_name.to_string(),
                            amount: amount_b,
                        }
                        .as_blob(
                            ContractName::new(token_b.clone()),
                            Some(BlobIndex(1)),
                            None,
                        ),
                    ];
                    contract::print_hyled_blob_tx(&identity.clone().into(), &blobs);

                    let tx_hash = BlobTransaction {
                        identity: identity.clone().into(),
                        blobs: blobs.clone(),
                    }
                    .hash();

                    contract::run(
                        context,
                        "hydentity",
                        |token: hydentity::Hydentity| -> ContractInput {
                            ContractInput {
                                initial_state: token.as_digest(),
                                identity: identity.clone().into(),
                                tx_hash: tx_hash.clone(),
                                private_blob: BlobData(password.clone()),
                                blobs: blobs.clone(),
                                index: BlobIndex(0),
                            }
                        },
                    );
                    // Run to add new pair to Amm
                    contract::run(
                        context,
                        &amm_contract_name,
                        |amm: amm::AmmState| -> ContractInput {
                            ContractInput {
                                initial_state: amm.as_digest(),
                                identity: identity.clone().into(),
                                tx_hash: tx_hash.clone(),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(1),
                            }
                        },
                    );
                    // Run for transferring token_a
                    contract::run(
                        context,
                        &token_a,
                        |token: hyllar::HyllarToken| -> ContractInput {
                            ContractInput {
                                initial_state: token.as_digest(),
                                identity: identity.clone().into(),
                                tx_hash: tx_hash.clone(),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(2),
                            }
                        },
                    );

                    // Run for transferring token_b
                    contract::run(
                        context,
                        &token_b,
                        |token: hyllar::HyllarToken| -> ContractInput {
                            ContractInput {
                                initial_state: token.as_digest(),
                                identity: identity.clone().into(),
                                tx_hash: tx_hash.clone(),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(3),
                            }
                        },
                    );
                }
                AmmArgs::Swap {
                    token_a,
                    token_b,
                    amount_a,
                    amount_b,
                } => {
                    let identity = context
                        .cli
                        .user
                        .clone()
                        .unwrap_or_else(|| panic!("Missing user argument"));
                    let nonce = context
                        .cli
                        .nonce
                        .unwrap_or_else(|| panic!("Missing nonce argument"));
                    let password = context
                        .cli
                        .password
                        .clone()
                        .unwrap_or_else(|| panic!("Missing password argument"))
                        .as_bytes()
                        .to_vec();
                    let identity_cf: IdentityAction = IdentityAction::VerifyIdentity {
                        account: identity.clone(),
                        nonce,
                    };

                    let blobs = vec![
                        identity_cf.as_blob(ContractName::new("hydentity")),
                        AmmAction::Swap {
                            pair: (token_a.to_string(), token_b.to_string()),
                            amounts: (amount_a, amount_b),
                        }
                        .as_blob(
                            ContractName::new(amm_contract_name.clone()),
                            None,
                            Some(vec![BlobIndex(2), BlobIndex(3)]),
                        ),
                        ERC20Action::TransferFrom {
                            sender: identity.clone(),
                            recipient: amm_contract_name.to_string(),
                            amount: amount_a,
                        }
                        .as_blob(
                            ContractName::new(token_a.clone()),
                            Some(BlobIndex(1)),
                            None,
                        ),
                        ERC20Action::Transfer {
                            recipient: identity.clone(),
                            amount: amount_b,
                        }
                        .as_blob(
                            ContractName::new(token_b.clone()),
                            Some(BlobIndex(1)),
                            None,
                        ),
                    ];

                    contract::print_hyled_blob_tx(&identity.clone().into(), &blobs);

                    let tx_hash = BlobTransaction {
                        identity: identity.clone().into(),
                        blobs: blobs.clone(),
                    }
                    .hash();

                    contract::run(
                        context,
                        "hydentity",
                        |token: hydentity::Hydentity| -> ContractInput {
                            ContractInput {
                                initial_state: token.as_digest(),
                                identity: identity.clone().into(),
                                tx_hash: tx_hash.clone(),
                                private_blob: BlobData(password.clone()),
                                blobs: blobs.clone(),
                                index: BlobIndex(0),
                            }
                        },
                    );
                    // Run for swapping token_a for token_b
                    contract::run(
                        context,
                        &amm_contract_name,
                        |amm: amm::AmmState| -> ContractInput {
                            ContractInput {
                                initial_state: amm.as_digest(),
                                identity: identity.clone().into(),
                                tx_hash: tx_hash.clone(),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(1),
                            }
                        },
                    );
                    // Run for transferring token_a
                    contract::run(
                        context,
                        &token_a,
                        |token: hyllar::HyllarToken| -> ContractInput {
                            ContractInput {
                                initial_state: token.as_digest(),
                                identity: identity.clone().into(),
                                tx_hash: tx_hash.clone(),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(2),
                            }
                        },
                    );

                    // Run for transferring token_b
                    contract::run(
                        context,
                        &token_b,
                        |token: hyllar::HyllarToken| -> ContractInput {
                            ContractInput {
                                initial_state: token.as_digest(),
                                identity: identity.clone().into(),
                                tx_hash: tx_hash.clone(),
                                private_blob: BlobData(vec![]),
                                blobs: blobs.clone(),
                                index: BlobIndex(3),
                            }
                        },
                    );
                }
            }
        }
    };
}
