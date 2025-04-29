use std::{collections::BTreeMap, path::Path};

use anyhow::{bail, Result};
use fjall::{
    Config, Keyspace, KvSeparationOptions, PartitionCreateOptions, PartitionHandle, Slice,
};
use hyle_model::{LaneId, Signed, SignedByValidator, ValidatorSignature};
use tracing::info;

use crate::{
    log_warn,
    model::{DataProposal, DataProposalHash, Hashed},
};

use super::{
    storage::{CanBePutOnTop, LaneEntryMetadata, Storage},
    MempoolNetMessage,
};

pub use hyle_model::LaneBytesSize;

pub struct LanesStorage {
    pub lanes_tip: BTreeMap<LaneId, (DataProposalHash, LaneBytesSize)>,
    db: Keyspace,
    pub by_hash_metadata: PartitionHandle,
    pub by_hash_data: PartitionHandle,
}

impl LanesStorage {
    pub fn new(
        path: &Path,
        lanes_tip: BTreeMap<LaneId, (DataProposalHash, LaneBytesSize)>,
    ) -> Result<Self> {
        let db = Config::new(path)
            .cache_size(256 * 1024 * 1024)
            .max_journaling_size(512 * 1024 * 1024)
            .max_write_buffer_size(512 * 1024 * 1024)
            .open()?;

        let by_hash_metadata = db.open_partition(
            "dp_metadata",
            PartitionCreateOptions::default()
                .with_kv_separation(
                    KvSeparationOptions::default().file_target_size(256 * 1024 * 1024),
                )
                .block_size(32 * 1024)
                .manual_journal_persist(true)
                .max_memtable_size(128 * 1024 * 1024),
        )?;

        let by_hash_data = db.open_partition(
            "dp_data",
            PartitionCreateOptions::default()
                .with_kv_separation(
                    KvSeparationOptions::default().file_target_size(256 * 1024 * 1024),
                )
                .block_size(32 * 1024)
                .manual_journal_persist(true)
                .max_memtable_size(128 * 1024 * 1024),
        )?;

        info!("{} DP(s) available", by_hash_metadata.len()?);

        Ok(LanesStorage {
            lanes_tip,
            db,
            by_hash_metadata,
            by_hash_data,
        })
    }
}

impl Storage for LanesStorage {
    fn persist(&self) -> Result<()> {
        self.db
            .persist(fjall::PersistMode::Buffer)
            .map_err(Into::into)
    }

    fn contains(&self, lane_id: &LaneId, dp_hash: &DataProposalHash) -> bool {
        self.by_hash_metadata
            .contains_key(format!("{}:{}", lane_id, dp_hash))
            .unwrap_or(false)
    }

