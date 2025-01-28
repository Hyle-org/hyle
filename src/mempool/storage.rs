use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use hyle_model::{
    ContractName, DataSized, ProgramId, RegisterContractAction, Signed, StructuredBlobData,
    ValidatorSignature, Verifier,
};
use serde::{Deserialize, Serialize};
use staking::state::Staking;
use std::{collections::HashMap, path::Path, sync::Arc, vec};
use tracing::{debug, error, warn};

use crate::{
    model::{
        BlobProofOutput, Cut, DataProposal, DataProposalHash, Hashable, PoDA, SignedByValidator,
        Transaction, TransactionData, ValidatorPublicKey,
    },
    utils::{crypto::BlstCrypto, logger::LogMe},
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

pub use hyle_model::LaneBytesSize;

#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneEntry {
    pub data_proposal: DataProposal,
    pub cumul_size: LaneBytesSize,
    pub signatures: Vec<SignedByValidator<MempoolNetMessage>>,
}

pub trait Storage {
    fn new(
        path: &Path,
        id: ValidatorPublicKey,
        lanes_tip: HashMap<ValidatorPublicKey, DataProposalHash>,
    ) -> Result<Self>
    where
        Self: std::marker::Sized;
    fn id(&self) -> &ValidatorPublicKey;
    fn contains(&self, validator_key: &ValidatorPublicKey, dp_hash: &DataProposalHash) -> bool;
    fn get_by_hash(
        &self,
        validator_key: &ValidatorPublicKey,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<LaneEntry>>;
    fn put(&mut self, validator_key: ValidatorPublicKey, lane_entry: LaneEntry) -> Result<()>;
    fn update(&mut self, validator_key: ValidatorPublicKey, lane_entry: LaneEntry) -> Result<()>;
    fn persist(&self) -> Result<()>;
    fn get_lane_tip(&self, validator: &ValidatorPublicKey) -> Option<&DataProposalHash>;
    fn update_lane_tip(
        &mut self,
        validator: ValidatorPublicKey,
        dp_hash: DataProposalHash,
    ) -> Option<DataProposalHash>;

    // Called by the initial proposal validator to aggregate votes
    fn on_data_vote(
        &mut self,
        msg: &SignedByValidator<MempoolNetMessage>,
        data_proposal_hash: &DataProposalHash,
        new_lane_size: LaneBytesSize,
    ) -> Result<(
        DataProposalHash,
        Vec<Signed<MempoolNetMessage, ValidatorSignature>>,
    )> {
        let id = self.id().clone();
        match self.get_by_hash(&id, data_proposal_hash)? {
            Some(mut lane_entry) => {
                if lane_entry.cumul_size != new_lane_size {
                    bail!("Received size {new_lane_size} does no match the actual size of the DataProposal ({})", lane_entry.cumul_size);
                }
                lane_entry.signatures.push(msg.clone());
                lane_entry.signatures.dedup();

                // Update LaneEntry for self
                self.update(self.id().clone(), lane_entry.clone())?;

                Ok((data_proposal_hash.clone(), lane_entry.signatures))
            }
            None => bail!(
                "Received vote from unknown validator {}",
                msg.signature.validator,
            ),
        }
    }

    fn on_poda_update(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal_hash: &DataProposalHash,
        signatures: Vec<SignedByValidator<MempoolNetMessage>>,
    ) -> Result<()> {
        match self
            .get_by_hash(validator, data_proposal_hash)
            .log_warn("Received PoDA update for unknown DataProposal")?
        {
            Some(mut lane_entry) => {
                lane_entry.signatures.extend(signatures);
                lane_entry.signatures.dedup();
                self.put(validator.clone(), lane_entry)
            }
            None => bail!(
                "Received poda update for unknown DP {} for validator {}",
                data_proposal_hash,
                validator
            ),
        }
    }

    fn get_latest_car(
        &self,
        validator: &ValidatorPublicKey,
        staking: &Staking,
        previous_committed_car: Option<&(DataProposalHash, PoDA)>,
    ) -> Result<Option<(DataProposalHash, LaneBytesSize, PoDA)>> {
        let bonded_validators = staking.bonded();
        // We start from the tip of the lane, and go backup until we find a DP with enough signatures
        if let Some(tip_dp_hash) = self.get_lane_tip(validator) {
            let mut dp_hash = tip_dp_hash.clone();
            while let Some(le) = self.get_by_hash(validator, &dp_hash)? {
                if let Some((hash, poda)) = previous_committed_car {
                    if &dp_hash == hash {
                        // Latest car has already been committed
                        return Ok(Some((hash.clone(), le.cumul_size, poda.clone())));
                    }
                }
                // Filter signatures on DataProposal to only keep the ones from the current validators
                let filtered_signatures: Vec<SignedByValidator<MempoolNetMessage>> = le
                    .signatures
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
                    if let Some(parent_dp_hash) = le.data_proposal.parent_data_proposal_hash {
                        dp_hash = parent_dp_hash.clone();
                        continue;
                    }
                    return Ok(None);
                }

                // Aggregate the signatures in a PoDA
                let poda = match BlstCrypto::aggregate(
                    MempoolNetMessage::DataVote(dp_hash.clone(), le.cumul_size),
                    &filtered_signatures.iter().collect::<Vec<_>>(),
                ) {
                    Ok(poda) => poda,
                    Err(e) => {
                        error!(
                        "Could not aggregate signatures for validator {} and data proposal hash {}: {}",
                        validator, dp_hash, e
                    );
                        break;
                    }
                };
                return Ok(Some((dp_hash.clone(), le.cumul_size, poda.signature)));
            }
        }

        Ok(None)
    }

