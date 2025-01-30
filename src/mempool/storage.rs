use anyhow::{bail, Context, Result};
use bincode::{Decode, Encode};
use hyle_model::{
    ContractName, DataSized, ProgramId, RegisterContractAction, Signed, StructuredBlobData,
    ValidatorSignature, Verifier,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use staking::state::Staking;
use std::{collections::HashMap, fmt::Display, sync::Arc, vec};
use tracing::{debug, error, warn};

use crate::{
    model::{
        BlobProofOutput, Cut, DataProposal, DataProposalHash, Hashable, PoDA, SignedByValidator,
        Transaction, TransactionData, ValidatorPublicKey,
    },
    utils::crypto::BlstCrypto,
};

use super::verifiers::{verify_proof, verify_recursive_proof};
use super::{KnownContracts, MempoolNetMessage};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataProposalVerdict {
    Empty,
    Wait(Option<DataProposalHash>),
    Vote,
    Process,
    Refuse,
}

#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneEntry {
    pub data_proposal: DataProposal,
    pub cumul_size: LaneBytesSize,
    pub signatures: Vec<SignedByValidator<MempoolNetMessage>>,
}

#[derive(Debug, Default, Clone, Encode, Decode)]
pub struct Lane {
    pub last_cut: Option<(PoDA, DataProposalHash)>,
    #[bincode(with_serde)]
    pub data_proposals: IndexMap<DataProposalHash, LaneEntry>,
}

pub use hyle_model::LaneBytesSize;

#[derive(Debug, Default, Clone, Encode, Decode)]
pub struct Storage {
    pub id: ValidatorPublicKey,
    pub lanes: HashMap<ValidatorPublicKey, Lane>,
    pub lanes_tip: HashMap<ValidatorPublicKey, DataProposalHash>,
}

impl Display for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Replica {}", self.id)?;
        for (i, lane) in self.lanes.iter() {
            write!(f, "\n - OL {}: {}", i, lane)?;
        }

        Ok(())
    }
}

impl Storage {
    pub fn new(
        id: ValidatorPublicKey,
        lanes_tip: HashMap<ValidatorPublicKey, DataProposalHash>,
    ) -> Storage {
        let mut lanes = HashMap::new();
        lanes.insert(id.clone(), Lane::default());
        Storage {
            id,
            lanes,
            lanes_tip,
        }
    }

    pub fn new_cut(&mut self, staking: &Staking) -> Cut {
        // For each validator, we get the last validated car and put it in the cut
        let mut cut: Cut = vec![];
        let bonded_validators = staking.bonded();
        for validator in bonded_validators.iter() {
            // Get lane of the validator. Create a new empty one is it does not exist
            let lane = self.lanes.entry(validator.clone()).or_default();

            // Iterate their lane starting from the most recent DataProposal until we find one with enough signatures
            for (
                data_proposal_hash,
                LaneEntry {
                    data_proposal: _,
                    cumul_size: size,
                    signatures,
                },
            ) in lane.iter_reverse()
            {
                // Only cut on DataProposal that have not been cut yet
                if lane.last_cut.as_ref().map(|lc| &lc.1) == Some(data_proposal_hash) {
                    #[allow(clippy::unwrap_used, reason = "we know the value is Some")]
                    let poda = lane.last_cut.as_ref().map(|lc| lc.0.clone()).unwrap();
                    cut.push((validator.clone(), data_proposal_hash.clone(), *size, poda));
                    break;
                }
                // Filter signatures on DataProposal to only keep the ones from the current validators
                let filtered_signatures: Vec<SignedByValidator<MempoolNetMessage>> = signatures
                    .iter()
                    .filter(|signed_msg| {
                        bonded_validators.contains(&signed_msg.signature.validator)
                    })
                    .cloned()
                    .collect();

                // Collect all filtered validators that signed the DataProposal
                let filtered_validators: Vec<ValidatorPublicKey> = filtered_signatures
                    .iter()
                    .map(|s| s.signature.validator.clone())
                    .collect();

                // Compute their voting power to check if the DataProposal received enough votes
                let voting_power = staking.compute_voting_power(filtered_validators.as_slice());
                let f = staking.compute_f();
                if voting_power < f + 1 {
                    // Check if previous DataProposals received enough votes
                    continue;
                }

                // Aggregate the signatures in a PoDA
                let poda = match BlstCrypto::aggregate(
                    MempoolNetMessage::DataVote(data_proposal_hash.clone(), *size),
                    &filtered_signatures.iter().collect::<Vec<_>>(),
                ) {
                    Ok(poda) => poda,
                    Err(e) => {
                        error!(
                                "Could not aggregate signatures for validator {} and data proposal hash {}: {}",
                                validator, data_proposal_hash, e
                            );
                        break;
                    }
                };

                // Add the DataProposal to the cut for this validator
                cut.push((
                    validator.clone(),
                    data_proposal_hash.clone(),
                    *size,
                    poda.signature,
                ));
                break;
            }
        }

        cut
    }

    // Called by the initial proposal validator to aggregate votes
    pub fn on_data_vote(
        &mut self,
        msg: &SignedByValidator<MempoolNetMessage>,
        data_proposal_hash: &DataProposalHash,
        new_lane_size: LaneBytesSize,
    ) -> Result<(
        DataProposalHash,
        Vec<Signed<MempoolNetMessage, ValidatorSignature>>,
    )> {
        let lane = match self.lanes.get_mut(&self.id) {
            Some(lane) => lane,
            None => bail!(
                "Received vote from unkown validator {}",
                msg.signature.validator,
            ),
        };
        match lane.get_proposal_mut(data_proposal_hash) {
            Some(lane_entry) => {
                if lane_entry.cumul_size != new_lane_size {
                    debug!("Received size {:?} does no match the actual size of the DataProposal ({:?})", new_lane_size, lane_entry.cumul_size);
                    bail!("Received size {new_lane_size} does no match the actual size of the DataProposal ({})", lane_entry.cumul_size);
                }
                // Adding the vote to the DataProposal
                lane_entry.signatures.push(msg.clone());
                lane_entry.signatures.dedup();
                Ok((data_proposal_hash.clone(), lane_entry.signatures.clone()))
            }
            None => {
                bail!("Received vote from validator {}  for unknown DataProposal ({data_proposal_hash})", msg.signature.validator);
            }
        }
    }

    pub fn on_poda_update(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal_hash: &DataProposalHash,
        signatures: Vec<SignedByValidator<MempoolNetMessage>>,
    ) {
        if let Some(data_proposal) = self
            .lanes
            .entry(validator.clone())
            .or_default()
            .get_proposal_mut(data_proposal_hash)
        {
            // Adding the votes to the DataProposal
            data_proposal.signatures.extend(signatures);
            data_proposal.signatures.dedup();
        } else {
            warn!("Received PoDA update from validator {validator} for unknown DataProposal ({data_proposal_hash}). Not storing it");
        }
    }

