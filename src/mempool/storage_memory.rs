use std::{collections::HashMap, path::Path};

use anyhow::{bail, Result};
use hyle_model::LaneBytesSize;
use tracing::info;

use super::storage::{CanBePutOnTop, LaneEntry, Storage};
use crate::model::{DataProposalHash, Hashable, ValidatorPublicKey};

pub struct LanesStorage {
    pub id: ValidatorPublicKey,
    pub lanes_tip: HashMap<ValidatorPublicKey, (DataProposalHash, LaneBytesSize)>,
    pub by_hash: HashMap<ValidatorPublicKey, HashMap<DataProposalHash, LaneEntry>>,
}

impl Storage for LanesStorage {
    fn new(
        _path: &Path,
        id: ValidatorPublicKey,
        lanes_tip: HashMap<ValidatorPublicKey, (DataProposalHash, LaneBytesSize)>,
    ) -> Result<Self> {
        // FIXME: load from disk
        let by_hash = HashMap::default();

        info!("{} DP(s) available", by_hash.len());

        Ok(LanesStorage {
            id,
            lanes_tip,
            by_hash,
        })
    }

    fn id(&self) -> &ValidatorPublicKey {
        &self.id
    }

    fn contains(&self, validator_key: &ValidatorPublicKey, dp_hash: &DataProposalHash) -> bool {
        if let Some(lane) = self.by_hash.get(validator_key) {
            return lane.contains_key(dp_hash);
        }
        false
    }

    fn get_by_hash(
        &self,
        validator_key: &ValidatorPublicKey,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<LaneEntry>> {
        if let Some(lane) = self.by_hash.get(validator_key) {
            return Ok(lane.get(dp_hash).cloned());
        }
        bail!("Can't find validator {}", validator_key)
    }

    fn pop(
        &mut self,
        validator: ValidatorPublicKey,
    ) -> Result<Option<(DataProposalHash, LaneEntry)>> {
        if let Some((lane_tip, _)) = self.lanes_tip.get(&validator).cloned() {
            if let Some(lane) = self.by_hash.get_mut(&validator) {
                if let Some(lane_entry) = lane.remove(&lane_tip) {
                    self.update_lane_tip(
                        validator,
                        lane_entry.data_proposal.hash(),
                        lane_entry.cumul_size,
                    );
                    return Ok(Some((lane_tip.clone(), lane_entry)));
                }
            }
        }
        Ok(None)
    }

    fn put(&mut self, validator: ValidatorPublicKey, lane_entry: LaneEntry) -> Result<()> {
        let dp_hash = lane_entry.data_proposal.hash();
        match self.can_be_put_on_top(
            &validator,
            &dp_hash,
            lane_entry.data_proposal.parent_data_proposal_hash.as_ref(),
        ) {
            CanBePutOnTop::No => bail!(
                "Can't store DataProposal {}, as parent is unknown ",
                lane_entry.data_proposal.hash()
            ),
            CanBePutOnTop::Yes => {
                // Add DataProposal to validator's lane
                let size = lane_entry.cumul_size;
                self.by_hash
                    .entry(validator.clone())
                    .or_default()
                    .insert(dp_hash.clone(), lane_entry);

                // Validatoupdate_lane_tipr's lane tip is only updated if DP-chain is respected
                self.update_lane_tip(validator, dp_hash, size);

                Ok(())
            }
            CanBePutOnTop::AlreadyPresent => {
                bail!("DataProposal {} was already in lane", dp_hash);
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
        self.by_hash
            .entry(validator_key)
            .or_default()
            .insert(dp_hash, lane_entry);
        Ok(())
    }

    fn update(&mut self, validator_key: ValidatorPublicKey, lane_entry: LaneEntry) -> Result<()> {
        let dp_hash = lane_entry.data_proposal.hash();

        if !self.contains(&validator_key, &dp_hash) {
            bail!("LaneEntry does not exist");
        }

        self.by_hash
            .entry(validator_key)
            .or_default()
            .insert(dp_hash, lane_entry);

        Ok(())
    }

    fn persist(&self) -> Result<()> {
        Ok(())
    }

    fn get_lane_hash_tip(&self, validator: &ValidatorPublicKey) -> Option<&DataProposalHash> {
        self.lanes_tip.get(validator).map(|(hash, _)| hash)
    }

    fn get_lane_size_tip(&self, validator: &ValidatorPublicKey) -> Option<&LaneBytesSize> {
        self.lanes_tip.get(validator).map(|(_, size)| size)
    }

    fn update_lane_tip(
        &mut self,
        validator: ValidatorPublicKey,
        dp_hash: DataProposalHash,
        size: LaneBytesSize,
    ) -> Option<(DataProposalHash, LaneBytesSize)> {
        self.lanes_tip.insert(validator, (dp_hash, size))
    }

    #[cfg(test)]
    fn remove_lane_entry(&mut self, validator: &ValidatorPublicKey, dp_hash: &DataProposalHash) {
        if let Some(lane) = self.by_hash.get_mut(validator) {
            lane.remove(dp_hash);
        }
    }
}