    fn new_cut(&self, staking: &Staking, previous_cut: &Cut) -> Result<Cut> {
        let validator_last_cut: HashMap<ValidatorPublicKey, (DataProposalHash, PoDA)> =
            previous_cut
                .iter()
                .map(|(v, dp, _, poda)| (v.clone(), (dp.clone(), poda.clone())))
                .collect();

        // For each validator, we get the last validated car and put it in the cut
        let mut cut: Cut = vec![];
        let bonded_validators = staking.bonded();
        for validator in bonded_validators.iter() {
            if let Some((dp_hash, cumul_size, poda)) =
                self.get_latest_car(validator, staking, validator_last_cut.get(validator))?
            {
                cut.push((validator.clone(), dp_hash, cumul_size, poda));
            }
        }
        Ok(cut)
    }

    // TODO: renommer en store_and_sign ou alors refaire la logique de storage pour rendre explicite la gestion de signatures
    fn store_data_proposal(
        &mut self,
        crypto: &BlstCrypto,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
    ) -> Result<LaneBytesSize> {
        // Add DataProposal to validator's lane
        let data_proposal_hash = data_proposal.hash();

        let dp_size = data_proposal.estimate_size();
        let lane_size = self.get_lane_size(validator)?;
        let cumul_size = lane_size + dp_size;

        let msg = MempoolNetMessage::DataVote(data_proposal_hash.clone(), cumul_size);
        let signatures = vec![crypto.sign(msg)?];

        self.put(
            validator.clone(),
            LaneEntry {
                data_proposal,
                cumul_size,
                signatures,
            },
        )?;
        Ok(cumul_size)
    }
    fn on_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> Result<(DataProposalVerdict, Option<LaneBytesSize>)> {
        // Check that data_proposal is not empty
        if data_proposal.txs.is_empty() {
            return Ok((DataProposalVerdict::Empty, None));
        }

        let validator_lane_size = self.get_lane_size(validator)?;

        // ALREADY STORED

        // Already present data proposal (just resend a vote)
        if self.contains(validator, &data_proposal.hash()) {
            return Ok((DataProposalVerdict::Vote, Some(validator_lane_size)));
        }

        // LEGIT DATA PROPOSAL

        // data proposal parent hash needs to match the lane tip of that validator
        let last_known_hash = self.get_lane_tip(validator);
        let valid_next_data_proposal =
            data_proposal.parent_data_proposal_hash.as_ref() == last_known_hash;

        if valid_next_data_proposal {
            return Ok((DataProposalVerdict::Process, None));
        }

        // UNKNOWN PARENT

        // DataProposal's parent is unknown
        if let Some(ref dp_parent_hash) = data_proposal.parent_data_proposal_hash {
            if !self.contains(validator, dp_parent_hash) {
                // Get the last known parent hash in order to get all the next ones
                return Ok((
                    DataProposalVerdict::Wait(Some(dp_parent_hash.clone())),
                    None,
                ));
            }
        }

        // FORKS
        // Based on previous conditions; we are sure that:
        // - the DataProposal's parent is known
        // - the DataProposal's parent is not the last known hash
        // Hence, this data proposal is a fork

        warn!(
            "DataProposal cannot be handled: {:?}",
            data_proposal.clone()
        );

        Ok((DataProposalVerdict::Refuse, None))
    }