    pub fn on_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> (DataProposalVerdict, Option<LaneBytesSize>) {
        // Check that data_proposal is not empty
        if data_proposal.txs.is_empty() {
            return (DataProposalVerdict::Empty, None);
        }

        let validator_lane = self.lanes.entry(validator.clone()).or_default();

        let last_known_id = validator_lane.get_last_proposal_id().unwrap_or(&0);
        let last_known_hash = validator_lane.get_last_proposal_hash();
        let validator_lane_size = validator_lane.get_lane_size();

        let local_parent = data_proposal
            .parent_data_proposal_hash
            .as_ref()
            .and_then(|hash| validator_lane.data_proposals.get(hash));

        let local_parent_data_proposal_hash =
            local_parent.map(|lane_entry| lane_entry.data_proposal.hash());

        let local_parent_data_proposal_id = local_parent
            .map(|lane_entry| lane_entry.data_proposal.id)
            .unwrap_or(0);

        // LEGIT DATA PROPOSAL

        // id == 0 + no registered data proposals = first data proposal to be processed
        // id == last + 1 and the parent is present locally means no fork
        let first_data_proposal = data_proposal.id == 0 && validator_lane.data_proposals.is_empty();

        let valid_next_data_proposal = data_proposal.id == last_known_id + 1
            && data_proposal.parent_data_proposal_hash == local_parent_data_proposal_hash
            && local_parent_data_proposal_id + 1 == data_proposal.id;

        if first_data_proposal || valid_next_data_proposal {
            return (DataProposalVerdict::Process, None);
        }

        // FORKS

        // Find local data proposal that has the same parent as the received data proposal
        let data_proposal_already_on_top_of_parent =
            validator_lane.data_proposals.iter().find(|dp| {
                dp.1.data_proposal.parent_data_proposal_hash
                    == data_proposal.parent_data_proposal_hash
            });
        // A fork happens when the received data proposal points
        // [id: 2] abc <- [id: 3] fgh
        //             <- [id: 3] ijkl

        // A different data proposal is already referring to the parent
        if let Some(p) = data_proposal_already_on_top_of_parent {
            if &p.1.data_proposal != data_proposal {
                // Means a later data proposal is already on top of the parent
                warn!("Fork detected on lane {:?}", validator);
                return (DataProposalVerdict::Refuse, None);
            }
        }

        // ALREADY STORED

        // Already present data proposal (just resend a vote)
        if last_known_hash == Some(&data_proposal.hash()) {
            return (DataProposalVerdict::Vote, Some(validator_lane_size));
        }

        // Missing data, let's sync before taking a decision
        if local_parent_data_proposal_hash.is_none() {
            // Get the last known parent hash in order to get all the next ones
            return (DataProposalVerdict::Wait(last_known_hash.cloned()), None);
        }

        error!(
            "DataProposal cannot be handled: {:?}",
            data_proposal.clone()
        );

        (DataProposalVerdict::Refuse, None)
    }

