//! Logic for processing the API inbound TXs in the mempool.

use crate::{bus::BusClientSender, mempool::InternalMempoolEvent, model::*};

use anyhow::{bail, Context, Result};
use client_sdk::tcp::TcpServerMessage;
use std::sync::Arc;
use tracing::{debug, trace};

use super::verifiers::{verify_proof, verify_recursive_proof};
use super::KnownContracts;
use super::{api::RestApiMessage, storage::Storage};

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

    /// Creates and saves a new DataProposal if there are pending transactions
    pub(super) fn create_new_dp_if_pending(&mut self) -> Result<()> {
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
    use crate::tests::autobahn_testing::assert_chanmsg_matches;
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
}
