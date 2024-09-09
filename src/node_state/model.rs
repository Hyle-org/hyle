#![allow(dead_code, unused_variables)]
use std::collections::HashMap;

struct BlockHeight(u64);
struct PayloadIndex(u32);
struct Identity(String);
struct ContractName(String);
struct TxHash(String);
struct PayloadsHash(String);

#[derive(Default)]
pub struct NodeState {
    timeouts: Timeouts,
    contracts: HashMap<ContractName, Contract>,
    transactions: HashMap<TxHash, UnsettledTransaction>,
    tx_order: HashMap<ContractName, Vec<TxHash>>,
}

#[derive(Default)]
struct Timeouts {
    by_block: HashMap<BlockHeight, Vec<TxHash>>,
    by_tx_hash: HashMap<TxHash, BlockHeight>,
}

struct Contract {
    name: ContractName,
    program_id: u64,
    state: Vec<u8>,
}

struct UnsettledTransaction {
    identity: Identity,
    hash: TxHash,
    payloads_hash: PayloadsHash,
    payloads: HashMap<PayloadIndex, PayloadDetail>,
}

#[derive(Debug)]
struct PayloadDetail {
    contract_name: String,
    verification_status: VerificationStatus,
    hyle_output: Option<HyleOutput>,
}

#[derive(Debug)]
enum VerificationStatus {
    WaitingProof,
    Success,
    InvalidProof,
    ExecutionFailed,
}

#[derive(Debug)]
pub struct HyleOutput {
    pub version: u32,
    pub initial_state: Vec<u8>,
    pub next_state: Vec<u8>,
    pub identity: String,
    pub tx_hash: Vec<u8>,
    pub index: u32,
    pub payloads: Vec<u8>,
    pub success: bool,
}

impl NodeState {
    pub fn tt() {}
}

impl Timeouts {
    pub fn list_timeouts(&self, at: BlockHeight) {}
    pub fn get(&self, tx: &TxHash) -> BlockHeight {
        todo!()
    }
    pub fn set(&mut self, tx: TxHash, at: BlockHeight) {}
}