    pub fn process_data_proposal(
        data_proposal: &mut DataProposal,
        known_contracts: Arc<std::sync::RwLock<KnownContracts>>,
    ) -> DataProposalVerdict {
        for tx in &data_proposal.txs {
            match &tx.transaction_data {
                TransactionData::Blob(blob_tx) => {
                    if let Err(e) = blob_tx.validate_identity() {
                        warn!(
                            "Refusing DataProposal: invalid identity in blob transaction: {}",
                            e
                        );
                        return DataProposalVerdict::Refuse;
                    }
                }
                TransactionData::Proof(_) => {
                    warn!("Refusing DataProposal: unverified recursive proof transaction");
                    return DataProposalVerdict::Refuse;
                }
                TransactionData::VerifiedProof(proof_tx) => {
                    // TODO: figure out what we want to do with the contracts.
                    // Extract the proof
                    let proof = match &proof_tx.proof {
                        Some(proof) => proof,
                        None => {
                            warn!("Refusing DataProposal: proof is missing");
                            return DataProposalVerdict::Refuse;
                        }
                    };
                    // TODO: we could early-reject proofs where the blob
                    // is not for the correct transaction.
                    #[allow(clippy::expect_used, reason = "not held across await")]
                    let (verifier, program_id) = match known_contracts
                        .read()
                        .expect("logic error")
                        .0
                        .get(&proof_tx.contract_name)
                        .cloned()
                    {
                        Some((verifier, program_id)) => (verifier, program_id),
                        None => {
                            match Self::find_contract(data_proposal, tx, &proof_tx.contract_name) {
                                Some((v, p)) => (v.clone(), p.clone()),
                                None => {
                                    warn!("Refusing DataProposal: contract not found");
                                    return DataProposalVerdict::Refuse;
                                }
                            }
                        }
                    };
                    // TODO: figure out how to generalize this
                    let is_recursive = proof_tx.contract_name.0 == "risc0-recursion";

                    if is_recursive {
                        match verify_recursive_proof(proof, &verifier, &program_id) {
                            Ok((local_program_ids, local_hyle_outputs)) => {
                                let data_matches = local_program_ids
                                    .iter()
                                    .zip(local_hyle_outputs.iter())
                                    .zip(proof_tx.proven_blobs.iter())
                                    .all(
                                        |(
                                            (local_program_id, local_hyle_output),
                                            BlobProofOutput {
                                                program_id,
                                                hyle_output,
                                                ..
                                            },
                                        )| {
                                            local_hyle_output == hyle_output
                                                && local_program_id == program_id
                                        },
                                    );
                                if local_program_ids.len() != proof_tx.proven_blobs.len()
                                    || !data_matches
                                {
                                    warn!("Refusing DataProposal: incorrect HyleOutput in proof transaction");
                                    return DataProposalVerdict::Refuse;
                                }
                            }
                            Err(e) => {
                                warn!("Refusing DataProposal: invalid recursive proof transaction: {}", e);
                                return DataProposalVerdict::Refuse;
                            }
                        }
                    } else {
                        match verify_proof(proof, &verifier, &program_id) {
                            Ok(outputs) => {
                                // TODO: we could check the blob hash here too.
                                if outputs.len() != proof_tx.proven_blobs.len()
                                    && std::iter::zip(outputs.iter(), proof_tx.proven_blobs.iter())
                                        .any(|(output, BlobProofOutput { hyle_output, .. })| {
                                            output != hyle_output
                                        })
                                {
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
                }
            }
        }

        // Remove proofs from transactions
        remove_proofs(data_proposal);

        DataProposalVerdict::Vote
    }

    pub fn store_data_proposal(
        &mut self,
        crypto: &BlstCrypto,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
    ) -> LaneBytesSize {
        // Updating validator's lane tip
        if let Some(validator_tip) = self.lanes_tip.get(validator) {
            // If validator already has a lane, we only update the tip if DP-chain is respected
            if let Some(parent_dp_hash) = &data_proposal.parent_data_proposal_hash {
                if validator_tip == parent_dp_hash {
                    self.lanes_tip
                        .insert(validator.clone(), data_proposal.hash());
                }
            }
        } else {
            self.lanes_tip
                .insert(validator.clone(), data_proposal.hash());
        }

        // Add DataProposal to validator's lane
        self.lanes
            .entry(validator.clone())
            .or_default()
            .add_new_proposal(crypto, data_proposal)
    }

    // Find the verifier and program_id for a contract name in the current lane, optimistically.
    fn find_contract(
        data_proposal: &DataProposal,
        tx: &Transaction,
        contract_name: &ContractName,
    ) -> Option<(Verifier, ProgramId)> {
        // Check if it's in the same data proposal.
        // (kind of inefficient, but it's mostly to make our tests work)
        // TODO: improve on this logic, possibly look into other data proposals / lanes.
        #[allow(
            clippy::unwrap_used,
            reason = "we know position will return a valid range"
        )]
        data_proposal
            .txs
            .get(
                0..data_proposal
                    .txs
                    .iter()
                    .position(|tx2| std::ptr::eq(tx, tx2))
                    .unwrap(),
            )
            .unwrap()
            .iter()
            .find_map(|tx| match &tx.transaction_data {
                TransactionData::Blob(tx) => tx.blobs.iter().find_map(|blob| {
                    if blob.contract_name.0 == "hyle" {
                        if let Ok(tx) = StructuredBlobData::<RegisterContractAction>::try_from(
                            blob.data.clone(),
                        ) {
                            if &tx.parameters.contract_name == contract_name {
                                return Some((tx.parameters.verifier, tx.parameters.program_id));
                            }
                        }
                    }
                    None
                }),
                _ => None,
            })
    }

    pub fn lane_has_data_proposal(
        &self,
        validator: &ValidatorPublicKey,
        data_proposal_hash: &DataProposalHash,
    ) -> bool {
        self.lanes
            .get(validator)
            .is_some_and(|lane| lane.has_proposal(data_proposal_hash))
    }

    pub fn get_lane_entries_between_hashes(
        &self,
        validator: &ValidatorPublicKey,
        from_data_proposal_hash: Option<&DataProposalHash>,
        to_data_proposal_hash: Option<&DataProposalHash>,
    ) -> Result<Vec<LaneEntry>> {
        if let Some(lane) = self.lanes.get(validator) {
            return lane
                .get_lane_entries_between_hashes(from_data_proposal_hash, to_data_proposal_hash);
        }
        bail!("Validator not found");
    }

    // Add lane entries to validator"s lane
    pub fn add_missing_lane_entries(
        &mut self,
        validator: &ValidatorPublicKey,
        lane_entries: Vec<LaneEntry>,
    ) -> Result<()> {
        let lane = self.lanes.entry(validator.clone()).or_default();

        debug!(
            "Trying to add missing lane entries on lane \n {}\n {:?}",
            lane, lane_entries
        );

        lane.add_missing_lane_entries(lane_entries.clone())?;

        // WARNING: This is not a proper way to do it. This *will* be properly done after refactoring storage to HashMap
        // Updating validator's lane tip
        if let Some(validator_tip) = self.lanes_tip.get(validator).cloned() {
            for le in lane_entries.iter() {
                // If validator already has a lane, we only update the tip if DP-chain is respected
                if let Some(parent_dp_hash) = &le.data_proposal.parent_data_proposal_hash {
                    if &validator_tip == parent_dp_hash {
                        self.lanes_tip
                            .insert(validator.clone(), le.data_proposal.hash());
                    }
                }
            }
        } else {
            self.lanes_tip.insert(
                validator.clone(),
                #[allow(clippy::unwrap_used, reason = "must exist because of previous checks")]
                lane_entries.last().unwrap().data_proposal.hash(),
            );
        }
        Ok(())
    }

    /// Creates and saves a new DataProposal if there are pending transactions
    pub fn new_data_proposal(&mut self, crypto: &BlstCrypto, txs: Vec<Transaction>) {
        if txs.is_empty() {
            return;
        }

        // Get last DataProposal of own lane
        let data_proposal = if let Some(LaneEntry {
            data_proposal: parent_data_proposal,
            cumul_size: _,
            signatures: _,
        }) = self
            .lanes
            .entry(self.id.clone())
            .or_default()
            .get_last_proposal()
        {
            // Create new data proposal
            DataProposal {
                id: parent_data_proposal.id + 1,
                parent_data_proposal_hash: Some(parent_data_proposal.hash()),
                txs,
            }
        } else {
            // Own lane has no parent DataProposal yet
            DataProposal {
                id: 0,
                parent_data_proposal_hash: None,
                txs,
            }
        };

        debug!(
            "Creating new DataProposal in local lane ({}) with {} transactions",
            self.id,
            data_proposal.txs.len()
        );

        self.store_data_proposal(crypto, &self.id.clone(), data_proposal);
    }

    pub fn get_lane_latest_entry(&self, validator: &ValidatorPublicKey) -> Option<&LaneEntry> {
        self.lanes
            .get(validator)
            .and_then(|lane| lane.get_last_proposal())
    }

    pub fn get_lane_pending_entries(
        &self,
        validator: &ValidatorPublicKey,
    ) -> Option<Vec<LaneEntry>> {
        self.lanes
            .get(validator)
            .map(|lane| lane.get_pending_entries())
    }

    pub fn get_lane_latest_data_proposal_hash(
        &self,
        validator: &ValidatorPublicKey,
    ) -> Option<&DataProposalHash> {
        self.lanes
            .get(validator)
            .and_then(|lane| lane.get_last_proposal_hash())
    }

    pub fn update_lanes_with_commited_cut(&mut self, committed_cut: &Cut) {
        for (validator, data_proposal_hash, _, poda) in committed_cut.iter() {
            if let Some(lane) = self.lanes.get_mut(validator) {
                lane.last_cut = Some((poda.clone(), data_proposal_hash.clone()));
            }
        }
    }

    pub fn try_update_lanes_tip(&mut self, cut: &Cut) {
        for (validator, data_proposal_hash, _, _) in cut.iter() {
            // If we know the hash, we don't change the tip (because either it is already at the tip, or we have advanced DPs)
            if self.lane_has_data_proposal(validator, data_proposal_hash) {
                continue;
            }
            // If we do not know the hash, 2 options:
            // 1) Cut's DP is ahead of our lane tip: we update the tip
            self.lanes_tip
                .insert(validator.clone(), data_proposal_hash.clone());
            // FIXME:
            // 2) Cut's DP is on a fork of our lane...
        }
    }
}

/// Remove proofs from all transactions in the DataProposal
fn remove_proofs(dp: &mut DataProposal) {
    dp.txs.iter_mut().for_each(|tx| {
        match &mut tx.transaction_data {
            TransactionData::VerifiedProof(proof_tx) => {
                proof_tx.proof = None;
            }
            TransactionData::Proof(_) => {
                // This can never happen.
                // A DataProposal that has been processed has turned all TransactionData::Proof into TransactionData::VerifiedProof
                unreachable!();
            }
            TransactionData::Blob(_) => {}
        }
    });
}

impl Display for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (
            _,
            LaneEntry {
                data_proposal,
                cumul_size: size,
                signatures: _,
            },
        ) in self.data_proposals.iter()
        {
            match &data_proposal.parent_data_proposal_hash {
                None => {
                    let _ = write!(f, "{} ({})", data_proposal, size);
                }
                Some(_) => {
                    let _ = write!(f, " <- {} ({})", data_proposal, size);
                }
            }
        }

        Ok(())
    }
}

impl Lane {
    pub fn has_proposal(&self, data_proposal_hash: &DataProposalHash) -> bool {
        self.data_proposals.contains_key(data_proposal_hash)
    }

    pub fn get_proposal(&self, hash: &DataProposalHash) -> Option<&LaneEntry> {
        self.data_proposals.get(hash)
    }

    pub fn get_proposal_mut(&mut self, hash: &DataProposalHash) -> Option<&mut LaneEntry> {
        self.data_proposals.get_mut(hash)
    }

    pub fn get_pending_entries(&self) -> Vec<LaneEntry> {
        let last_comitted_dp_hash = self.last_cut.as_ref().map(|(_, dp_hash)| dp_hash);
        let last_dp_hash = self.get_last_proposal_hash();

        self.get_lane_entries_between_hashes(last_comitted_dp_hash, last_dp_hash)
            .unwrap_or_default()
    }

