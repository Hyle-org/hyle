use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{bail, Result};
use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle, Slice};
use tracing::info;

use crate::{
    model::{DataProposalHash, Hashable, ValidatorPublicKey},
    utils::logger::LogMe,
};

use super::storage::{CanBePutOnTop, LaneEntry, Storage};

pub use hyle_model::LaneBytesSize;

pub struct LanesStorage {
    pub id: ValidatorPublicKey,
    pub lanes_tip: HashMap<ValidatorPublicKey, DataProposalHash>,
    db: Keyspace,
    pub by_hash: PartitionHandle,
}

impl Storage for LanesStorage {
    fn new(
        path: &Path,
        id: ValidatorPublicKey,
        lanes_tip: HashMap<ValidatorPublicKey, DataProposalHash>,
    ) -> Result<Self> {
        let db = Config::new(path)
            .blob_cache(Arc::new(fjall::BlobCache::with_capacity_bytes(
                5 * 1024 * 1024 * 1024, // 5Go cache
            )))
            .block_cache(Arc::new(fjall::BlockCache::with_capacity_bytes(
                5 * 1024 * 1024 * 1024, // 5Go cache
            )))
            .open()?;
        let by_hash = db.open_partition(
            "dp",
            PartitionCreateOptions::default()
                .block_size(56 * 1024)
                .manual_journal_persist(true)
                .max_memtable_size(128 * 1024 * 1024),
        )?;

        info!("{} DP(s) available", by_hash.len()?);

        Ok(LanesStorage {
            id,
            lanes_tip,
            db,
            by_hash,
        })
    }

    fn id(&self) -> &ValidatorPublicKey {
        &self.id
    }

    fn persist(&self) -> Result<()> {
        self.db
            .persist(fjall::PersistMode::Buffer)
            .map_err(Into::into)
    }

    fn contains(&self, validator_key: &ValidatorPublicKey, dp_hash: &DataProposalHash) -> bool {
        self.by_hash
            .contains_key(format!("{}:{}", validator_key, dp_hash))
            .unwrap_or(false)
    }

    fn get_by_hash(
        &self,
        validator_key: &ValidatorPublicKey,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<LaneEntry>> {
        let item = self
            .by_hash
            .get(format!("{}:{}", validator_key, dp_hash))
            .log_warn(format!(
                "Can't find DP {} for validator {}",
                dp_hash, validator_key
            ))?;
        item.map(decode_from_item).transpose()
    }

    fn pop(
        &mut self,
        validator: ValidatorPublicKey,
    ) -> Result<Option<(DataProposalHash, LaneEntry)>> {
        if let Some(lane_tip) = self.lanes_tip.get(&validator).cloned() {
            if let Some(lane_entry) = self.get_by_hash(&validator, &lane_tip)? {
                self.by_hash.remove(format!("{}:{}", validator, lane_tip))?;
                self.update_lane_tip(validator, lane_entry.data_proposal.hash());
                return Ok(Some((lane_tip, lane_entry)));
            }
        }
        Ok(None)
    }

    fn put(&mut self, validator: ValidatorPublicKey, lane_entry: LaneEntry) -> Result<()> {
        match self.can_be_put_on_top(
            &validator,
            lane_entry.data_proposal.parent_data_proposal_hash.as_ref(),
        ) {
            CanBePutOnTop::No => bail!(
                "Can't store DataProposal {}, as parent is unknown ",
                lane_entry.data_proposal.hash()
            ),
            CanBePutOnTop::Yes => {
                // Add DataProposal to validator's lane
                let dp_hash = lane_entry.data_proposal.hash();
                self.by_hash.insert(
                    format!("{}:{}", validator, dp_hash),
                    encode_to_item(lane_entry.clone())?,
                )?;

                // Validator's lane tip is only updated if DP-chain is respected
                self.update_lane_tip(validator, dp_hash);

                Ok(())
            }
            CanBePutOnTop::Fork => {
                let last_known_hash = self.lanes_tip.get(&validator);
                bail!(
                    "DataProposal cannot be put in lane because it creates a fork: last dp hash {:?} while proposed parent_data_proposal_hash: {:?}",
                    last_known_hash,
                    lane_entry.data_proposal.parent_data_proposal_hash
                )
            }
        }
    }

    fn put_no_verification(
        &mut self,
        validator_key: ValidatorPublicKey,
        lane_entry: LaneEntry,
    ) -> Result<()> {
        let dp_hash = lane_entry.data_proposal.hash();
        self.by_hash.insert(
            format!("{}:{}", validator_key, dp_hash),
            encode_to_item(lane_entry)?,
        )?;
        Ok(())
    }

    fn update(&mut self, validator_key: ValidatorPublicKey, lane_entry: LaneEntry) -> Result<()> {
        let dp_hash = lane_entry.data_proposal.hash();

        if !self.contains(&validator_key, &dp_hash) {
            bail!("LaneEntry does not exist");
        }
        self.by_hash.insert(
            format!("{}:{}", validator_key, dp_hash),
            encode_to_item(lane_entry.clone())?,
        )?;

        Ok(())
    }

    fn get_lane_tip(&self, validator: &ValidatorPublicKey) -> Option<&DataProposalHash> {
        self.lanes_tip.get(validator)
    }

    fn update_lane_tip(
        &mut self,
        validator: ValidatorPublicKey,
        dp_hash: DataProposalHash,
    ) -> Option<DataProposalHash> {
        self.lanes_tip.insert(validator, dp_hash)
    }
}

fn decode_from_item(item: Slice) -> Result<LaneEntry> {
    bincode::decode_from_slice(&item, bincode::config::standard())
        .map(|(b, _)| b)
        .map_err(Into::into)
}

fn encode_to_item(lane_entry: LaneEntry) -> Result<Slice> {
    bincode::encode_to_vec(lane_entry, bincode::config::standard())
        .map(Slice::from)
        .map_err(Into::into)
}
