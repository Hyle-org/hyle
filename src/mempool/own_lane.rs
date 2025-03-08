//! Logic for processing the API inbound TXs in the mempool.

use crate::{bus::BusClientSender, mempool::InternalMempoolEvent, model::*};

use anyhow::{bail, Context, Result};
use client_sdk::tcp::TcpServerMessage;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, trace};

use super::verifiers::{verify_proof, verify_recursive_proof};
use super::{api::RestApiMessage, storage::Storage};
use super::{KnownContracts, MempoolNetMessage};

impl super::Mempool {
    pub(super) fn handle_api_message(&mut self, command: RestApiMessage) -> Result<()> {
        match command {
            RestApiMessage::NewTx(tx) => self.on_new_tx(tx).context("New Tx"),
        }
    }

    pub(super) fn handle_tcp_server_message(&mut self, command: TcpServerMessage) -> Result<()> {
        match command {
            TcpServerMessage::NewTx(tx) => self.on_new_tx(tx).context("New Tx"),
        }
    }

    fn get_last_data_prop_hash_in_own_lane(&self) -> Option<DataProposalHash> {
        self.lanes.get_lane_hash_tip(&self.own_lane_id()).cloned()
    }

    pub(super) fn own_lane_id(&self) -> LaneId {
        LaneId(self.crypto.validator_pubkey().clone())
    }

    pub(super) fn on_data_vote(
        &mut self,
        msg: &SignedByValidator<MempoolNetMessage>,
        data_proposal_hash: DataProposalHash,
    ) -> Result<()> {
        let validator = &msg.signature.validator;
        debug!("Vote from {} on own lane {}", validator, self.own_lane_id());
        let lane_id = self.own_lane_id();

        let signatures = self.lanes.add_signatures(
            &lane_id,
            &data_proposal_hash,
            std::iter::once(msg.clone()),
        )?;

        // Compute voting power of all signers to check if the DataProposal received enough votes
        let validators: Vec<ValidatorPublicKey> = signatures
            .iter()
            .map(|s| s.signature.validator.clone())
            .collect();
        let old_voting_power = self.staking.compute_voting_power(
            validators
                .iter()
                .filter(|v| *v != validator)
                .cloned()
                .collect::<Vec<_>>()
                .as_slice(),
        );
        let new_voting_power = self.staking.compute_voting_power(validators.as_slice());
        let f = self.staking.compute_f();
        // Only send the message if voting power exceeds f, 2 * f or is exactly 3 * f + 1
        // This garentees that the message is sent only once per threshold
        if old_voting_power < f && new_voting_power >= f
            || old_voting_power < 2 * f && new_voting_power >= 2 * f
            || new_voting_power == 3 * f + 1
        {
            self.broadcast_net_message(MempoolNetMessage::PoDAUpdate(
                data_proposal_hash,
                signatures,
            ))?;
        }
        Ok(())
    }

    pub(super) fn handle_data_proposal_management(&mut self) -> Result<()> {
        trace!("🌝 Handling DataProposal management");

        self.create_new_dp_if_pending()?;

        let last_cut = self
            .last_ccp
            .as_ref()
            .map(|ccp| ccp.consensus_proposal.cut.clone());

        // Check for each pending DataProposal if it has enough signatures
        let entries = self
            .lanes
            .get_lane_pending_entries(&self.own_lane_id(), last_cut)?;

        for lane_entry in entries {
            // If there's only 1 signature (=own signature), broadcast it to everyone
            if lane_entry.signatures.len() == 1 && self.staking.bonded().len() > 1 {
                debug!(
                    "🚗 Broadcast DataProposal {} ({} validators, {} txs)",
                    lane_entry.data_proposal.hashed(),
                    self.staking.bonded().len(),
                    lane_entry.data_proposal.txs.len()
                );
                self.metrics.add_data_proposal(&lane_entry.data_proposal);
                self.metrics.add_proposed_txs(&lane_entry.data_proposal);
                self.broadcast_net_message(MempoolNetMessage::DataProposal(
                    lane_entry.data_proposal.clone(),
                ))?;
            } else {
                // If None, rebroadcast it to every validator that has not yet signed it
                let validator_that_has_signed: HashSet<&ValidatorPublicKey> = lane_entry
                    .signatures
                    .iter()
                    .map(|s| &s.signature.validator)
                    .collect();

                // No PoA means we rebroadcast the DataProposal for non present voters
                let only_for: HashSet<ValidatorPublicKey> = self
                    .staking
                    .bonded()
                    .iter()
                    .filter(|pubkey| !validator_that_has_signed.contains(pubkey))
                    .cloned()
                    .collect();

                if only_for.is_empty() {
                    continue;
                }

                self.metrics.add_data_proposal(&lane_entry.data_proposal);
                self.metrics.add_proposed_txs(&lane_entry.data_proposal);
                debug!(
                    "🚗 Broadcast DataProposal {} (only for {} validators, {} txs)",
                    &lane_entry.data_proposal.hashed(),
                    only_for.len(),
                    &lane_entry.data_proposal.txs.len()
                );
                self.broadcast_only_for_net_message(
                    only_for,
                    MempoolNetMessage::DataProposal(lane_entry.data_proposal.clone()),
                )?;
            }
        }

        Ok(())
    }