    pub fn get_last_proposal(&self) -> Option<&LaneEntry> {
        self.data_proposals.iter().last().map(|(_, entry)| entry)
    }

    pub fn get_last_proposal_id(&self) -> Option<&u32> {
        self.data_proposals
            .iter()
            .last()
            .map(|(_, entry)| &entry.data_proposal.id)
    }

    pub fn get_last_proposal_hash(&self) -> Option<&DataProposalHash> {
        self.data_proposals
            .iter()
            .last()
            .map(|(data_proposal_hash, _)| data_proposal_hash)
    }

    pub fn get_lane_size(&self) -> LaneBytesSize {
        self.data_proposals
            .iter()
            .last()
            .map(|(_, entry)| entry.cumul_size)
            .unwrap_or_default()
    }

    pub fn add_new_proposal(
        &mut self,
        crypto: &BlstCrypto,
        data_proposal: DataProposal,
    ) -> LaneBytesSize {
        if let Some(le) = self.data_proposals.get(&data_proposal.hash()) {
            warn!(
                "DataProposal {} already exists in lane, size: {}",
                data_proposal.hash(),
                le.cumul_size
            );
            return le.cumul_size;
        }
        let data_proposal_hash = data_proposal.hash();
        let dp_size = data_proposal.estimate_size();
        let lane_size = self.get_lane_size();
        let tx_len = data_proposal.txs.len();
        let msg = MempoolNetMessage::DataVote(data_proposal_hash.clone(), lane_size + dp_size);
        let signatures = match crypto.sign(msg) {
            Ok(s) => vec![s],
            Err(_) => vec![],
        };
        self.data_proposals.insert(
            data_proposal_hash.clone(),
            LaneEntry {
                data_proposal,
                cumul_size: lane_size + dp_size,
                signatures,
            },
        );
        tracing::info!(
            "Added new DataProposal {} to lane, txs: {}, lane size: {}",
            data_proposal_hash,
            tx_len,
            lane_size + dp_size
        );

        lane_size + dp_size
    }

    pub fn add_proposal(&mut self, hash: DataProposalHash, lane_entry: LaneEntry) {
        self.data_proposals.insert(hash, lane_entry);
    }

    pub fn iter_reverse(&self) -> impl Iterator<Item = (&DataProposalHash, &LaneEntry)> {
        self.data_proposals.iter().rev()
    }

    pub fn current(&self) -> Option<(&DataProposalHash, &LaneEntry)> {
        self.data_proposals.last()
    }

    pub fn current_hash(&self) -> Option<&DataProposalHash> {
        self.current()
            .map(|(data_proposal_hash, _)| data_proposal_hash)
    }

    fn get_index_or_bail(&self, hash: &DataProposalHash) -> Result<usize> {
        self.data_proposals.get_index_of(hash).context(
            "Won't return any LaneEntry as aimed DataProposal {hash} does not exist on Lane",
        )
    }

    fn collect_entries(&self, start: usize, end: usize) -> Vec<LaneEntry> {
        self.data_proposals
            .values()
            .skip(start)
            .take(end - start)
            .cloned()
            .collect()
    }

    fn get_lane_entries_between_hashes(
        &self,
        from_data_proposal_hash: Option<&DataProposalHash>,
        to_data_proposal_hash: Option<&DataProposalHash>,
    ) -> Result<Vec<LaneEntry>> {
        let from_index = match from_data_proposal_hash {
            Some(hash) => {
                self.get_index_or_bail(hash)
                    .context("getting 'from' index")?
                    + 1
            }
            None => 0,
        };
        let to_index = match to_data_proposal_hash {
            Some(hash) => self.get_index_or_bail(hash).context("getting 'to' index")?,
            None => self.data_proposals.len(),
        };

        Ok(self.collect_entries(from_index, to_index + 1))
    }

    pub fn add_missing_lane_entries(&mut self, lane_entries: Vec<LaneEntry>) -> Result<()> {
        let lane_entries_len = lane_entries.len();
        let mut ordered_lane_entries = lane_entries;
        ordered_lane_entries.dedup();

        for lane_entry in ordered_lane_entries.into_iter() {
            if Some(&lane_entry.data_proposal.hash()) == self.current_hash() {
                debug!("Skipping already known LaneEntry");
                continue;
            }
            if lane_entry.data_proposal.parent_data_proposal_hash != self.current_hash().cloned() {
                bail!("Incorrect parent hash while adding missing LaneEntry");
            }
            let current_size = self.get_lane_size();
            let expected_size = current_size + lane_entry.data_proposal.estimate_size();
            if lane_entry.cumul_size != expected_size {
                bail!(
                    "Incorrect size while adding missing LaneEntry. Expected: {}, Got: {}",
                    expected_size,
                    lane_entry.cumul_size
                );
            }
            self.add_proposal(lane_entry.data_proposal.hash(), lane_entry);
        }
        debug!(
            "Nb data proposals after adding {} missing entries: {}",
            lane_entries_len,
            self.data_proposals.len()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use std::{
        collections::HashMap,
        sync::{Arc, RwLock},
        vec,
    };

    use crate::{
        mempool::{
            storage::{DataProposalHash, DataProposalVerdict, LaneEntry, Storage},
            test::make_register_contract_tx,
            KnownContracts, MempoolNetMessage,
        },
        model::{
            Blob, BlobData, BlobProofOutput, BlobTransaction, ContractName, Hashable, ProofData,
            ProofTransaction, Transaction, TransactionData, ValidatorPublicKey,
            VerifiedProofTransaction,
        },
        utils::crypto::{self, BlstCrypto},
    };
    use hyle_contract_sdk::{BlobIndex, HyleOutput, Identity, ProgramId, StateDigest, TxHash};
    use hyle_model::DataSized;
    use staking::state::Staking;

    use super::{DataProposal, Lane, LaneBytesSize};

    fn get_hyle_output() -> HyleOutput {
        HyleOutput {
            version: 1,
            initial_state: StateDigest(vec![0, 1, 2, 3]),
            next_state: StateDigest(vec![4, 5, 6]),
            identity: Identity::new("test"),
            tx_hash: TxHash::new(""),
            index: BlobIndex(0),
            blobs: vec![],
            success: true,
            tx_ctx: None,
            registered_contracts: vec![],
            program_outputs: vec![],
        }
    }

    fn make_proof_tx(contract_name: ContractName) -> ProofTransaction {
        let hyle_output = get_hyle_output();
        ProofTransaction {
            contract_name: contract_name.clone(),
            proof: ProofData(
                bincode::encode_to_vec(vec![hyle_output.clone()], bincode::config::standard())
                    .unwrap(),
            ),
        }
    }

    fn make_unverified_proof_tx(contract_name: ContractName) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::Proof(make_proof_tx(contract_name)),
        }
    }

    fn make_verified_proof_tx(contract_name: ContractName) -> Transaction {
        let hyle_output = get_hyle_output();
        let proof = ProofData(
            bincode::encode_to_vec(vec![hyle_output.clone()], bincode::config::standard()).unwrap(),
        );
        Transaction {
            version: 1,
            transaction_data: TransactionData::VerifiedProof(VerifiedProofTransaction {
                contract_name: contract_name.clone(),
                proof_hash: proof.hash(),
                proven_blobs: vec![BlobProofOutput {
                    program_id: ProgramId(vec![]),
                    blob_tx_hash: TxHash::default(),
                    hyle_output,
                    original_proof_hash: proof.hash(),
                }],
                proof_size: proof.0.len(),
                proof: Some(proof),
                is_recursive: false,
            }),
        }
    }

