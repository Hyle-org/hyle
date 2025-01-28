use std::{collections::HashMap, path::Path};

use anyhow::{bail, Result};
use tracing::info;

use super::storage::{LaneEntry, Storage};
use crate::model::{DataProposalHash, Hashable, ValidatorPublicKey};

pub struct LanesStorage {
    pub id: ValidatorPublicKey,
    pub lanes_tip: HashMap<ValidatorPublicKey, DataProposalHash>,
    pub by_hash: HashMap<String, LaneEntry>,
}

impl Storage for LanesStorage {
    fn new(
        _path: &Path,
        id: ValidatorPublicKey,
        lanes_tip: HashMap<ValidatorPublicKey, DataProposalHash>,
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
        self.by_hash
            .contains_key(&format!("{}:{}", validator_key, dp_hash))
    }

    fn get_by_hash(
        &self,
        validator_key: &ValidatorPublicKey,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<LaneEntry>> {
        let item = self.by_hash.get(&format!("{}:{}", validator_key, dp_hash));
        Ok(item.cloned())
    }

    fn put(&mut self, validator_key: ValidatorPublicKey, lane_entry: LaneEntry) -> Result<()> {
        let dp_hash = lane_entry.data_proposal.hash();
        self.by_hash
            .insert(format!("{}:{}", validator_key, dp_hash), lane_entry.clone());
        tracing::info!(
            "Added new DataProposal {} to lane, size: {}",
            dp_hash,
            lane_entry.cumul_size
        );

        // Updating validator's lane tip
        if let Some(validator_tip) = self.get_lane_tip(&validator_key) {
            // If validator already has a lane, we only update the tip if DP-chain is respected
            if let Some(parent_dp_hash) = &lane_entry.data_proposal.parent_data_proposal_hash {
                if validator_tip == parent_dp_hash {
                    self.update_lane_tip(validator_key, lane_entry.data_proposal.hash());
                }
            }
        } else {
            self.update_lane_tip(validator_key, lane_entry.data_proposal.hash());
        }

        Ok(())
    }

    fn update(&mut self, validator_key: ValidatorPublicKey, lane_entry: LaneEntry) -> Result<()> {
        let dp_hash = lane_entry.data_proposal.hash();

        if !self.contains(&validator_key, &dp_hash) {
            bail!("LaneEntry does not exist");
        }
        self.by_hash
            .insert(format!("{}:{}", validator_key, dp_hash), lane_entry.clone());

        Ok(())
    }

    fn persist(&self) -> Result<()> {
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