    /// Creates and saves a new DataProposal if there are pending transactions
    fn create_new_dp_if_pending(&mut self) -> Result<()> {
        if self.waiting_dissemination_txs.is_empty() {
            return Ok(());
        }

        debug!(
            "🌝 Creating new data proposals with {} txs",
            self.waiting_dissemination_txs.len()
        );

        let validator_key = self.crypto.validator_pubkey().clone();

        // Create new data proposal
        let data_proposal = DataProposal::new(
            self.get_last_data_prop_hash_in_own_lane(),
            std::mem::take(&mut self.waiting_dissemination_txs),
        );

        debug!(
            "Creating new DataProposal in local lane ({}) with {} transactions",
            validator_key,
            data_proposal.txs.len()
        );

        // TODO: handle this differently
        let parent_data_proposal_hash = self
            .get_last_data_prop_hash_in_own_lane()
            .unwrap_or(DataProposalHash(self.crypto.validator_pubkey().to_string()));

        let txs_metadatas = data_proposal
            .txs
            .iter()
            .map(|tx| tx.metadata(parent_data_proposal_hash.clone()))
            .collect();

        self.bus
            .send(MempoolStatusEvent::DataProposalCreated {
                data_proposal_hash: data_proposal.hashed(),
                txs_metadatas,
            })
            .context("Sending MempoolStatusEvent DataProposalCreated")?;

        self.lanes
            .store_data_proposal(&self.crypto, &self.own_lane_id(), data_proposal)?;

        Ok(())
    }

    pub(super) fn on_new_tx(&mut self, tx: Transaction) -> Result<()> {
        // TODO: Verify fees ?

        let tx_type: &'static str = (&tx.transaction_data).into();
        trace!("Tx {} received in mempool", tx_type);

        let tx_hash = tx.hashed();

        match tx.transaction_data {
            TransactionData::Blob(ref blob_tx) => {
                debug!("Got new blob tx {}", tx_hash);
                // TODO: we should check if the registration handler contract exists.
                // TODO: would be good to not need to clone here.
                self.handle_hyle_contract_registration(blob_tx);
            }
            TransactionData::Proof(ref proof_tx) => {
                debug!(
                    "Got new proof tx {} for {}",
                    tx.hashed(),
                    proof_tx.contract_name
                );
                let kc = self.known_contracts.clone();
                self.running_tasks.spawn_blocking(move || {
                    let tx =
                        Self::process_proof_tx(kc, tx).context("Processing proof tx in blocker")?;
                    Ok(InternalMempoolEvent::OnProcessedNewTx(tx))
                });

                return Ok(());
            }
            TransactionData::VerifiedProof(ref proof_tx) => {
                debug!(
                    "Got verified proof tx {} for {}",
                    tx_hash,
                    proof_tx.contract_name.clone()
                );
            }
        }

        let tx_type: &'static str = (&tx.transaction_data).into();

        self.metrics.add_api_tx(tx_type);
        self.waiting_dissemination_txs.push(tx.clone());
        self.metrics
            .snapshot_pending_tx(self.waiting_dissemination_txs.len());

        let status_event = MempoolStatusEvent::WaitingDissemination {
            // TODO: handle this differently somehow - we need some unique identifier or we risk collisions in the DB.
            parent_data_proposal_hash: DataProposalHash(self.crypto.validator_pubkey().to_string()),
            tx,
        };

        self.bus
            .send(status_event)
            .context(format!("Sending Status event for TX {}", tx_hash))?;

        Ok(())
    }