    fn make_empty_verified_proof_tx(contract_name: ContractName) -> Transaction {
        let hyle_output = get_hyle_output();
        let proof = ProofData(
            bincode::encode_to_vec(vec![hyle_output.clone()], bincode::config::standard()).unwrap(),
        );
        Transaction {
            version: 1,
            transaction_data: TransactionData::VerifiedProof(VerifiedProofTransaction {
                contract_name: contract_name.clone(),
                proof_hash: proof.hash(),
                proof_size: proof.0.len(),
                proven_blobs: vec![BlobProofOutput {
                    program_id: ProgramId(vec![]),
                    blob_tx_hash: TxHash::default(),
                    hyle_output,
                    original_proof_hash: proof.hash(),
                }],
                proof: None,
                is_recursive: false,
            }),
        }
    }

    fn make_blob_tx(inner_tx: &'static str) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::Blob(BlobTransaction {
                identity: Identity::new("id.c1"),
                blobs: vec![Blob {
                    contract_name: ContractName::new("c1"),
                    data: BlobData(inner_tx.as_bytes().to_vec()),
                }],
            }),
        }
    }

    fn handle_data_proposal(
        store: &mut Storage,
        crypto: &BlstCrypto,
        pubkey: &ValidatorPublicKey,
        mut data_proposal: DataProposal,
        known_contracts: Arc<RwLock<KnownContracts>>,
    ) -> (DataProposalVerdict, Option<LaneBytesSize>) {
        let (verdict, size) = store.on_data_proposal(pubkey, &data_proposal);
        let verdict = match verdict {
            DataProposalVerdict::Process => {
                Storage::process_data_proposal(&mut data_proposal, known_contracts)
            }
            verdict => verdict,
        };
        match verdict {
            DataProposalVerdict::Vote => {
                let size = store.store_data_proposal(crypto, pubkey, data_proposal);
                (verdict, Some(size))
            }
            verdict => (verdict, size),
        }
    }

    #[test_log::test]
    fn test_data_proposal_hash_with_verified_proof() {
        let contract_name = ContractName::new("test");

        let proof_tx_with_proof = make_verified_proof_tx(contract_name.clone());
        let proof_tx_without_proof = make_empty_verified_proof_tx(contract_name.clone());

        let data_proposal_with_proof = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![proof_tx_with_proof],
        };

        let data_proposal_without_proof = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![proof_tx_without_proof],
        };

        let hash_with_proof = data_proposal_with_proof.hash();
        let hash_without_proof = data_proposal_without_proof.hash();

        assert_eq!(hash_with_proof, hash_without_proof);
    }

    #[test_log::test]
    fn test_add_missing_lane_entries() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();
        let mut store = Storage::new(pubkey1.clone(), HashMap::default());

        let data_proposal1 = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let data_proposal1_hash = data_proposal1.hash();
        let l_dp1_size = LaneBytesSize(data_proposal1.estimate_size() as u64);

        let data_proposal2 = DataProposal {
            id: 1,
            parent_data_proposal_hash: Some(data_proposal1_hash.clone()),
            txs: vec![],
        };
        let data_proposal2_hash = data_proposal2.hash();
        let l_dp2_size = l_dp1_size + data_proposal2.estimate_size();

        let lane_entry1 = LaneEntry {
            data_proposal: data_proposal1,
            cumul_size: l_dp1_size,
            signatures: vec![],
        };

        let lane_entry2 = LaneEntry {
            data_proposal: data_proposal2,
            cumul_size: l_dp2_size,
            signatures: vec![],
        };

        store
            .add_missing_lane_entries(pubkey1, vec![lane_entry1.clone(), lane_entry2.clone()])
            .expect("Failed to add missing lane entries");

        let lane = store.lanes.get(pubkey1).expect("Lane not found");
        assert!(lane.has_proposal(&data_proposal1_hash));
        assert!(lane.has_proposal(&data_proposal2_hash));

        // Ensure the lane entries are in the correct order
        let lane_entries = lane
            .data_proposals
            .values()
            .cloned()
            .collect::<Vec<LaneEntry>>();
        assert_eq!(lane_entries, vec![lane_entry1.clone(), lane_entry2.clone()]);

        // Adding an incorrect data proposal should fail
        let data_proposal3 = DataProposal {
            id: 0,
            parent_data_proposal_hash: Some(DataProposalHash("non_existent".to_string())),
            txs: vec![],
        };
        let data_proposal3_hash = data_proposal3.hash();
        let l_dp3_size = l_dp2_size + data_proposal3.estimate_size();

        let lane_entry3 = LaneEntry {
            data_proposal: data_proposal3,
            cumul_size: l_dp3_size,
            signatures: vec![],
        };

        assert!(store
            .add_missing_lane_entries(pubkey1, vec![lane_entry3.clone()])
            .is_err());
        let lane = store.lanes.get(pubkey1).expect("Lane not found");
        // Ensure incorrect data proposal is not in the lane entries
        assert!(!lane.has_proposal(&data_proposal3_hash));
    }

    #[test_log::test]
    fn test_get_lane_entries_between_hashes() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();
        let mut store = Storage::new(pubkey1.clone(), HashMap::default());

        let data_proposal1 = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let data_proposal1_hash = data_proposal1.hash();
        let l_dp1_size = LaneBytesSize(data_proposal1.estimate_size() as u64);

        let data_proposal2 = DataProposal {
            id: 1,
            parent_data_proposal_hash: Some(data_proposal1_hash.clone()),
            txs: vec![],
        };
        let data_proposal2_hash = data_proposal2.hash();
        let l_dp2_size = l_dp1_size + data_proposal2.estimate_size();

        let data_proposal3 = DataProposal {
            id: 2,
            parent_data_proposal_hash: Some(data_proposal2_hash.clone()),
            txs: vec![],
        };
        let data_proposal3_hash = data_proposal3.hash();
        let l_dp3_size = l_dp2_size + data_proposal3.estimate_size();

        let lane_entry1 = LaneEntry {
            data_proposal: data_proposal1,
            cumul_size: l_dp1_size,
            signatures: vec![],
        };

        let lane_entry2 = LaneEntry {
            data_proposal: data_proposal2,
            cumul_size: l_dp2_size,
            signatures: vec![],
        };

        let lane_entry3 = LaneEntry {
            data_proposal: data_proposal3,
            cumul_size: l_dp3_size,
            signatures: vec![],
        };

        store
            .add_missing_lane_entries(
                pubkey1,
                vec![
                    lane_entry1.clone(),
                    lane_entry2.clone(),
                    lane_entry3.clone(),
                ],
            )
            .expect("Failed to add missing lane entries");

        let lane = store.lanes.get(pubkey1).expect("Lane not found");

        // Test getting all entries from the beginning to the second proposal
        let entries = lane
            .get_lane_entries_between_hashes(None, Some(&data_proposal2_hash))
            .expect("Failed to get lane entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], lane_entry1);
        assert_eq!(entries[1], lane_entry2);

        // Test getting entries between the first and second proposal
        let entries = lane
            .get_lane_entries_between_hashes(Some(&data_proposal1_hash), Some(&data_proposal2_hash))
            .expect("Failed to get lane entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], lane_entry2);

        // Test getting entries between the first, second and third proposals
        let entries = lane
            .get_lane_entries_between_hashes(Some(&data_proposal1_hash), Some(&data_proposal3_hash))
            .expect("Failed to get lane entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], lane_entry2);
        assert_eq!(entries[1], lane_entry3);

        // Test getting entries between the first, and the first proposals (empty)
        let entries = lane
            .get_lane_entries_between_hashes(Some(&data_proposal1_hash), Some(&data_proposal1_hash))
            .expect("Failed to get lane entries");
        assert!(entries.is_empty());

        // Test getting entries with a non-existent starting hash
        let non_existent_hash = DataProposalHash("non_existent".to_string());
        let entries = lane
            .get_lane_entries_between_hashes(Some(&non_existent_hash), Some(&data_proposal2_hash));
        assert!(entries.is_err());

        // Test getting entries with a non-existent ending hash
        let entries = lane
            .get_lane_entries_between_hashes(Some(&data_proposal1_hash), Some(&non_existent_hash));
        assert!(entries.is_err());
    }

    fn lane<'a>(store: &'a mut Storage, id: &ValidatorPublicKey) -> &'a mut Lane {
        store.lanes.entry(id.clone()).or_default()
    }

    #[test_log::test]
    fn test_on_poa_update() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let crypto2 = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();
        let pubkey2 = crypto2.validator_pubkey();
        let mut store1 = Storage::new(pubkey1.clone(), HashMap::default());

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let data_proposal_hash = data_proposal.hash();
        let l_dp_size = LaneBytesSize(data_proposal.estimate_size() as u64);

        lane(&mut store1, pubkey1).add_new_proposal(&crypto1, data_proposal);

        let signatures = vec![
            crypto1
                .sign(MempoolNetMessage::DataVote(
                    data_proposal_hash.clone(),
                    l_dp_size,
                ))
                .expect("Failed to sign message"),
            crypto2
                .sign(MempoolNetMessage::DataVote(
                    data_proposal_hash.clone(),
                    l_dp_size,
                ))
                .expect("Failed to sign message"),
        ];

        store1.on_poda_update(pubkey1, &data_proposal_hash, signatures);

        let lane = store1.lanes.get(pubkey1).expect("Lane not found");
        let lane_entry = lane
            .get_proposal(&data_proposal_hash)
            .expect("Data proposal not found");

        assert_eq!(lane_entry.signatures.len(), 2);
        assert!(lane_entry
            .signatures
            .iter()
            .any(|s| &s.signature.validator == pubkey1));
        assert!(lane_entry
            .signatures
            .iter()
            .any(|s| &s.signature.validator == pubkey2));
    }

    #[test_log::test]
    fn test_workflow() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let crypto2 = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();
        let pubkey2 = crypto2.validator_pubkey();
        let mut store1 = Storage::new(pubkey1.clone(), HashMap::default());
        let mut store2 = Storage::new(pubkey2.clone(), HashMap::default());
        let known_contracts = Arc::new(RwLock::new(KnownContracts::default()));

        // First data proposal
        let tx1 = make_blob_tx("test1");
        store1.new_data_proposal(&crypto1, vec![tx1]);

        let data_proposal1 = store1
            .get_lane_latest_entry(pubkey1)
            .unwrap()
            .data_proposal
            .clone();
        let data_proposal1_hash = data_proposal1.hash();
        let l_dp1_size = LaneBytesSize(data_proposal1.estimate_size() as u64);

        assert_eq!(
            handle_data_proposal(
                &mut store2,
                &crypto2,
                pubkey1,
                data_proposal1,
                known_contracts.clone()
            ),
            (DataProposalVerdict::Vote, Some(l_dp1_size))
        );

        let msg1 = crypto2
            .sign(MempoolNetMessage::DataVote(
                data_proposal1_hash.clone(),
                l_dp1_size,
            ))
            .expect("Could not sign DataVote message");

        store1
            .on_data_vote(&msg1, &data_proposal1_hash, l_dp1_size)
            .expect("Expect vote success");

        // Second data proposal
        let tx2 = make_blob_tx("test2");
        store1.new_data_proposal(&crypto1, vec![tx2]);

        let data_proposal2 = store1
            .get_lane_latest_entry(pubkey1)
            .unwrap()
            .data_proposal
            .clone();
        let data_proposal2_hash = data_proposal2.hash();
        let l_dp2_size = l_dp1_size + data_proposal2.estimate_size();

        assert_eq!(
            handle_data_proposal(
                &mut store2,
                &crypto2,
                pubkey1,
                data_proposal2,
                known_contracts.clone()
            ),
            (DataProposalVerdict::Vote, Some(l_dp2_size))
        );
        let msg2 = crypto2
            .sign(MempoolNetMessage::DataVote(
                data_proposal2_hash.clone(),
                l_dp2_size,
            ))
            .expect("Could not sign DataVote message");

        store1
            .on_data_vote(&msg2, &data_proposal2_hash, l_dp2_size)
            .expect("vote success");

        // Third data proposal
        let tx3 = make_blob_tx("test3");
        store1.new_data_proposal(&crypto1, vec![tx3]);

        let data_proposal3 = store1
            .get_lane_latest_entry(pubkey1)
            .unwrap()
            .data_proposal
            .clone();
        let data_proposal3_hash = data_proposal3.hash();
        let l_dp3_size = l_dp2_size + data_proposal3.estimate_size();

        assert_eq!(
            handle_data_proposal(
                &mut store2,
                &crypto2,
                pubkey1,
                data_proposal3,
                known_contracts.clone()
            ),
            (DataProposalVerdict::Vote, Some(l_dp3_size))
        );
        let msg3 = crypto2
            .sign(MempoolNetMessage::DataVote(
                data_proposal3_hash.clone(),
                l_dp3_size,
            ))
            .expect("Could not sign DataVote message");

        store1
            .on_data_vote(&msg3, &data_proposal3_hash, l_dp3_size)
            .expect("vote success");

        // Fourth data proposal
        let tx4 = make_blob_tx("test4");
        store1.new_data_proposal(&crypto1, vec![tx4]);

        let data_proposal4 = store1
            .get_lane_latest_entry(pubkey1)
            .unwrap()
            .data_proposal
            .clone();
        let data_proposal4_hash = data_proposal4.hash();
        let l_dp4_size = l_dp3_size + data_proposal4.estimate_size();

        assert_eq!(
            handle_data_proposal(
                &mut store2,
                &crypto2,
                pubkey1,
                data_proposal4,
                known_contracts.clone()
            ),
            (DataProposalVerdict::Vote, Some(l_dp4_size))
        );
        let msg4 = crypto2
            .sign(MempoolNetMessage::DataVote(
                data_proposal4_hash.clone(),
                l_dp4_size,
            ))
            .expect("Could not sign DataVote message");

        store1
            .on_data_vote(&msg4, &data_proposal4_hash, l_dp4_size)
            .expect("vote success");

        // Verifications
        assert_eq!(store2.lanes.len(), 2);
        assert!(store2.lanes.contains_key(pubkey1));

        let store_own_lane = store1.lanes.get(pubkey1).expect("lane");
        let (_, first_data_proposal_entry) = store_own_lane
            .data_proposals
            .first()
            .expect("first data proposal");
        assert_eq!(
            first_data_proposal_entry
                .data_proposal
                .parent_data_proposal_hash,
            None
        );
        assert_eq!(
            first_data_proposal_entry.data_proposal.txs,
            vec![make_blob_tx("test1")]
        );

        let validators_that_signed = first_data_proposal_entry
            .signatures
            .iter()
            .map(|s| s.signature.validator.clone())
            .collect::<Vec<_>>();
        assert!(validators_that_signed.contains(pubkey2));

        let store2_own_lane = store2.lanes.get(pubkey1).expect("lane");
        let (_, first_data_proposal_entry) = store2_own_lane
            .data_proposals
            .first()
            .expect("first data proposal");
        assert_eq!(
            first_data_proposal_entry
                .data_proposal
                .parent_data_proposal_hash,
            None
        );
        assert_eq!(
            first_data_proposal_entry.data_proposal.txs,
            vec![make_blob_tx("test1")]
        );

        let lane2_entries = store2
            .get_lane_entries_between_hashes(pubkey1, None, Some(&data_proposal4_hash))
            .expect("Could not load own lane entries");

        assert_eq!(lane2_entries.len(), 4);
    }

    #[test_log::test]
    fn test_vote() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let crypto2 = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let crypto3 = crypto::BlstCrypto::new("3".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();
        let pubkey2 = crypto2.validator_pubkey();
        let pubkey3 = crypto3.validator_pubkey();
        let mut store2 = Storage::new(pubkey2.clone(), HashMap::default());
        let mut store3 = Storage::new(pubkey3.clone(), HashMap::default());
        let known_contracts2 = Arc::new(RwLock::new(KnownContracts::default()));

        let txs = vec![
            make_blob_tx("test1"),
            make_blob_tx("test2"),
            make_blob_tx("test3"),
            make_blob_tx("test4"),
        ];

        store3.new_data_proposal(&crypto3, txs);
        let data_proposal = store3
            .get_lane_latest_entry(pubkey3)
            .unwrap()
            .data_proposal
            .clone();
        let size = store3.get_lane_latest_entry(pubkey3).unwrap().cumul_size;
        let data_proposal_hash = data_proposal.hash();
        assert_eq!(store3.lanes.get(pubkey3).unwrap().data_proposals.len(), 1);

        assert_eq!(
            handle_data_proposal(
                &mut store2,
                &crypto2,
                pubkey3,
                data_proposal.clone(),
                known_contracts2.clone()
            ),
            (DataProposalVerdict::Vote, Some(size))
        );
        // Assert we can vote multiple times
        assert_eq!(
            handle_data_proposal(
                &mut store2,
                &crypto2,
                pubkey3,
                data_proposal,
                known_contracts2
            ),
            (DataProposalVerdict::Vote, Some(size))
        );

        let msg1 = crypto1
            .sign(MempoolNetMessage::DataVote(
                data_proposal_hash.clone(),
                size,
            ))
            .expect("Could not sign DataVote message");

        store3
            .on_data_vote(&msg1, &data_proposal_hash, size)
            .expect("success");

        let msg2 = crypto2
            .sign(MempoolNetMessage::DataVote(
                data_proposal_hash.clone(),
                size,
            ))
            .expect("Could not sign DataVote message");

        store3
            .on_data_vote(&msg2, &data_proposal_hash, size)
            .expect("success");

        assert_eq!(
            store3
                .lanes
                .get(pubkey3)
                .unwrap()
                .data_proposals
                .get(&data_proposal_hash)
                .unwrap()
                .signatures
                .len(),
            3
        );

        let (_, first_data_proposal_entry) = store3
            .lanes
            .get(pubkey3)
            .expect("lane")
            .data_proposals
            .first()
            .expect("first data proposal");
        let validators_that_signed = first_data_proposal_entry
            .signatures
            .iter()
            .map(|s| s.signature.validator.clone())
            .collect::<Vec<_>>();
        assert!(validators_that_signed.contains(pubkey1));
        assert!(validators_that_signed.contains(pubkey2));
    }

    #[test_log::test]
    fn test_update_lane_with_unverified_proof_transaction() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let crypto2 = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();
        let pubkey2 = crypto2.validator_pubkey();

        let mut store1 = Storage::new(pubkey1.clone(), HashMap::default());
        let known_contracts = Arc::new(RwLock::new(KnownContracts::default()));

        let contract_name = ContractName::new("test");
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_unverified_proof_tx(contract_name.clone());

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![register_tx, proof_tx],
        };
        let data_proposal_hash = data_proposal.hash();

        let (verdict, _) = handle_data_proposal(
            &mut store1,
            &crypto1,
            pubkey2,
            data_proposal,
            known_contracts,
        );
        assert_eq!(verdict, DataProposalVerdict::Refuse);

        // Ensure the lane was not updated with the unverified proof transaction
        assert!(!store1.lane_has_data_proposal(pubkey2, &data_proposal_hash));
    }

    #[test_log::test]
    fn test_update_lane_with_verified_proof_transaction() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();

        let mut store1 = Storage::new(pubkey1.clone(), HashMap::default());
        let known_contracts = Arc::new(RwLock::new(KnownContracts::default()));

        let contract_name = ContractName::new("test");
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![proof_tx.clone()],
        };

        let (verdict, _) = handle_data_proposal(
            &mut store1,
            &crypto1,
            pubkey1,
            data_proposal,
            known_contracts.clone(),
        );
        assert_eq!(verdict, DataProposalVerdict::Refuse); // refused because contract not found

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![register_tx, proof_tx],
        };

        let (verdict, _) = handle_data_proposal(
            &mut store1,
            &crypto1,
            pubkey1,
            data_proposal,
            known_contracts,
        );
        assert_eq!(verdict, DataProposalVerdict::Vote);
    }

    #[test_log::test]
    // This test currently panics as we no longer optimistically register contracts
    #[should_panic]
    fn test_new_data_proposal_with_register_tx_in_previous_uncommitted_car() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();

        let mut store1 = Storage::new(pubkey1.clone(), HashMap::default());
        let known_contracts = Arc::new(RwLock::new(KnownContracts::default()));

        let contract_name = ContractName::new("test");
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_verified_proof_tx(contract_name.clone());

        let data_proposal1 = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![register_tx],
        };
        let data_proposal1_hash = data_proposal1.hash();

        lane(&mut store1, pubkey1).add_new_proposal(&crypto1, data_proposal1);

        let data_proposal = DataProposal {
            id: 1,
            parent_data_proposal_hash: Some(data_proposal1_hash.clone()),
            txs: vec![proof_tx],
        };

        let (verdict, _) = handle_data_proposal(
            &mut store1,
            &crypto1,
            pubkey1,
            data_proposal,
            known_contracts,
        );
        assert_eq!(verdict, DataProposalVerdict::Vote);

        // Ensure the lane was updated with the DataProposal
        let empty_verified_proof_tx = make_empty_verified_proof_tx(contract_name.clone());
        let saved_data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: Some(data_proposal1_hash),
            txs: vec![empty_verified_proof_tx.clone()],
        };
        assert!(store1.lane_has_data_proposal(pubkey1, &saved_data_proposal.hash()));
    }

    #[test_log::test]
    fn test_register_contract_and_proof_tx_in_same_car() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();

        let mut store1 = Storage::new(pubkey1.clone(), HashMap::default());
        let known_contracts = Arc::new(RwLock::new(KnownContracts::default()));

        let contract_name = ContractName::new("test");
        let register_tx = make_register_contract_tx(contract_name.clone());
        let proof_tx = make_verified_proof_tx(contract_name.clone());

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![register_tx.clone(), proof_tx],
        };

        let (verdict, _) = handle_data_proposal(
            &mut store1,
            &crypto1,
            pubkey1,
            data_proposal,
            known_contracts,
        );
        assert_eq!(verdict, DataProposalVerdict::Vote);

        // Ensure the lane was updated with the DataProposal
        let empty_verified_proof_tx = make_empty_verified_proof_tx(contract_name.clone());
        let saved_data_proposal = DataProposal {
            parent_data_proposal_hash: None,
            id: 0,
            txs: vec![register_tx, empty_verified_proof_tx.clone()],
        };
        assert!(store1.lane_has_data_proposal(pubkey1, &saved_data_proposal.hash()));
    }

    #[test_log::test]
    fn test_register_contract_and_proof_tx_in_same_car_wrong_order() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let crypto2 = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();
        let pubkey2 = crypto2.validator_pubkey();

        let mut store1 = Storage::new(pubkey2.clone(), HashMap::default());
        let known_contracts = Arc::new(RwLock::new(KnownContracts::default()));

        let contract_name = ContractName::new("test");
        let register_tx = make_register_contract_tx(contract_name.clone());
        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![proof_tx, register_tx],
        };
        let data_proposal_hash = data_proposal.hash();

        let (verdict, _) = handle_data_proposal(
            &mut store1,
            &crypto1,
            pubkey1,
            data_proposal,
            known_contracts,
        );
        assert_eq!(verdict, DataProposalVerdict::Refuse);

        // Ensure the lane was not updated with the DataProposal
        assert!(!store1.lane_has_data_proposal(pubkey1, &data_proposal_hash));
    }

    #[test_log::test]
    fn test_new_cut() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let crypto2 = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let pubkey1 = crypto1.validator_pubkey();
        let pubkey2 = crypto2.validator_pubkey();

        let mut store1 = Storage::new(pubkey1.clone(), HashMap::default());
        let mut store2 = Storage::new(pubkey2.clone(), HashMap::default());
        let known_contracts1 = Arc::new(RwLock::new(KnownContracts::default()));
        let known_contracts2 = Arc::new(RwLock::new(KnownContracts::default()));
        let mut staking = Staking::default();
        staking.stake("pk1".into(), 100).expect("could not stake");
        staking
            .delegate_to("pk1".into(), pubkey1.clone())
            .expect("could not delegate");
        staking.stake("pk2".into(), 100).expect("could not stake");
        staking
            .delegate_to("pk2".into(), pubkey2.clone())
            .expect("could not delegate");
        staking
            .bond(pubkey1.clone())
            .expect("Could not bond pubkey1");
        staking
            .bond(pubkey2.clone())
            .expect("Could not bond pubkey2");

        let tx1 = make_blob_tx("test1");
        store1.new_data_proposal(&crypto1, vec![tx1]);
        let data_proposal1 = store1
            .get_lane_latest_entry(pubkey1)
            .unwrap()
            .data_proposal
            .clone();
        let size = store1.get_lane_latest_entry(pubkey1).unwrap().cumul_size;

        assert_eq!(
            handle_data_proposal(
                &mut store2,
                &crypto2,
                pubkey1,
                data_proposal1.clone(),
                known_contracts2.clone()
            ),
            (DataProposalVerdict::Vote, Some(size))
        );

        let tx2 = make_blob_tx("tx2");
        store2.new_data_proposal(&crypto2, vec![tx2]);
        let data_proposal2 = store2
            .get_lane_latest_entry(pubkey2)
            .unwrap()
            .data_proposal
            .clone();
        let size = store2.get_lane_latest_entry(pubkey2).unwrap().cumul_size;

        assert_eq!(
            handle_data_proposal(
                &mut store1,
                &crypto1,
                pubkey2,
                data_proposal2.clone(),
                known_contracts1
            ),
            (DataProposalVerdict::Vote, Some(size))
        );

        let cut1 = store1.new_cut(&staking);
        assert_eq!(cut1.len(), 2);
        assert_eq!(cut1[0].0, pubkey1.clone());
        assert_eq!(cut1[0].1, data_proposal1.hash());
        assert_eq!(cut1[1].0, pubkey2.clone());
        assert_eq!(cut1[1].1, data_proposal2.hash());
    }

    #[test_log::test]
    fn test_poda() {
        let crypto1 = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let crypto2 = crypto::BlstCrypto::new("2".to_owned()).unwrap();

        let pubkey1 = crypto1.validator_pubkey();
        let pubkey2 = crypto2.validator_pubkey();

        let mut store1 = Storage::new(pubkey1.clone(), HashMap::default());
        let mut staking = Staking::default();

        staking.stake("pk1".into(), 100).expect("Staking failed");
        staking
            .delegate_to("pk1".into(), pubkey1.clone())
            .expect("Delegation failed");
        staking.stake("pk2".into(), 100).expect("Staking failed");
        staking
            .delegate_to("pk2".into(), pubkey2.clone())
            .expect("Delegation failed");

        staking
            .bond(pubkey1.clone())
            .expect("Could not bond pubkey1");
        staking
            .bond(pubkey2.clone())
            .expect("Could not bond pubkey2");

        let tx1 = make_blob_tx("test1");
        store1.new_data_proposal(&crypto1, vec![tx1]);

        let data_proposal = store1
            .get_lane_latest_entry(pubkey1)
            .unwrap()
            .data_proposal
            .clone();
        let data_proposal_hash = data_proposal.hash();
        let size = store1.get_lane_latest_entry(pubkey1).unwrap().cumul_size;

        let msg2 = crypto2
            .sign(MempoolNetMessage::DataVote(
                data_proposal_hash.clone(),
                size,
            ))
            .expect("Could not sign DataVote message");

        store1
            .on_data_vote(&msg2, &data_proposal_hash, size)
            .expect("Expect vote success");

        let cut = store1.new_cut(&staking);
        let poda = cut[0].3.clone();

        assert!(poda.validators.contains(pubkey1));
        assert!(poda.validators.contains(pubkey2));
        assert_eq!(cut.len(), 1);
    }

    #[test_log::test]
    fn test_add_new_proposal() {
        let crypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let mut lane = Lane::default();

        let tx = make_verified_proof_tx("testContract".into());

        let data_proposal1 = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![tx],
        };

        let size = lane.add_new_proposal(&crypto, data_proposal1.clone());
        assert_eq!(size, lane.get_lane_size());
        assert_eq!(size.0, data_proposal1.estimate_size() as u64);

        // Test adding the same proposal again
        let size_again = lane.add_new_proposal(&crypto, data_proposal1.clone());
        assert_eq!(size, size_again);

        // Adding a new DP
        let tx2 = make_register_contract_tx("testContract2".into());
        let data_proposal2 = DataProposal {
            id: 1,
            parent_data_proposal_hash: Some(data_proposal1.hash()),
            txs: vec![tx2],
        };
        let size = lane.add_new_proposal(&crypto, data_proposal2.clone());
        assert_eq!(size, lane.get_lane_size());
        assert_eq!(
            size.0,
            (data_proposal1.estimate_size() + data_proposal2.estimate_size()) as u64
        );

        // Test adding the same proposal again
        let size_again = lane.add_new_proposal(&crypto, data_proposal2.clone());
        assert_eq!(size, size_again);
        let size = lane.add_new_proposal(&crypto, data_proposal1.clone());
        assert_eq!(size.0, data_proposal1.estimate_size() as u64);
    }
}