    fn process_data_proposal(
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
        Self::remove_proofs(data_proposal);

        DataProposalVerdict::Vote
    }

    fn get_lane_entries_between_hashes(
        &self,
        validator: &ValidatorPublicKey,
        from_data_proposal_hash: Option<&DataProposalHash>,
        to_data_proposal_hash: Option<&DataProposalHash>,
    ) -> Result<Vec<LaneEntry>> {
        // If no dp hash is provided, we use the tip of the lane
        let mut dp_hash: DataProposalHash = match to_data_proposal_hash {
            Some(hash) => hash.clone(),
            None => match self.get_lane_tip(validator) {
                Some(dp_hash) => dp_hash.clone(),
                None => {
                    return Ok(vec![]);
                }
            },
        };
        let mut entries = vec![];
        while Some(&dp_hash) != from_data_proposal_hash {
            let lane_entry = self.get_by_hash(validator, &dp_hash)?;
            match lane_entry {
                Some(lane_entry) => {
                    entries.insert(0, lane_entry.clone());
                    if let Some(parent_dp_hash) = lane_entry.data_proposal.parent_data_proposal_hash
                    {
                        dp_hash = parent_dp_hash;
                    } else {
                        break;
                    }
                }
                None => {
                    bail!("Local lane is incomplete: could not find DP {}", dp_hash);
                }
            }
        }

        Ok(entries)
    }

    fn get_lane_size(&self, validator: &ValidatorPublicKey) -> Result<LaneBytesSize> {
        if let Some(latest_data_proposal_hash) = self.get_lane_tip(validator) {
            self.get_by_hash(validator, latest_data_proposal_hash)?
                .map_or_else(
                    || Ok(LaneBytesSize::default()),
                    |entry| Ok(entry.cumul_size),
                )
        } else {
            Ok(LaneBytesSize::default())
        }
    }

    // Add lane entries to validator"s lane
    fn add_missing_lane_entries(
        &mut self,
        validator: &ValidatorPublicKey,
        lane_entries: Vec<LaneEntry>,
    ) -> Result<()> {
        debug!("Trying to add missing lane entries on validator ({validator})'s lane");
        for lane_entry in lane_entries {
            self.put(validator.clone(), lane_entry)?;
        }
        Ok(())
    }

    /// Creates and saves a new DataProposal if there are pending transactions
    fn new_data_proposal(&mut self, crypto: &BlstCrypto, txs: Vec<Transaction>) -> Result<()> {
        if txs.is_empty() {
            return Ok(());
        }

        let validator_key = self.id().clone();

        // Create new data proposal
        let data_proposal = DataProposal {
            parent_data_proposal_hash: self.get_lane_tip(&validator_key).cloned(),
            txs,
        };

        debug!(
            "Creating new DataProposal in local lane ({}) with {} transactions",
            self.id(),
            data_proposal.txs.len()
        );

        let v_id = self.id().clone();
        self.store_data_proposal(crypto, &v_id, data_proposal)?;

        Ok(())
    }

    fn get_lane_pending_entries(
        &self,
        validator: &ValidatorPublicKey,
        last_cut: Option<Cut>,
    ) -> Result<Vec<LaneEntry>> {
        let lane_tip = self.get_lane_tip(validator);

        let last_committed_dp_hash = match last_cut {
            Some(cut) => cut
                .iter()
                .find(|(v, _, _, _)| v == validator)
                .map(|(_, dp, _, _)| dp.clone()),
            None => None,
        };
        self.get_lane_entries_between_hashes(validator, last_committed_dp_hash.as_ref(), lane_tip)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mempool::{storage::DataProposalVerdict, storage_memory::LanesStorage, MempoolNetMessage},
        utils::crypto::{self, BlstCrypto},
    };
    use hyle_model::{
        DataProposal, DataSized, Signature, SignedByValidator, Transaction, ValidatorSignature,
    };
    use staking::state::Staking;
    use std::collections::HashMap;