    fn process_proof_tx(
        known_contracts: Arc<std::sync::RwLock<KnownContracts>>,
        mut tx: Transaction,
    ) -> Result<Transaction> {
        let TransactionData::Proof(proof_transaction) = tx.transaction_data else {
            bail!("Can only process ProofTx");
        };
        // Verify and extract proof
        #[allow(clippy::expect_used, reason = "not held across await")]
        let (verifier, program_id) = known_contracts
            .read()
            .expect("logic error")
            .0
            .get(&proof_transaction.contract_name)
            .context("Contract unknown")?
            .clone();

        let is_recursive = proof_transaction.contract_name.0 == "risc0-recursion";

        let (hyle_outputs, program_ids) = if is_recursive {
            let (program_ids, hyle_outputs) =
                verify_recursive_proof(&proof_transaction.proof, &verifier, &program_id)
                    .context("verify_rec_proof")?;
            (hyle_outputs, program_ids)
        } else {
            let hyle_outputs = verify_proof(&proof_transaction.proof, &verifier, &program_id)
                .context("verify_proof")?;
            (hyle_outputs, vec![program_id.clone()])
        };

        let tx_hashes = hyle_outputs
            .iter()
            .map(|ho| ho.tx_hash.clone())
            .collect::<Vec<_>>();

        std::iter::zip(&tx_hashes, std::iter::zip(&hyle_outputs, &program_ids)).for_each(
            |(blob_tx_hash, (hyle_output, program_id))| {
                debug!(
                    "Blob tx hash {} verified with hyle output {:?} and program id {}",
                    blob_tx_hash,
                    hyle_output,
                    hex::encode(&program_id.0)
                );
            },
        );

        tx.transaction_data = TransactionData::VerifiedProof(VerifiedProofTransaction {
            proof_hash: proof_transaction.proof.hashed(),
            proof_size: proof_transaction.estimate_size(),
            proof: Some(proof_transaction.proof),
            contract_name: proof_transaction.contract_name.clone(),
            is_recursive,
            proven_blobs: std::iter::zip(tx_hashes, std::iter::zip(hyle_outputs, program_ids))
                .map(
                    |(blob_tx_hash, (hyle_output, program_id))| BlobProofOutput {
                        original_proof_hash: ProofDataHash("todo?".to_owned()),
                        blob_tx_hash: blob_tx_hash.clone(),
                        hyle_output,
                        program_id,
                    },
                )
                .collect(),
        });

        Ok(tx)
    }
}

#[cfg(test)]
pub mod test {
    use core::panic;

    use super::*;
    use crate::{
        mempool::storage::LaneEntry, tests::autobahn_testing::assert_chanmsg_matches,
        utils::crypto::BlstCrypto,
    };
    use anyhow::Result;

    use crate::mempool::test::*;

    #[test_log::test(tokio::test)]
    async fn test_single_mempool_receiving_new_tx() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.submit_tx(&register_tx);

        assert_chanmsg_matches!(
            ctx.mempool_status_event_receiver,
            MempoolStatusEvent::WaitingDissemination { parent_data_proposal_hash, tx } => {
                assert_eq!(parent_data_proposal_hash, DataProposalHash(ctx.mempool.crypto.validator_pubkey().to_string()));
                assert_eq!(tx,register_tx);
            }
        );

        ctx.timer_tick()?;

        let dp_hash = ctx
            .mempool
            .lanes
            .get_lane_hash_tip(&ctx.own_lane())
            .unwrap();
        let dp = ctx
            .mempool
            .lanes
            .get_by_hash(&ctx.own_lane(), dp_hash)
            .unwrap()
            .unwrap()
            .data_proposal;

        assert_eq!(dp.txs, vec![register_tx.clone()]);

        // Assert that pending_tx has been flushed
        assert!(ctx.mempool.waiting_dissemination_txs.is_empty());

        assert_chanmsg_matches!(
            ctx.mempool_status_event_receiver,
            MempoolStatusEvent::DataProposalCreated { data_proposal_hash, txs_metadatas } => {
                assert_eq!(data_proposal_hash, dp.hashed());
                assert_eq!(txs_metadatas.len(), dp.txs.len());
            }
        );

        // Timer again with no txs
        ctx.timer_tick()?;

