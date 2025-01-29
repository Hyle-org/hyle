use std::{collections::HashMap, path::Path};

use anyhow::{bail, Result};
use tracing::info;

use super::storage::{CanBePutOnTop, LaneEntry, Storage};
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

    fn pop(
        &mut self,
        validator: ValidatorPublicKey,
    ) -> Result<Option<(DataProposalHash, LaneEntry)>> {
        if let Some(lane_tip) = self.lanes_tip.get(&validator).cloned() {
            if let Some(lane_entry) = self.by_hash.remove(&format!("{}:{}", validator, lane_tip)) {
                self.update_lane_tip(validator, lane_entry.data_proposal.hash());
                return Ok(Some((lane_tip.clone(), lane_entry)));
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
                self.by_hash
                    .insert(format!("{}:{}", validator, dp_hash), lane_entry);

                // Validatoupdate_lane_tipr's lane tip is only updated if DP-chain is respected
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
        self.by_hash
            .insert(format!("{}:{}", validator_key, dp_hash), lane_entry.clone());
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