    fn setup_storage(pubkey: &ValidatorPublicKey) -> LanesStorage {
        let tmp_dir = tempfile::tempdir().unwrap().into_path();
        LanesStorage::new(&tmp_dir, pubkey.clone(), HashMap::default()).unwrap()
    }

    #[test_log::test(tokio::test)]
    async fn test_put_contains_get() {
        let crypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);

        let data_proposal = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let cumul_size: LaneBytesSize = LaneBytesSize(data_proposal.estimate_size() as u64);

        let entry = LaneEntry {
            data_proposal,
            cumul_size,
            signatures: vec![],
        };
        let dp_hash = entry.data_proposal.hash();
        storage.put(pubkey.clone(), entry.clone()).unwrap();
        assert!(storage.contains(pubkey, &dp_hash));
        assert_eq!(
            storage.get_by_hash(pubkey, &dp_hash).unwrap().unwrap(),
            entry
        );
    }

    #[test_log::test(tokio::test)]
    async fn test_update() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);
        let data_proposal = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let cumul_size: LaneBytesSize = LaneBytesSize(data_proposal.estimate_size() as u64);
        let mut entry = LaneEntry {
            data_proposal,
            cumul_size,
            signatures: vec![],
        };
        let dp_hash = entry.data_proposal.hash();
        storage.put(pubkey.clone(), entry.clone()).unwrap();
        entry.signatures.push(SignedByValidator {
            msg: MempoolNetMessage::DataVote(dp_hash.clone(), cumul_size),
            signature: ValidatorSignature {
                validator: pubkey.clone(),
                signature: Signature::default(),
            },
        });
        storage.update(pubkey.clone(), entry.clone()).unwrap();
        let updated = storage.get_by_hash(pubkey, &dp_hash).unwrap().unwrap();
        assert_eq!(1, updated.signatures.len());
    }

    #[test_log::test(tokio::test)]
    async fn test_on_data_proposal() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();

        let crypto2: BlstCrypto = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let pubkey2 = crypto2.validator_pubkey();

        let mut storage = setup_storage(pubkey);
        let dp = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        // 2 send a DP to 1
        let (verdict, _) = storage.on_data_proposal(pubkey2, &dp).unwrap();
        assert_eq!(verdict, DataProposalVerdict::Empty);

        let dp = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![Transaction::default()],
        };
        let (verdict, _) = storage.on_data_proposal(pubkey2, &dp).unwrap();
        assert_eq!(verdict, DataProposalVerdict::Process);

        let dp = DataProposal {
            parent_data_proposal_hash: Some(DataProposalHash::default()),
            txs: vec![Transaction::default()],
        };
        // 2 send a DP to 1
        let (verdict, _) = storage.on_data_proposal(pubkey2, &dp).unwrap();
        assert_eq!(
            verdict,
            DataProposalVerdict::Wait(Some(DataProposalHash::default()))
        );
    }

    #[test_log::test(tokio::test)]
    async fn test_on_data_proposal_fork() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();

        let crypto2: BlstCrypto = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let pubkey2 = crypto2.validator_pubkey();

        let mut storage = setup_storage(pubkey);
        let dp = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![Transaction::default()],
        };
        let dp2 = DataProposal {
            parent_data_proposal_hash: Some(dp.hash()),
            txs: vec![Transaction::default()],
        };

        storage
            .store_data_proposal(&crypto, pubkey2, dp.clone())
            .unwrap();
        storage.store_data_proposal(&crypto, pubkey2, dp2).unwrap();

        let dp2_fork = DataProposal {
            parent_data_proposal_hash: Some(dp.hash()),
            txs: vec![Transaction::default(), Transaction::default()],
        };

        let (verdict, _) = storage.on_data_proposal(pubkey2, &dp2_fork).unwrap();
        assert_eq!(verdict, DataProposalVerdict::Refuse);
    }

    #[test_log::test(tokio::test)]
    async fn test_on_data_vote() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);

        let crypto2: BlstCrypto = crypto::BlstCrypto::new("2".to_owned()).unwrap();

        let data_proposal = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let dp_hash = data_proposal.hash();
        // 1 creates a DP
        let cumul_size = storage
            .store_data_proposal(&crypto, pubkey, data_proposal)
            .unwrap();

        let lane_entry = storage.get_by_hash(pubkey, &dp_hash).unwrap().unwrap();
        assert_eq!(1, lane_entry.signatures.len());

        // 2 votes on this DP
        let vote_msg = MempoolNetMessage::DataVote(dp_hash.clone(), cumul_size);
        let signed_msg = crypto2.sign(vote_msg).expect("Failed to sign message");

        let result = storage
            .on_data_vote(&signed_msg, &dp_hash, cumul_size)
            .unwrap();
        assert_eq!(result.0, dp_hash);
        assert_eq!(2, result.1.len());
    }

    #[test_log::test(tokio::test)]
    async fn test_on_poda_update() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);

        let crypto2: BlstCrypto = crypto::BlstCrypto::new("2".to_owned()).unwrap();
        let pubkey2 = crypto2.validator_pubkey();
        let crypto3: BlstCrypto = crypto::BlstCrypto::new("3".to_owned()).unwrap();

        let dp = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let dp_hash = dp.hash();

        // 1 stores DP in 2's lane
        let cumul_size = storage.store_data_proposal(&crypto, pubkey2, dp).unwrap();

        // 3 votes on this DP
        let vote_msg = MempoolNetMessage::DataVote(dp_hash.clone(), cumul_size);
        let signed_msg = crypto3.sign(vote_msg).expect("Failed to sign message");

        // 1 updates its lane with all signatures
        storage
            .on_poda_update(pubkey2, &dp_hash, vec![signed_msg])
            .unwrap();

        let lane_entry = storage.get_by_hash(pubkey2, &dp_hash).unwrap().unwrap();
        assert_eq!(
            2,
            lane_entry.signatures.len(),
            "{pubkey2}'s lane entry: {:?}",
            lane_entry
        );
    }

    #[test_log::test(tokio::test)]
    async fn test_get_lane_entries_between_hashes() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);
        let dp1 = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let dp2 = DataProposal {
            parent_data_proposal_hash: Some(dp1.hash()),
            txs: vec![],
        };
        let dp3 = DataProposal {
            parent_data_proposal_hash: Some(dp2.hash()),
            txs: vec![],
        };

        storage
            .store_data_proposal(&crypto, pubkey, dp1.clone())
            .unwrap();
        storage
            .store_data_proposal(&crypto, pubkey, dp2.clone())
            .unwrap();
        storage
            .store_data_proposal(&crypto, pubkey, dp3.clone())
            .unwrap();

        tracing::error!("dp1hash: {}", dp1.hash());
        tracing::error!("dp2hash: {}", dp2.hash());
        tracing::error!("dp3hash: {}", dp3.hash());

        // [start, end] == [1, 2, 3]
        let all_entries = storage
            .get_lane_entries_between_hashes(pubkey, None, None)
            .unwrap();
        assert_eq!(3, all_entries.len());

        // ]1, end] == [2, 3]
        let entries_from_1_to_end = storage
            .get_lane_entries_between_hashes(pubkey, Some(&dp1.hash()), None)
            .unwrap();
        assert_eq!(2, entries_from_1_to_end.len());
        assert_eq!(dp2, entries_from_1_to_end.first().unwrap().data_proposal);
        assert_eq!(dp3, entries_from_1_to_end.last().unwrap().data_proposal);

        // [start, 2] == [1, 2]
        let entries_from_start_to_2 = storage
            .get_lane_entries_between_hashes(pubkey, None, Some(&dp2.hash()))
            .unwrap();
        assert_eq!(2, entries_from_start_to_2.len());
        assert_eq!(dp1, entries_from_start_to_2.first().unwrap().data_proposal);
        assert_eq!(dp2, entries_from_start_to_2.last().unwrap().data_proposal);

        // ]1, 2] == [2]
        let entries_from_1_to_2 = storage
            .get_lane_entries_between_hashes(pubkey, Some(&dp1.hash()), Some(&dp2.hash()))
            .unwrap();
        assert_eq!(1, entries_from_1_to_2.len());
        assert_eq!(dp2, entries_from_1_to_2.first().unwrap().data_proposal);

        // ]1, 3] == [2, 3]
        let entries_from_1_to_3 = storage
            .get_lane_entries_between_hashes(pubkey, Some(&dp1.hash()), None)
            .unwrap();
        assert_eq!(2, entries_from_1_to_3.len());
        assert_eq!(dp2, entries_from_1_to_3.first().unwrap().data_proposal);
        assert_eq!(dp3, entries_from_1_to_3.last().unwrap().data_proposal);

        // ]1, 1[ == []
        let entries_from_1_to_1 = storage
            .get_lane_entries_between_hashes(pubkey, Some(&dp1.hash()), Some(&dp1.hash()))
            .unwrap();
        assert_eq!(0, entries_from_1_to_1.len());
    }

    #[test_log::test(tokio::test)]
    async fn test_add_missing_lane_entries() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);
        let data_proposal = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let data_proposal_hash = data_proposal.hash();
        let cumul_size: LaneBytesSize = LaneBytesSize(data_proposal.estimate_size() as u64);
        let entry = LaneEntry {
            data_proposal,
            cumul_size,
            signatures: vec![],
        };

        storage
            .add_missing_lane_entries(pubkey, vec![entry.clone()])
            .unwrap();
        assert!(storage.contains(pubkey, &data_proposal_hash));
    }

    #[test_log::test(tokio::test)]
    async fn test_new_data_proposal() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);

        let txs = vec![Transaction::default()];

        storage.new_data_proposal(&crypto, txs).unwrap();

        let tip = storage.lanes_tip.get(pubkey);
        assert!(tip.is_some());
    }

    #[test_log::test]
    fn test_lane_size() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);

        let dp1 = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![Transaction::default()],
        };

        let size = storage
            .store_data_proposal(&crypto, pubkey, dp1.clone())
            .unwrap();
        assert_eq!(size, storage.get_lane_size(pubkey).unwrap());
        assert_eq!(size.0, dp1.estimate_size() as u64);

        // Adding a new DP
        let dp2 = DataProposal {
            parent_data_proposal_hash: Some(dp1.hash()),
            txs: vec![Transaction::default()],
        };
        let size = storage
            .store_data_proposal(&crypto, pubkey, dp2.clone())
            .unwrap();
        assert_eq!(size, storage.get_lane_size(pubkey).unwrap());
        assert_eq!(size.0, (dp1.estimate_size() + dp2.estimate_size()) as u64);
    }

    #[test_log::test(tokio::test)]
    async fn test_get_lane_pending_entries() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);
        let data_proposal = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let cumul_size: LaneBytesSize = LaneBytesSize(data_proposal.estimate_size() as u64);
        let entry = LaneEntry {
            data_proposal,
            cumul_size,
            signatures: vec![],
        };
        storage.put(pubkey.clone(), entry).unwrap();
        let pending = storage.get_lane_pending_entries(pubkey, None).unwrap();
        assert_eq!(1, pending.len());
    }

    #[test_log::test(tokio::test)]
    async fn test_get_latest_car_and_new_cut() {
        let crypto: BlstCrypto = crypto::BlstCrypto::new("1".to_owned()).unwrap();
        let pubkey = crypto.validator_pubkey();
        let mut storage = setup_storage(pubkey);
        let staking = Staking::new();
        let data_proposal = DataProposal {
            parent_data_proposal_hash: None,
            txs: vec![],
        };
        let cumul_size: LaneBytesSize = LaneBytesSize(data_proposal.estimate_size() as u64);
        let entry = LaneEntry {
            data_proposal,
            cumul_size,
            signatures: vec![],
        };
        storage.put(pubkey.clone(), entry).unwrap();
        let latest = storage.get_latest_car(pubkey, &staking, None).unwrap();
        assert!(latest.is_none());

        // Force some signature for f+1 check if needed:
        // This requires more advanced stubbing of Staking if you want a real test.

        let cut = storage.new_cut(&staking, &vec![]).unwrap();
        assert_eq!(0, cut.len());
    }
}