        assert_eq!(
            ctx.mempool.get_last_data_prop_hash_in_own_lane().unwrap(),
            dp.hashed()
        );

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_send_poda_update() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;
        let pubkey = (*ctx.mempool.crypto).clone();

        // Adding 4 other validators
        // Total voting_power = 500; f = 167 --> You need at least 2 signatures to send PoDAUpdate
        let crypto2 = BlstCrypto::new("validator2").unwrap();
        let crypto3 = BlstCrypto::new("validator3").unwrap();
        let crypto4 = BlstCrypto::new("validator4").unwrap();
        let crypto5 = BlstCrypto::new("validator5").unwrap();
        ctx.setup_node(&[pubkey, crypto2.clone(), crypto3.clone(), crypto4, crypto5]);

        let register_tx = make_register_contract_tx(ContractName::new("test1"));
        let dp = ctx.create_data_proposal(None, &[register_tx]);
        ctx.process_new_data_proposal(dp)?;
        ctx.timer_tick()?;

        let data_proposal = match ctx.assert_broadcast("DataProposal").msg {
            MempoolNetMessage::DataProposal(dp) => dp,
            _ => panic!("Expected DataProposal message"),
        };
        let size = LaneBytesSize(data_proposal.estimate_size() as u64);

        // Simulate receiving votes from other validators
        let signed_msg2 =
            crypto2.sign(MempoolNetMessage::DataVote(data_proposal.hashed(), size))?;
        let signed_msg3 =
            crypto3.sign(MempoolNetMessage::DataVote(data_proposal.hashed(), size))?;
        ctx.mempool.handle_net_message(signed_msg2)?;
        ctx.mempool.handle_net_message(signed_msg3)?;

        // Assert that PoDAUpdate message is broadcasted
        match ctx.assert_broadcast("PoDAUpdate").msg {
            MempoolNetMessage::PoDAUpdate(hash, signatures) => {
                assert_eq!(hash, data_proposal.hashed());
                assert_eq!(signatures.len(), 2);
            }
            _ => panic!("Expected PoDAUpdate message"),
        };

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_data_proposal_vote_from_unexpected_validator() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        let data_proposal = ctx.create_data_proposal(
            None,
            &[make_register_contract_tx(ContractName::new("test1"))],
        );
        let size = LaneBytesSize(data_proposal.estimate_size() as u64);

        let temp_crypto = BlstCrypto::new("temp_crypto").unwrap();
        let signed_msg =
            temp_crypto.sign(MempoolNetMessage::DataVote(data_proposal.hashed(), size))?;
        assert!(ctx
            .mempool
            .handle_net_message(SignedByValidator {
                msg: MempoolNetMessage::DataVote(data_proposal.hashed(), size),
                signature: signed_msg.signature,
            })
            .is_err());

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_data_proposal_vote() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Store the DP locally.
        let register_tx = make_register_contract_tx(ContractName::new("test1"));
        let data_proposal = ctx.create_data_proposal(None, &[register_tx.clone()]);
        ctx.process_new_data_proposal(data_proposal.clone())?;

        // Then make another validator vote on it.
        let size = LaneBytesSize(data_proposal.estimate_size() as u64);
        let data_proposal_hash = data_proposal.hashed();

        // Add new validator
        let crypto2 = BlstCrypto::new("2").unwrap();
        ctx.add_trusted_validator(crypto2.validator_pubkey());

        let signed_msg = crypto2.sign(MempoolNetMessage::DataVote(
            data_proposal_hash.clone(),
            size,
        ))?;

        ctx.mempool
            .handle_net_message(signed_msg)
            .expect("should handle net message");

        // Assert that we added the vote to the signatures
        let (
            LaneEntry {
                signatures: sig, ..
            },
            _,
        ) = ctx.last_lane_entry(&LaneId(ctx.validator_pubkey().clone()));

        assert_eq!(sig.len(), 2);
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_vote_for_unknown_data_proposal() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Add new validator
        let crypto2 = BlstCrypto::new("2").unwrap();
        ctx.add_trusted_validator(crypto2.validator_pubkey());

        let signed_msg = crypto2.sign(MempoolNetMessage::DataVote(
            DataProposalHash("non_existent".to_owned()),
            LaneBytesSize(0),
        ))?;

        assert!(ctx.mempool.handle_net_message(signed_msg).is_err());
        Ok(())
    }
}
