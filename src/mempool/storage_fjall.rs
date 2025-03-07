use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::{bail, Result};
use fjall::{
    Config, Keyspace, KvSeparationOptions, PartitionCreateOptions, PartitionHandle, Slice,
};
use hyle_model::LaneId;
use tracing::info;

use crate::{
    model::{DataProposalHash, Hashed},
    utils::logger::LogMe,
};

use super::storage::{CanBePutOnTop, LaneEntry, Storage};

pub use hyle_model::LaneBytesSize;

pub struct LanesStorage {
    pub own_lane_id: LaneId,
    pub lanes_tip: BTreeMap<LaneId, (DataProposalHash, LaneBytesSize)>,
    db: Keyspace,
    pub by_hash: PartitionHandle,
}

impl Storage for LanesStorage {
    fn new(
        path: &Path,
        own_lane_id: LaneId,
        lanes_tip: BTreeMap<LaneId, (DataProposalHash, LaneBytesSize)>,
    ) -> Result<Self> {
        let db = Config::new(path)
            .blob_cache(Arc::new(fjall::BlobCache::with_capacity_bytes(
                256 * 1024 * 1024,
            )))
            .block_cache(Arc::new(fjall::BlockCache::with_capacity_bytes(
                256 * 1024 * 1024,
            )))
            .max_journaling_size(512 * 1024 * 1024)
            .max_write_buffer_size(512 * 1024 * 1024)
            .open()?;
        let by_hash = db.open_partition(
            "dp",
            PartitionCreateOptions::default()
                // Up from default 128Mb
                .with_kv_separation(
                    KvSeparationOptions::default().file_target_size(256 * 1024 * 1024),
                )
                .block_size(32 * 1024)
                .manual_journal_persist(true)
                .max_memtable_size(128 * 1024 * 1024),
        )?;

        info!("{} DP(s) available", by_hash.len()?);

        Ok(LanesStorage {
            own_lane_id,
            lanes_tip,
            db,
            by_hash,
        })
    }

    fn own_lane_id(&self) -> &LaneId {
        &self.own_lane_id
    }

    fn persist(&self) -> Result<()> {
        self.db
            .persist(fjall::PersistMode::Buffer)
            .map_err(Into::into)
    }

    fn contains(&self, lane_id: &LaneId, dp_hash: &DataProposalHash) -> bool {
        self.by_hash
            .contains_key(format!("{}:{}", lane_id, dp_hash))
            .unwrap_or(false)
    }

    fn get_by_hash(
        &self,
        lane_id: &LaneId,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<LaneEntry>> {
        let item = self
            .by_hash
            .get(format!("{}:{}", lane_id, dp_hash))
            .log_warn(format!(
                "Can't find DP {} for validator {}",
                dp_hash, lane_id
            ))?;
        item.map(decode_from_item).transpose()
    }

    fn pop(&mut self, lane_id: LaneId) -> Result<Option<(DataProposalHash, LaneEntry)>> {
        if let Some((lane_hash_tip, _)) = self.lanes_tip.get(&lane_id).cloned() {
            if let Some(lane_entry) = self.get_by_hash(&lane_id, &lane_hash_tip)? {
                self.by_hash
                    .remove(format!("{}:{}", lane_id, lane_hash_tip))?;
                self.update_lane_tip(
                    lane_id,
                    lane_entry.data_proposal.hashed(),
                    lane_entry.cumul_size,
                );
                return Ok(Some((lane_hash_tip, lane_entry)));
            }
        }
        Ok(None)
    }

    fn put(&mut self, lane_id: LaneId, lane_entry: LaneEntry) -> Result<()> {
        let dp_hash = lane_entry.data_proposal.hashed();

        if self.contains(&lane_id, &dp_hash) {
            bail!("DataProposal {} was already in lane", dp_hash);
        }

        match self.can_be_put_on_top(
            &lane_id,
            lane_entry.data_proposal.parent_data_proposal_hash.as_ref(),
        ) {
            CanBePutOnTop::No => bail!(
                "Can't store DataProposal {}, as parent is unknown ",
                dp_hash
            ),
            CanBePutOnTop::Yes => {
                // Add DataProposal to validator's lane
                self.by_hash.insert(
                    format!("{}:{}", lane_id, dp_hash),
                    encode_to_item(lane_entry.clone())?,
                )?;

                // Validator's lane tip is only updated if DP-chain is respected
                self.update_lane_tip(lane_id, dp_hash, lane_entry.cumul_size);

                Ok(())
            }
            CanBePutOnTop::Fork => {
                let last_known_hash = self.lanes_tip.get(&lane_id);
                bail!(
                    "DataProposal cannot be put in lane because it creates a fork: last dp hash {:?} while proposed parent_data_proposal_hash: {:?}",
                    last_known_hash,
                    lane_entry.data_proposal.parent_data_proposal_hash
                )
            }
        }
    }

    fn put_no_verification(&mut self, lane_id: LaneId, lane_entry: LaneEntry) -> Result<()> {
        let dp_hash = lane_entry.data_proposal.hashed();
        self.by_hash.insert(
            format!("{}:{}", lane_id, dp_hash),
            encode_to_item(lane_entry)?,
        )?;
        Ok(())
    }

    fn get_lane_ids(&self) -> impl Iterator<Item = &LaneId> {
        self.lanes_tip.keys()
    }

    fn get_lane_hash_tip(&self, lane_id: &LaneId) -> Option<&DataProposalHash> {
        self.lanes_tip.get(lane_id).map(|(hash, _)| hash)
    }

    fn get_lane_size_tip(&self, lane_id: &LaneId) -> Option<&LaneBytesSize> {
        self.lanes_tip.get(lane_id).map(|(_, size)| size)
    }

    fn update_lane_tip(
        &mut self,
        lane_id: LaneId,
        dp_hash: DataProposalHash,
        size: LaneBytesSize,
    ) -> Option<(DataProposalHash, LaneBytesSize)> {
        self.lanes_tip.insert(lane_id, (dp_hash, size))
    }

    #[cfg(test)]
    fn remove_lane_entry(&mut self, lane_id: &LaneId, dp_hash: &DataProposalHash) {
        self.by_hash
            .remove(format!("{}:{}", lane_id, dp_hash))
            .unwrap();
    }
}

fn decode_from_item(item: Slice) -> Result<LaneEntry> {
    borsh::from_slice(&item).map_err(Into::into)
}

fn encode_to_item(lane_entry: LaneEntry) -> Result<Slice> {
    borsh::to_vec(&lane_entry)
        .map(Slice::from)
        .map_err(Into::into)
}
