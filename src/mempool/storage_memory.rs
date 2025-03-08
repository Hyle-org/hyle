use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use anyhow::{bail, Result};
use hyle_model::{LaneBytesSize, LaneId, Signed, SignedByValidator, ValidatorSignature};
use tracing::info;

use super::{
    storage::{CanBePutOnTop, LaneEntry, Storage},
    MempoolNetMessage,
};
use crate::model::{DataProposalHash, Hashed};

pub struct LanesStorage {
    pub lanes_tip: BTreeMap<LaneId, (DataProposalHash, LaneBytesSize)>,
    // NB: do not iterate on this one as it's unordered
    pub by_hash: HashMap<LaneId, HashMap<DataProposalHash, LaneEntry>>,
}

impl LanesStorage {
    pub fn new(
        _path: &Path,
        lanes_tip: BTreeMap<LaneId, (DataProposalHash, LaneBytesSize)>,
    ) -> Result<Self> {
        // FIXME: load from disk
        let by_hash = HashMap::default();

        info!("{} DP(s) available", by_hash.len());

        Ok(LanesStorage { lanes_tip, by_hash })
    }
}

impl Storage for LanesStorage {
    fn persist(&self) -> Result<()> {
        Ok(())
    }

    fn contains(&self, lane_id: &LaneId, dp_hash: &DataProposalHash) -> bool {
        if let Some(lane) = self.by_hash.get(lane_id) {
            return lane.contains_key(dp_hash);
        }
        false
    }

    fn get_by_hash(
        &self,
        lane_id: &LaneId,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<LaneEntry>> {
        if let Some(lane) = self.by_hash.get(lane_id) {
            return Ok(lane.get(dp_hash).cloned());
        }
        bail!("Can't find validator {}", lane_id)
    }

    fn pop(&mut self, validator: LaneId) -> Result<Option<(DataProposalHash, LaneEntry)>> {
        if let Some((lane_tip, _)) = self.lanes_tip.get(&validator).cloned() {
            if let Some(lane) = self.by_hash.get_mut(&validator) {
                if let Some(lane_entry) = lane.remove(&lane_tip) {
                    self.update_lane_tip(
                        validator,
                        lane_entry.data_proposal.hashed(),
                        lane_entry.cumul_size,
                    );
                    return Ok(Some((lane_tip.clone(), lane_entry)));
                }
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
                lane_entry.data_proposal.hashed()
            ),
            CanBePutOnTop::Yes => {
                // Add DataProposal to validator's lane
                let size = lane_entry.cumul_size;
                self.by_hash
                    .entry(lane_id.clone())
                    .or_default()
                    .insert(dp_hash.clone(), lane_entry);

                // Validatoupdate_lane_tipr's lane tip is only updated if DP-chain is respected
                self.update_lane_tip(lane_id, dp_hash, size);

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
        self.by_hash
            .entry(lane_id)
            .or_default()
            .insert(dp_hash, lane_entry);
        Ok(())
    }

    fn add_signatures<T: IntoIterator<Item = SignedByValidator<MempoolNetMessage>>>(
        &mut self,
        lane_id: &LaneId,
        dp_hash: &DataProposalHash,
        vote_msgs: T,
    ) -> Result<Vec<Signed<MempoolNetMessage, ValidatorSignature>>> {
        let Some(dp) = self
            .by_hash
            .get_mut(lane_id)
            .and_then(|lane| lane.get_mut(dp_hash))
        else {
            bail!("DataProposal {} not found in lane {}", dp_hash, lane_id);
        };

        for msg in vote_msgs {
            let MempoolNetMessage::DataVote(dph, cumul_size) = &msg.msg else {
                tracing::warn!(
                    "Received a non-DataVote message in add_signatures: {:?}",
                    msg.msg
                );
                continue;
            };
            if &dp.cumul_size != cumul_size || dp_hash != dph {
                tracing::warn!(
                    "Received a DataVote message with wrong hash or size: {:?}",
                    msg.msg
                );
                continue;
            }
            // Insert the new messages if they're not already in
            match dp
                .signatures
                .binary_search_by(|probe| probe.signature.cmp(&msg.signature))
            {
                Ok(_) => {}
                Err(pos) => dp.signatures.insert(pos, msg),
            }
        }
        Ok(dp.signatures.clone())
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
        if let Some(lane) = self.by_hash.get_mut(lane_id) {
            lane.remove(dp_hash);
        }
    }
}
