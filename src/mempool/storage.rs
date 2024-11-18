use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    io::Write,
    sync::Arc,
    vec,
};
use tracing::{debug, error, warn};

use crate::{
    model::{Hashable, Transaction, TransactionData, ValidatorPublicKey},
    node_state::NodeState,
    p2p::network::SignedByValidator,
    utils::crypto::{AggregateSignature, BlstCrypto, ValidatorSignature},
};

use super::MempoolNetMessage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataProposalVerdict {
    Empty,
    Wait(Option<CarHash>),
    Vote,
    DidVote,
    Refuse,
}

pub type Cut = Vec<(ValidatorPublicKey, Option<AggregateSignature>, CarHash)>;

pub fn verify_cut(_cut: &Cut) -> Result<()> {
    // FIXME: verify cut
    // for (_validator, signature, car_hash) in cut {
    //     let expected_signed_message = Signed {
    //         msg: MempoolNetMessage::DataVote(car_hash.clone()),
    //         signature: signature.clone().unwrap(),
    //     };

    //     match BlstCrypto::verify_aggregate(&expected_signed_message) {
    //         Ok(res) if !res => {
    //             bail!("Cut received is invalid ({validator})")
    //         }
    //         Err(err) => bail!("Cut verification failed for {validator}", err),
    //         _ => {}
    //     };
    // }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Storage {
    pub id: ValidatorPublicKey,
    pub pending_txs: Vec<Transaction>,
    pub data_proposal: Option<DataProposal>,
    pub lane: Lane,
    pub other_lanes: HashMap<ValidatorPublicKey, Lane>,
}

impl Display for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Replica {}", self.id)?;
        write!(f, "\nLane {}", self.lane)?;
        for (i, lane) in self.other_lanes.iter() {
            write!(f, "\n - OL {}: {}", i, lane)?;
        }

        Ok(())
    }
}

impl Storage {
    pub fn new(id: ValidatorPublicKey) -> Storage {
        Storage {
            id,
            pending_txs: vec![],
            data_proposal: None,
            lane: Lane {
                cars: vec![],
                poa: Poa::default(),
                waiting: vec![],
            },
            other_lanes: HashMap::new(),
        }
    }

    pub fn new_cut(
        &mut self,
        crypto: Arc<BlstCrypto>,
        validators: &[ValidatorPublicKey],
    ) -> Result<Cut> {
        // For each validator, we get the last validated car and put it in the cut
        let mut cut: Cut = vec![];
        for validator in validators.iter() {
            if validator == &self.id {
                self.lane
                    .add_current_hash_to_cut(&crypto, &mut cut, validator)?;
            } else if let Some(lane) = self.other_lanes.get(validator) {
                lane.add_current_hash_to_cut(&crypto, &mut cut, validator)?;
            } else {
                // can happen if validator does not have any DataProposal yet
                debug!(
                    "Validator {} not found in lane of {} (cutting)",
                    validator, self.id
                );
            }
        }

        Ok(cut)
    }

    pub fn commit_data_proposal(&mut self) {
        if let Some(data_proposal) = self.data_proposal.take() {
            self.lane.cars.push(data_proposal.car);
        }
    }

    // Called by the initial proposal validator to aggregate votes
    pub fn on_data_vote(
        &mut self,
        validator: &ValidatorPublicKey,
        car_hash: CarHash,
        message: SignedByValidator<MempoolNetMessage>,
    ) -> Result<()> {
        if let Some(current_data_proposal) = &self.data_proposal {
            if current_data_proposal.car.hash() == car_hash {
                if self.lane.poa.contains_key(validator) {
                    debug!("{validator} already voted for DataProposal");
                } else {
                    self.lane.poa.insert(validator.clone(), message);
                }
            } else {
                bail!("DataVote for wrong DataProposal ({validator})");
            }
        } else {
            debug!("Unexpected DataVote received from {validator} (no current DataProposal)");
        }
        Ok(())
    }