    fn get_metadata_by_hash(
        &self,
        lane_id: &LaneId,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<LaneEntryMetadata>> {
        let item = log_warn!(
            self.by_hash_metadata
                .get(format!("{}:{}", lane_id, dp_hash)),
            "Can't find DP metadata {} for validator {}",
            dp_hash,
            lane_id
        )?;
        item.map(decode_metadata_from_item).transpose()
    }

    fn get_dp_by_hash(
        &self,
        lane_id: &LaneId,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<DataProposal>> {
        let item = log_warn!(
            self.by_hash_data.get(format!("{}:{}", lane_id, dp_hash)),
            "Can't find DP data {} for validator {}",
            dp_hash,
            lane_id
        )?;
        item.map(|s| {
            decode_data_proposal_from_item(s).map(|mut dp| {
                // SAFETY: we trust our own fjall storage
                unsafe {
                    dp.unsafe_set_hash(dp_hash);
                }
                dp
            })
        })
        .transpose()
    }

    fn pop(
        &mut self,
        lane_id: LaneId,
    ) -> Result<Option<(DataProposalHash, (LaneEntryMetadata, DataProposal))>> {
        if let Some((lane_hash_tip, _)) = self.lanes_tip.get(&lane_id).cloned() {
            if let Some(lane_entry) = self.get_metadata_by_hash(&lane_id, &lane_hash_tip)? {
                self.by_hash_metadata
                    .remove(format!("{}:{}", lane_id, lane_hash_tip))?;
                // Check if have the data locally after regardless - if we don't, print an error but delete metadata anyways for consistency.
                let Some(dp) = self.get_dp_by_hash(&lane_id, &lane_hash_tip)? else {
                    bail!(
                        "Can't find DP data {} for lane {} where metadata could be found",
                        lane_hash_tip,
                        lane_id
                    );
                };
                self.by_hash_data
                    .remove(format!("{}:{}", lane_id, lane_hash_tip))?;
                self.update_lane_tip(lane_id, lane_hash_tip.clone(), lane_entry.cumul_size);
                return Ok(Some((lane_hash_tip, (lane_entry, dp))));
            }
        }
        Ok(None)
    }

    fn put(
        &mut self,
        lane_id: LaneId,
        (lane_entry, data_proposal): (LaneEntryMetadata, DataProposal),
    ) -> Result<()> {
        let dp_hash = data_proposal.hashed();

        if self.contains(&lane_id, &dp_hash) {
            bail!("DataProposal {} was already in lane", dp_hash);
        }

        match self.can_be_put_on_top(&lane_id, lane_entry.parent_data_proposal_hash.as_ref()) {
            CanBePutOnTop::No => bail!(
                "Can't store DataProposal {}, as parent is unknown ",
                dp_hash
            ),
            CanBePutOnTop::Yes => {
                // Add DataProposal metadata to validator's lane
                self.by_hash_metadata.insert(
                    format!("{}:{}", lane_id, dp_hash),
                    encode_metadata_to_item(lane_entry.clone())?,
                )?;

                // Add DataProposal data to validator's lane
                self.by_hash_data.insert(
                    format!("{}:{}", lane_id, dp_hash),
                    encode_data_proposal_to_item(data_proposal)?,
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
                    lane_entry.parent_data_proposal_hash
                )
            }
        }
    }

    fn put_no_verification(
        &mut self,
        lane_id: LaneId,
        (lane_entry, data_proposal): (LaneEntryMetadata, DataProposal),
    ) -> Result<()> {
        let dp_hash = data_proposal.hashed();
        self.by_hash_metadata.insert(
            format!("{}:{}", lane_id, dp_hash),
            encode_metadata_to_item(lane_entry)?,
        )?;
        self.by_hash_data.insert(
            format!("{}:{}", lane_id, dp_hash),
            encode_data_proposal_to_item(data_proposal)?,
        )?;
        Ok(())
    }

    fn add_signatures<T: IntoIterator<Item = SignedByValidator<MempoolNetMessage>>>(
        &mut self,
        lane_id: &LaneId,
        dp_hash: &DataProposalHash,
        vote_msgs: T,
    ) -> Result<Vec<Signed<MempoolNetMessage, ValidatorSignature>>> {
        let key = format!("{}:{}", lane_id, dp_hash);
        let Some(mut lem) = log_warn!(
            self.by_hash_metadata.get(key.clone()),
            "Can't find lane entry metadata {} for lane {}",
            dp_hash,
            lane_id
        )?
        .map(decode_metadata_from_item)
        .transpose()?
        else {
            bail!(
                "Can't find lane entry metadata {} for lane {}",
                dp_hash,
                lane_id
            );
        };

        for msg in vote_msgs {
            let MempoolNetMessage::DataVote(dph, cumul_size) = &msg.msg else {
                tracing::warn!(
                    "Received a non-DataVote message in add_signatures: {:?}",
                    msg.msg
                );
                continue;
            };
            if &lem.cumul_size != cumul_size || dp_hash != dph {
                tracing::warn!(
                    "Received a DataVote message with wrong hash or size: {:?}",
                    msg.msg
                );
                continue;
            }
            // Insert the new messages if they're not already in
            match lem
                .signatures
                .binary_search_by(|probe| probe.signature.cmp(&msg.signature))
            {
                Ok(_) => {}
                Err(pos) => lem.signatures.insert(pos, msg),
            }
        }
        let signatures = lem.signatures.clone();
        self.by_hash_metadata
            .insert(key, encode_metadata_to_item(lem)?)?;
        Ok(signatures)
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

    fn get_entries_between_hashes(
        &self,
        lane_id: &LaneId,
        from_data_proposal_hash: Option<&DataProposalHash>,
        to_data_proposal_hash: Option<&DataProposalHash>,
    ) -> Result<Vec<(LaneEntryMetadata, DataProposal)>> {
        let metadata = self.get_entries_metadata_between_hashes(
            lane_id,
            from_data_proposal_hash,
            to_data_proposal_hash,
        )?;
        let mut result = Vec::with_capacity(metadata.len());
        // TODO: make these a range and use that.
        for (metadata, dp_hash) in metadata {
            let data_proposal = self.get_dp_by_hash(lane_id, &dp_hash)?.ok_or_else(|| {
                anyhow::anyhow!("Data proposal {} not found in lane {}", dp_hash, lane_id)
            })?;
            result.push((metadata, data_proposal));
        }
        Ok(result)
    }
    #[cfg(test)]
    fn remove_lane_entry(&mut self, lane_id: &LaneId, dp_hash: &DataProposalHash) {
        self.by_hash_metadata
            .remove(format!("{}:{}", lane_id, dp_hash))
            .unwrap();
        self.by_hash_data
            .remove(format!("{}:{}", lane_id, dp_hash))
            .unwrap();
    }
}

fn decode_metadata_from_item(item: Slice) -> Result<LaneEntryMetadata> {
    borsh::from_slice(&item).map_err(Into::into)
}

fn encode_metadata_to_item(metadata: LaneEntryMetadata) -> Result<Slice> {
    borsh::to_vec(&metadata)
        .map(Slice::from)
        .map_err(Into::into)
}

fn decode_data_proposal_from_item(item: Slice) -> Result<DataProposal> {
    borsh::from_slice(&item).map_err(Into::into)
}

fn encode_data_proposal_to_item(data_proposal: DataProposal) -> Result<Slice> {
    borsh::to_vec(&data_proposal)
        .map(Slice::from)
        .map_err(Into::into)
}