    pub fn on_data_proposal(
        &mut self,
        crypto: Arc<BlstCrypto>,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
        node_state: &NodeState,
    ) -> DataProposalVerdict {
        if data_proposal.car.txs.is_empty() {
            return DataProposalVerdict::Empty;
        }
        if self.other_lane_has_data_proposal(validator, data_proposal) {
            return DataProposalVerdict::DidVote;
        }
        if !self.other_lane_has_parent_data_proposal(validator, data_proposal) {
            self.other_lanes
                .entry(validator.clone())
                .or_default()
                .waiting
                .push(data_proposal.clone());
            return DataProposalVerdict::Wait(data_proposal.car.parent_hash.clone());
        }

        // FIXME: verify data_proposal.parent_poa

        // optimistic_node_state is here to handle the case where a contract is registered in a car that is not yet committed.
        // For performance reasons, we only clone node_state in it for unregistered contracts that are potentially in those uncommitted cars.
        let mut optimistic_node_state: Option<NodeState> = None;
        for tx in data_proposal.car.txs.iter() {
            match &tx.transaction_data {
                TransactionData::Proof(_) => {
                    warn!("Refusing DataProposal: unverified proof transaction");
                    return DataProposalVerdict::Refuse;
                }
                TransactionData::VerifiedProof(proof_tx) => {
                    // Ensure all referenced contracts are registered
                    for blob_ref in &proof_tx.proof_transaction.blobs_references {
                        if !node_state.contracts.contains_key(&blob_ref.contract_name)
                            && !optimistic_node_state.as_ref().map_or(false, |state| {
                                state.contracts.contains_key(&blob_ref.contract_name)
                            })
                        {
                            // Process previous cars to register the missing contract
                            if let Some(lane) = self.other_lanes.get_mut(validator) {
                                for car in &lane.cars {
                                    for tx in &car.txs {
                                        if let TransactionData::RegisterContract(reg_tx) =
                                            &tx.transaction_data
                                        {
                                            if reg_tx.contract_name == blob_ref.contract_name {
                                                if optimistic_node_state.is_none() {
                                                    optimistic_node_state =
                                                        Some(node_state.clone());
                                                }
                                                if optimistic_node_state
                                                    .as_mut()
                                                    .unwrap()
                                                    .handle_register_contract_tx(reg_tx)
                                                    .is_err()
                                                {
                                                    // Register transactions in a validated car should never fail
                                                    // as car is accepted only if all transactions are valid
                                                    unreachable!();
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Verifying the proof before voting
                    match optimistic_node_state
                        .as_ref()
                        .unwrap_or(node_state)
                        .verify_proof(&proof_tx.proof_transaction)
                    {
                        Ok(hyle_outputs) => {
                            if hyle_outputs != proof_tx.hyle_outputs {
                                warn!("Refusing DataProposal: incorrect HyleOutput in proof transaction");
                                return DataProposalVerdict::Refuse;
                            }
                        }
                        Err(e) => {
                            warn!("Refusing DataProposal: invalid proof transaction: {}", e);
                            return DataProposalVerdict::Refuse;
                        }
                    }
                }
                TransactionData::RegisterContract(register_contract_tx) => {
                    if optimistic_node_state.is_none() {
                        optimistic_node_state = Some(node_state.clone());
                    }
                    match optimistic_node_state
                        .as_mut()
                        .unwrap()
                        .handle_register_contract_tx(register_contract_tx)
                    {
                        Ok(_) => (),
                        Err(e) => {
                            warn!("Refusing DataProposal: {}", e);
                            return DataProposalVerdict::Refuse;
                        }
                    }
                }
                _ => {}
            }
        }
        if let (Some(parent_hash), Some(parent_poa)) =
            (&data_proposal.car.parent_hash, &data_proposal.parent_poa)
        {
            self.other_lane_update_parent_poa(validator, parent_hash, parent_poa)
        }
        self.other_lane_add_data_proposal(crypto, validator, data_proposal);
        DataProposalVerdict::Vote
    }

    fn other_lane_add_data_proposal(
        &mut self,
        crypto: Arc<BlstCrypto>,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        let parent_hash = lane.current_hash();
        // Removing proofs from transactions
        let mut txs_without_proofs = data_proposal.car.txs.clone();
        txs_without_proofs.iter_mut().for_each(|tx| {
            match &mut tx.transaction_data {
                TransactionData::VerifiedProof(proof_tx) => {
                    proof_tx.proof_transaction.proof = Default::default();
                }
                TransactionData::Proof(_) => {
                    // This can never happen.
                    // A DataProposal that has been processed has turned all TransactionData::Proof into TransactionData::VerifiedProof
                    unreachable!();
                }
                TransactionData::Blob(_)
                | TransactionData::Stake(_)
                | TransactionData::RegisterContract(_) => {}
            }
        });
        lane.cars.push(Car {
            parent_hash,
            txs: txs_without_proofs,
        });

        let msg = MempoolNetMessage::DataVote(data_proposal.car.hash());
        let self_signed_msg = crypto.sign(msg.clone()).expect("signed message");
        lane.poa.insert(self.id.clone(), self_signed_msg);
        lane.poa.insert(
            validator.clone(),
            SignedByValidator {
                msg,
                signature: data_proposal.data_vote_signature.clone(),
            },
        );
    }

    fn other_lane_has_parent_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> bool {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        data_proposal.car.parent_hash == lane.current_hash()
    }

    fn other_lane_update_parent_poa(
        &mut self,
        validator: &ValidatorPublicKey,
        parent_hash: &CarHash,
        parent_poa: &BTreeMap<ValidatorPublicKey, SignedByValidator<MempoolNetMessage>>,
    ) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        if let Some(car) = lane.current_mut() {
            if &car.hash() == parent_hash {
                lane.poa.extend(parent_poa.clone());
            }
        }
    }

    fn other_lane_has_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> bool {
        self.other_lanes
            .entry(validator.clone())
            .or_default()
            .cars
            .iter()
            .any(|car| car.hash() == data_proposal.car.hash())
    }

    pub fn get_missing_cars(
        &self,
        last_car_hash: Option<CarHash>,
        data_proposal_tip_hash: &CarHash,
    ) -> Option<Vec<Car>> {
        self.lane
            .get_missing_cars(last_car_hash, data_proposal_tip_hash)
    }

    pub fn get_waiting_data_proposals(
        &mut self,
        validator: &ValidatorPublicKey,
    ) -> Result<Vec<DataProposal>> {
        match self.other_lanes.get_mut(validator) {
            Some(lane) => lane.get_waiting_data_proposals(),
            None => Ok(vec![]),
        }
    }

    // Updates local view other lane matching the validator with sent cars
    pub fn other_lane_add_missing_cars(
        &mut self,
        validator: &ValidatorPublicKey,
        cars: Vec<Car>,
    ) -> Result<()> {
        let lane = self.other_lanes.entry(validator.clone()).or_default();

        debug!(
            "Trying to add missing cars on lane \n {}\n {:?}",
            lane, cars
        );

        lane.add_missing_cars(cars)
    }

    /// Received a new transaction when the previous DataProposal had no PoA yet
    pub fn on_new_tx(&mut self, tx: Transaction) {
        self.pending_txs.push(tx);
    }

    pub fn new_data_proposal(&mut self, crypto: Arc<BlstCrypto>) -> Option<DataProposal> {
        if self.pending_txs.is_empty() || self.data_proposal.is_some() {
            return None;
        }
        let car = Car {
            parent_hash: self.lane.current_hash(),
            txs: std::mem::take(&mut self.pending_txs),
        };
        let msg = MempoolNetMessage::DataVote(car.hash());
        let signed_msg = crypto.sign(msg).expect("signed message");
        self.data_proposal.replace(DataProposal {
            car,
            // The very first time we create a DataProposal, lane.poa.len() will be 0 so this means we have no parent Poa.
            // After that lane.poa will contain the parent Poa.
            parent_poa: if self.lane.poa.len() == 0 {
                None
            } else {
                Some(std::mem::take(&mut self.lane.poa).0)
            },
            data_vote_signature: signed_msg.signature.clone(),
        });

        self.lane.poa.insert(self.id.clone(), signed_msg);
        self.data_proposal.clone()
    }

    pub fn collect_lanes(&mut self, lanes: Cut) -> Vec<Transaction> {
        let mut txs = Vec::new();
        for (validator, _, car_hash) in lanes.iter() {
            if validator == &self.id {
                self.lane.collect_cars(car_hash, &mut txs);
            } else if let Some(lane) = self.other_lanes.get_mut(validator) {
                lane.collect_cars(car_hash, &mut txs);
            } else {
                // can happen if validator does not have any DataProposal yet
                debug!(
                    "Validator {} not found in lane of {} (updating)",
                    validator, self.id
                );
            }
        }
        txs
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct DataProposal {
    pub car: Car,
    pub parent_poa: Option<BTreeMap<ValidatorPublicKey, SignedByValidator<MempoolNetMessage>>>,
    pub data_vote_signature: ValidatorSignature,
}

impl Display for DataProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.car.hash())
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialOrd,
    Ord,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Encode,
    Decode,
    Hash,
)]
pub struct CarHash(pub String);

impl Display for CarHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Car {
    pub parent_hash: Option<CarHash>,
    pub txs: Vec<Transaction>,
}

impl Hashable<CarHash> for Car {
    fn hash(&self) -> CarHash {
        let mut hasher = Sha3_256::new();
        if let Some(parent_hash) = &self.parent_hash {
            _ = write!(hasher, "{}", parent_hash);
        }
        for tx in self.txs.iter() {
            hasher.update(tx.hash().0);
        }
        CarHash(hex::encode(hasher.finalize()))
    }
}

impl Display for Car {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hash())
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Poa(pub BTreeMap<ValidatorPublicKey, SignedByValidator<MempoolNetMessage>>);

impl std::ops::Deref for Poa {
    type Target = BTreeMap<ValidatorPublicKey, SignedByValidator<MempoolNetMessage>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Poa {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Default, Debug, Clone)]
pub struct Lane {
    cars: Vec<Car>,
    pub poa: Poa,
    waiting: Vec<DataProposal>,
}

impl Display for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for car in self.cars.iter() {
            match &car.parent_hash {
                None => {
                    let _ = write!(f, "{}", car);
                }
                Some(_car_hash) => {
                    let _ = write!(f, " <- {}", car);
                }
            }
        }

        Ok(())
    }
}

impl Lane {
    pub fn current_mut(&mut self) -> Option<&mut Car> {
        self.cars.last_mut()
    }

    pub fn current(&self) -> Option<&Car> {
        self.cars.last()
    }

    pub fn current_hash(&self) -> Option<CarHash> {
        self.current().map(|car| car.hash())
    }

    fn dedup_push_txs(txs: &mut Vec<Transaction>, car_txs: Vec<Transaction>) {
        for tx in car_txs.into_iter() {
            if !txs.contains(&tx) {
                txs.push(tx);
            }
        }
    }

    fn add_current_hash_to_cut(
        &self,
        _crypto: &Arc<BlstCrypto>,
        cut: &mut Cut,
        validator: &ValidatorPublicKey,
    ) -> Result<()> {
        if let Some(car) = self.current() {
            // FIXME: Failed to aggregate signatures into valid one. Messages might be different
            // let aggregates: Vec<&SignedByValidator<MempoolNetMessage>> =
            //     self.poa.values().collect();
            // let signed_aggregate =
            //     crypto.sign_aggregate(MempoolNetMessage::DataVote(car.hash()), &aggregates)?;
            cut.push((
                validator.clone(),
                None,
                // Some(signed_aggregate.signature),
                car.hash(),
            ));
        }
        Ok(())
    }

    fn collect_cars(&mut self, car_hash: &CarHash, txs: &mut Vec<Transaction>) {
        if let Some(pos) = self.cars.iter().position(|car| &car.hash() == car_hash) {
            let latest_txs = std::mem::take(&mut self.cars[pos].txs);
            // collect all self.cars but the last. we need it for future cuts.
            self.cars.drain(..pos).for_each(|car| {
                Self::dedup_push_txs(txs, car.txs);
            });
            Self::dedup_push_txs(txs, latest_txs);
        } else {
            error!("Car {:?} not found !", car_hash);
        }
    }

    fn get_missing_cars(
        &self,
        last_car_hash: Option<CarHash>,
        data_proposal_tip_hash: &CarHash,
    ) -> Option<Vec<Car>> {
        let data_proposal_car = self
            .cars
            .iter()
            .find(|car| &car.hash() == data_proposal_tip_hash);

        match data_proposal_car {
            None => {
                error!("DataProposal does not exist locally");
                None
            }
            Some(data_proposal_car) => {
                //Normally last_car_id must be < current_car since we are on the lane reference (that must have more Cars than others)
                match last_car_hash {
                    // Nothing on the lane, we send everything, up to the DataProposal id/pos
                    None => Some(
                        self.cars
                            .iter()
                            .take_while(|car| &car.hash() != data_proposal_tip_hash)
                            .cloned()
                            .collect(),
                    ),
                    // If there is an index, two cases
                    // - it matches the current tip, in this case we don't send any more Cars
                    // - it does not match, we send the diff
                    Some(last_car_hash) => {
                        if last_car_hash == data_proposal_car.hash() {
                            None
                        } else {
                            Some(
                                self.cars
                                    .iter()
                                    .skip_while(|car| car.hash() != last_car_hash)
                                    .skip(1)
                                    .take_while(|car| car.hash() != data_proposal_car.hash())
                                    .cloned()
                                    .collect(),
                            )
                        }
                    }
                }
            }
        }
    }

    pub fn add_missing_cars(&mut self, cars: Vec<Car>) -> Result<()> {
        let mut ordered_cars = cars;
        ordered_cars.dedup();

        for car in ordered_cars.into_iter() {
            if car.parent_hash != self.current_hash() {
                bail!("Hash mismatch while adding missing Cars");
            }
            self.cars.push(car);
        }
        Ok(())
    }

    fn get_waiting_data_proposals(&mut self) -> Result<Vec<DataProposal>> {
        let wp = self.waiting.drain(0..).collect::<Vec<_>>();
        if wp.len() > 1 {
            for i in 0..wp.len() - 1 {
                if Some(&wp[i].car.hash()) != wp[i + 1].car.parent_hash.as_ref() {
                    bail!("unsorted DataProposal");
                }
            }
        }
        Ok(wp)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use crate::{
        mempool::{
            storage::{Car, DataProposal, DataProposalVerdict, Storage},
            MempoolNetMessage,
        },
        model::{
            Blob, BlobData, BlobReference, BlobTransaction, ContractName, Hashable, ProofData,
            ProofTransaction, RegisterContractTransaction, Transaction, TransactionData,
            ValidatorPublicKey, VerifiedProofTransaction,
        },
        node_state::NodeState,
        p2p::network::SignedByValidator,
        utils::crypto::BlstCrypto,
    };
    use hyle_contract_sdk::{BlobIndex, HyleOutput, Identity, StateDigest, TxHash};

    use super::{CarHash, Cut, Lane};

    fn make_unverified_proof_tx() -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::Proof(ProofTransaction {
                blobs_references: vec![],
                proof: ProofData::default(),
            }),
        }
    }

    fn make_verified_proof_tx(contract_name: ContractName) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::VerifiedProof(VerifiedProofTransaction {
                proof_transaction: ProofTransaction {
                    blobs_references: vec![BlobReference {
                        contract_name,
                        blob_tx_hash: TxHash("".to_owned()),
                        blob_index: BlobIndex(0),
                    }],
                    proof: ProofData::default(),
                },
                hyle_outputs: vec![HyleOutput {
                    version: 1,
                    initial_state: StateDigest(vec![0, 1, 2, 3]),
                    next_state: StateDigest(vec![4, 5, 6]),
                    identity: Identity("test.c1".to_string()),
                    tx_hash: TxHash("".to_owned()),
                    index: BlobIndex(0),
                    blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
                    success: true,
                    program_outputs: vec![],
                }],
            }),
        }
    }

    fn make_blob_tx(inner_tx: &'static str) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::Blob(BlobTransaction {
                identity: Identity("id".to_string()),
                blobs: vec![Blob {
                    contract_name: ContractName("c1".to_string()),
                    data: BlobData(inner_tx.as_bytes().to_vec()),
                }],
            }),
        }
    }

    fn make_register_contract_tx(name: ContractName) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::RegisterContract(RegisterContractTransaction {
                owner: "test".to_string(),
                verifier: "test".to_string(),
                program_id: vec![],
                state_digest: StateDigest(vec![0, 1, 2, 3]),
                contract_name: name,
            }),
        }
    }

    struct TestCtx {
        pub crypto: Arc<BlstCrypto>,
        pub store: Storage,
        pub state: NodeState,
    }

    impl TestCtx {
        fn new(name: &'static str) -> Self {
            let crypto = Arc::new(BlstCrypto::new(name.into()));
            let pubkey = crypto.validator_pubkey().clone();
            let store = Storage::new(pubkey);
            let state = NodeState::default();
            Self {
                crypto,
                store,
                state,
            }
        }

        fn pubkey(&self) -> &ValidatorPublicKey {
            self.crypto.validator_pubkey()
        }

        fn sign_data_vote(&self, car_hash: CarHash) -> SignedByValidator<MempoolNetMessage> {
            let msg = MempoolNetMessage::DataVote(car_hash);
            self.crypto.sign(msg).expect("signed message")
        }

        fn other_lane_add_data_proposal(&mut self, from: &TestCtx, data_proposal: &DataProposal) {
            let crypto = Arc::clone(&self.crypto);
            self.store
                .other_lane_add_data_proposal(crypto, from.pubkey(), data_proposal);
        }

        fn new_blob_tx(&mut self, tx_data: &'static str) {
            let old_pending_txs = self.store.pending_txs.len();
            self.store.on_new_tx(make_blob_tx(tx_data));
            let new_pending_txs = self.store.pending_txs.len();
            assert_eq!(old_pending_txs + 1, new_pending_txs);
        }

        fn new_data_proposal(&mut self) -> DataProposal {
            let crypto = Arc::clone(&self.crypto);
            self.store
                .new_data_proposal(crypto)
                .expect("a DataProposal")
        }

        fn data_proposal_from(
            &mut self,
            from: &TestCtx,
            data_proposal: &DataProposal,
        ) -> DataProposalVerdict {
            let crypto = Arc::clone(&self.crypto);
            self.store
                .on_data_proposal(crypto, from.pubkey(), data_proposal, &self.state)
        }

        fn make_data_proposal(
            &self,
            car: Car,
            parent_poa: Option<BTreeMap<ValidatorPublicKey, SignedByValidator<MempoolNetMessage>>>,
        ) -> DataProposal {
            let data_vote_signature = self.sign_data_vote(car.hash()).signature;
            DataProposal {
                car,
                parent_poa,
                data_vote_signature,
            }
        }

        fn data_vote_from(&mut self, from: &TestCtx, data_proposal: &DataProposal) {
            let signed_msg = from.sign_data_vote(data_proposal.car.hash());
            self.store
                .on_data_vote(from.pubkey(), data_proposal.car.hash(), signed_msg)
                .expect("a successful vote");
        }

        fn assert_can_vote_for(&mut self, from: &TestCtx, data_proposal: &DataProposal) {
            assert_eq!(
                self.data_proposal_from(from, data_proposal),
                DataProposalVerdict::Vote
            );
        }

        fn assert_already_vote_for(&mut self, from: &TestCtx, data_proposal: &DataProposal) {
            assert_eq!(
                self.data_proposal_from(from, data_proposal),
                DataProposalVerdict::DidVote
            );
        }

        fn commit_data_proposal(&mut self) {
            self.store.commit_data_proposal();
            assert!(self.store.data_proposal.is_none());
        }

        fn lane_of(&self, of: &TestCtx) -> &Lane {
            self.store.other_lanes.get(of.pubkey()).expect("a Lane")
        }

        fn lane_of_mut(&mut self, of: &TestCtx) -> &mut Lane {
            self.store.other_lanes.get_mut(of.pubkey()).expect("a Lane")
        }

        fn current_hash_of(&self, of: &TestCtx) -> CarHash {
            self.lane_of(of).current_hash().expect("a CarHash")
        }

        #[track_caller]
        fn new_cut(&mut self, validators: &[ValidatorPublicKey]) -> Cut {
            self.store
                .new_cut(Arc::clone(&self.crypto), validators)
                .expect("a Cut")
        }
    }

    #[test_log::test]
    fn test_workflow() {
        let mut ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        ctx2.new_blob_tx("test1");
        let data_proposal1 = ctx2.new_data_proposal();
        ctx3.assert_can_vote_for(&ctx2, &data_proposal1);
        ctx2.data_vote_from(&ctx3, &data_proposal1);
        ctx2.commit_data_proposal();

        ctx2.new_blob_tx("test2");
        let data_proposal2 = ctx2.new_data_proposal();
        ctx3.assert_can_vote_for(&ctx2, &data_proposal2);
        ctx2.data_vote_from(&ctx3, &data_proposal2);
        ctx2.commit_data_proposal();

        ctx2.new_blob_tx("test3");
        let data_proposal3 = ctx2.new_data_proposal();
        ctx3.assert_can_vote_for(&ctx2, &data_proposal3);
        ctx2.data_vote_from(&ctx3, &data_proposal3);
        ctx2.commit_data_proposal();

        ctx2.new_blob_tx("test4");
        let data_proposal4 = ctx2.new_data_proposal();
        ctx3.assert_can_vote_for(&ctx2, &data_proposal4);
        ctx2.data_vote_from(&ctx3, &data_proposal4);
        ctx2.commit_data_proposal();

        assert_eq!(ctx3.store.other_lanes.len(), 1);
        assert!(ctx3.store.other_lanes.contains_key(ctx2.pubkey()));

        let lane2 = ctx3.lane_of(&ctx2);
        let first_car = lane2.cars.first().expect("first car");
        assert_eq!(first_car.parent_hash, None);
        assert_eq!(first_car.txs, vec![make_blob_tx("test1")]);
        assert!(lane2.poa.contains_key(ctx2.pubkey()));
        assert!(lane2.poa.contains_key(ctx3.pubkey()));

        let missing = ctx3.store.get_missing_cars(
            Some(first_car.hash()),
            &ctx3
                .store
                .other_lanes
                .get(ctx2.pubkey())
                .and_then(|lane| lane.current_hash())
                .unwrap_or_default(),
        );

        assert_eq!(missing, None);
    }

    #[test_log::test]
    fn test_vote() {
        let ctx1 = TestCtx::new("1");
        let mut ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        ctx3.new_blob_tx("test1");
        ctx3.new_blob_tx("test2");
        ctx3.new_blob_tx("test3");
        ctx3.new_blob_tx("test4");

        let data_proposal = ctx3.new_data_proposal();
        assert_eq!(ctx3.store.lane.poa.len(), 1);

        ctx2.assert_can_vote_for(&ctx3, &data_proposal);
        assert_eq!(
            ctx2.current_hash_of(&ctx3),
            CarHash("84ffbeea5a3d9fe1e1f64b6af44447a5d42b2ba938e8b2f00e84161990d7a2a0".to_string())
        );
        ctx2.assert_already_vote_for(&ctx3, &data_proposal);

        ctx3.data_vote_from(&ctx2, &data_proposal);
        assert!(ctx3.store.data_proposal.is_some());
        assert_eq!(ctx3.store.lane.poa.len(), 2);

        ctx3.data_vote_from(&ctx1, &data_proposal);
        ctx3.commit_data_proposal();
        assert_eq!(ctx3.store.lane.poa.len(), 3);

        assert!(ctx3.store.lane.poa.contains_key(ctx3.pubkey()));
        assert!(ctx3.store.lane.poa.contains_key(ctx1.pubkey()));
        assert!(ctx3.store.lane.poa.contains_key(ctx2.pubkey()));
    }

    #[test_log::test]
    fn test_update_lane_with_unverified_proof_transaction() {
        let ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_unverified_proof_tx();

        let data_proposal = ctx2.make_data_proposal(
            Car {
                parent_hash: None,
                txs: vec![register_tx, proof_tx],
            },
            None,
        );

        assert_eq!(
            ctx3.data_proposal_from(&ctx2, &data_proposal),
            DataProposalVerdict::Refuse
        );

        // Ensure the lane was not updated with the unverified proof transaction
        assert!(!ctx3
            .store
            .other_lane_has_data_proposal(ctx2.pubkey(), &data_proposal));
    }

    #[test_log::test]
    fn test_update_lane_with_verified_proof_transaction() {
        let ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal = ctx2.make_data_proposal(
            Car {
                parent_hash: None,
                txs: vec![proof_tx.clone()],
            },
            None,
        );

        assert_eq!(
            ctx3.data_proposal_from(&ctx2, &data_proposal),
            DataProposalVerdict::Refuse
        );

        let data_proposal = ctx2.make_data_proposal(
            Car {
                parent_hash: None,
                txs: vec![register_tx, proof_tx],
            },
            None,
        );

        ctx3.assert_can_vote_for(&ctx2, &data_proposal);
    }

    #[test_log::test]
    fn test_new_data_proposal_with_register_tx_in_previous_uncommitted_car() {
        let ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal1 = ctx2.make_data_proposal(
            Car {
                parent_hash: None,
                txs: vec![register_tx],
            },
            None,
        );
        ctx3.other_lane_add_data_proposal(&ctx2, &data_proposal1);

        let data_proposal = ctx2.make_data_proposal(
            Car {
                parent_hash: Some(data_proposal1.car.hash()),
                txs: vec![proof_tx.clone()],
            },
            Some(BTreeMap::from([
                (
                    ctx3.pubkey().clone(),
                    ctx3.sign_data_vote(data_proposal1.car.hash()),
                ),
                (
                    ctx2.pubkey().clone(),
                    ctx2.sign_data_vote(data_proposal1.car.hash()),
                ),
            ])),
        );

        ctx3.assert_can_vote_for(&ctx2, &data_proposal);

        // Ensure the lane was updated with the DataProposal
        assert!(ctx3
            .store
            .other_lane_has_data_proposal(ctx2.pubkey(), &data_proposal));
    }

    #[test_log::test]
    fn test_register_contract_and_proof_tx_in_same_car() {
        let ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());
        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal = ctx2.make_data_proposal(
            Car {
                parent_hash: None,
                txs: vec![register_tx, proof_tx],
            },
            None,
        );

        ctx3.assert_can_vote_for(&ctx2, &data_proposal);

        // Ensure the lane was updated with the DataProposal
        assert!(ctx3
            .store
            .other_lane_has_data_proposal(ctx2.pubkey(), &data_proposal));
    }

    #[test_log::test]
    fn test_register_contract_and_proof_tx_in_same_car_wrong_order() {
        let ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());
        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal = ctx2.make_data_proposal(
            Car {
                parent_hash: None,
                txs: vec![proof_tx, register_tx],
            },
            None,
        );
        assert_eq!(
            ctx3.data_proposal_from(&ctx2, &data_proposal),
            DataProposalVerdict::Refuse
        );

        // Ensure the lane was not updated with the DataProposal
        assert!(!ctx3
            .store
            .other_lane_has_data_proposal(ctx2.pubkey(), &data_proposal));
    }

    #[test_log::test]
    fn test_update_lanes_after_commit() {
        let mut ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        ctx3.new_blob_tx("test1");
        let data_proposal1 = ctx3.new_data_proposal();
        ctx2.assert_can_vote_for(&ctx3, &data_proposal1);
        ctx3.data_vote_from(&ctx2, &data_proposal1);
        ctx3.commit_data_proposal();

        ctx3.new_blob_tx("test2");
        let data_proposal2 = ctx3.new_data_proposal();
        ctx2.assert_can_vote_for(&ctx3, &data_proposal2);
        ctx3.data_vote_from(&ctx2, &data_proposal2);
        ctx3.commit_data_proposal();

        ctx2.new_blob_tx("test3");
        let data_proposal3 = ctx2.new_data_proposal();
        ctx3.assert_can_vote_for(&ctx2, &data_proposal3);
        ctx2.data_vote_from(&ctx3, &data_proposal3);
        ctx2.commit_data_proposal();

        assert_eq!(ctx3.store.lane.cars.len(), 2);
        assert_eq!(ctx3.lane_of(&ctx2).cars.len(), 1);

        let cut = ctx3.new_cut(&[ctx3.pubkey().clone(), ctx2.pubkey().clone()]);

        assert_eq!(ctx3.store.collect_lanes(cut.clone()).len(), 3);
        assert_eq!(ctx2.store.collect_lanes(cut).len(), 3);

        // should contain only the tip on all the lanes
        assert_eq!(ctx3.store.lane.cars.len(), 1);
        assert_eq!(ctx2.store.lane.cars.len(), 1);
        assert_eq!(ctx3.lane_of(&ctx2).cars.len(), 1);
        assert_eq!(ctx2.lane_of(&ctx3).cars.len(), 1);
    }

    #[test]
    fn test_add_missing_cars_order_check() {
        let ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        let car1 = Car {
            parent_hash: None,
            txs: vec![make_blob_tx("test1")],
        };
        let car2 = Car {
            parent_hash: Some(car1.hash()),
            txs: vec![make_blob_tx("test2")],
        };
        let car3 = Car {
            parent_hash: Some(car2.hash()),
            txs: vec![make_blob_tx("test3")],
        };
        let car4 = Car {
            parent_hash: Some(car3.hash()),
            txs: vec![make_blob_tx("test4")],
        };

        ctx3.store
            .other_lane_add_missing_cars(
                ctx2.pubkey(),
                vec![car1.clone(), car2.clone(), car3.clone(), car4.clone()],
            )
            .expect("to add missing cars");

        std::mem::take(&mut ctx3.lane_of_mut(&ctx2).cars);

        // car1 repetition should not be a problem
        ctx3.store
            .other_lane_add_missing_cars(
                ctx2.pubkey(),
                vec![
                    car1.clone(),
                    car1.clone(),
                    car2.clone(),
                    car3.clone(),
                    car4.clone(),
                ],
            )
            .expect("to add missing cars");

        std::mem::take(&mut ctx3.lane_of_mut(&ctx2).cars);

        // unordered cars should fail
        assert!(ctx3
            .store
            .other_lane_add_missing_cars(
                ctx2.pubkey(),
                vec![car4.clone(), car1.clone(), car2.clone(), car3.clone()],
            )
            .is_err());

        std::mem::take(&mut ctx3.lane_of_mut(&ctx2).cars);

        let car_bad = Car {
            parent_hash: Some(car3.hash()),
            txs: vec![make_blob_tx("testX")],
        };

        // car_bad is a fork on car3
        assert!(ctx3
            .store
            .other_lane_add_missing_cars(
                ctx2.pubkey(),
                vec![
                    car4.clone(),
                    car2.clone(),
                    car_bad,
                    car1.clone(),
                    car3.clone(),
                ],
            )
            .is_err());

        std::mem::take(&mut ctx3.lane_of_mut(&ctx2).cars);

        // car5 will not be part of the missing cars
        let car5 = Car {
            parent_hash: Some(car4.hash()),
            txs: vec![make_blob_tx("test5")],
        };
        let car6 = Car {
            parent_hash: Some(car5.hash()),
            txs: vec![make_blob_tx("test6")],
        };
        assert!(ctx3
            .store
            .other_lane_add_missing_cars(ctx2.pubkey(), vec![car4, car2, car6, car1, car3])
            .is_err());

        std::mem::take(&mut ctx3.lane_of_mut(&ctx2).cars);
    }

    #[test_log::test]
    fn test_add_missing_cars() {
        let ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        let car1 = Car {
            parent_hash: None,
            txs: vec![make_blob_tx("test1")],
        };
        ctx3.store
            .other_lane_add_missing_cars(
                ctx2.pubkey(),
                vec![
                    car1.clone(),
                    Car {
                        parent_hash: Some(car1.hash()),
                        txs: vec![make_blob_tx("test2")],
                    },
                ],
            )
            .expect("add missing cars");

        assert_eq!(ctx3.lane_of(&ctx2).cars.len(), 2);
    }

    #[test_log::test]
    fn test_missing_cars() {
        let ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        ctx3.new_blob_tx("test_local1");
        let data_proposal = ctx3.new_data_proposal();
        ctx3.data_vote_from(&ctx2, &data_proposal);
        ctx3.commit_data_proposal();

        let last_car_hash = ctx3.store.lane.current_hash();

        ctx3.new_blob_tx("test_local2");
        let data_proposal = ctx3.new_data_proposal();
        ctx3.data_vote_from(&ctx2, &data_proposal);
        ctx3.commit_data_proposal();

        ctx3.new_blob_tx("test_local3");
        let data_proposal = ctx3.new_data_proposal();
        ctx3.data_vote_from(&ctx2, &data_proposal);
        ctx3.commit_data_proposal();

        ctx3.new_blob_tx("test_local4");
        let data_proposal = ctx3.new_data_proposal();
        ctx3.data_vote_from(&ctx2, &data_proposal);
        ctx3.commit_data_proposal();

        assert_eq!(ctx3.store.lane.cars.len(), 4);

        let missing = ctx3
            .store
            .get_missing_cars(last_car_hash, &data_proposal.car.hash())
            .expect("missing cars");

        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0].txs[0], make_blob_tx("test_local2"));
        assert_eq!(missing[1].txs[0], make_blob_tx("test_local3"));

        let missing = ctx3
            .store
            .get_missing_cars(None, &data_proposal.car.hash())
            .expect("missing cars");

        assert_eq!(missing.len(), 3);
        assert_eq!(missing[0].txs[0], make_blob_tx("test_local1"));
        assert_eq!(missing[1].txs[0], make_blob_tx("test_local2"));
        assert_eq!(missing[2].txs[0], make_blob_tx("test_local3"));
    }

    #[test]
    fn test_lane_workflow() {
        let mut ctx2 = TestCtx::new("2");
        let mut ctx3 = TestCtx::new("3");

        ctx3.new_blob_tx("tx 1");
        let data_proposal3 = ctx3.new_data_proposal();

        ctx2.new_blob_tx("tx 2");
        let data_proposal2 = ctx2.new_data_proposal();

        ctx3.assert_can_vote_for(&ctx2, &data_proposal2);
        ctx2.assert_can_vote_for(&ctx3, &data_proposal3);

        ctx3.data_vote_from(&ctx2, &data_proposal3);
        ctx2.data_vote_from(&ctx3, &data_proposal2);

        ctx2.commit_data_proposal();
        ctx3.commit_data_proposal();

        let cut3 = ctx3.new_cut(&[ctx3.pubkey().clone(), ctx2.pubkey().clone()]);
        assert_eq!(cut3.len(), 2);
        let cut2 = ctx2.new_cut(&[ctx3.pubkey().clone(), ctx2.pubkey().clone()]);
        assert_eq!(cut2.len(), 2);

        let txs3 = ctx3.store.collect_lanes(cut3.clone());
        assert_eq!(txs3.len(), 2);
        let txs2 = ctx2.store.collect_lanes(cut3.clone());
        assert_eq!(txs2.len(), 2);
    }
}
