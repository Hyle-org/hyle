use std::collections::HashMap;

use crate::model::BlockHeight;
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_model::TxHash;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Timeouts {
    timeout_window: BlockHeight,
    by_block: HashMap<BlockHeight, Vec<TxHash>>,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            timeout_window: BlockHeight(100),
            by_block: HashMap::new(),
        }
    }
}

impl Timeouts {
    pub fn drop(&mut self, at: &BlockHeight) -> Vec<TxHash> {
        self.by_block.remove(at).unwrap_or_default()
    }

    /// Set timeout for a tx.
    /// This does not check if the TX is already set to timeout at a different (or same) block.
    pub fn set(&mut self, tx: TxHash, block_height: BlockHeight) {
        self.by_block
            .entry(block_height + self.timeout_window)
            .or_default()
            .push(tx);
    }
}

#[cfg(test)]
pub mod tests {
    use hyle_model::TxHash;

    use super::*;

    fn list_timeouts(t: &Timeouts, at: BlockHeight) -> Option<&Vec<TxHash>> {
        t.by_block.get(&at)
    }

    pub fn get(t: &Timeouts, tx: &TxHash) -> Option<BlockHeight> {
        t.by_block.iter().find_map(|(k, v)| {
            if v.contains(tx) {
                Some(k).copied()
            } else {
                None
            }
        })
    }

    pub fn get_timeout_window(t: &Timeouts) -> BlockHeight {
        t.timeout_window
    }

    #[test]
    fn timeout() {
        let mut t = Timeouts::default();
        let b1 = BlockHeight(0);
        let b2 = BlockHeight(1);
        let tx1 = TxHash::new("tx1");

        t.set(tx1.clone(), b1);

        assert_eq!(list_timeouts(&t, b1 + t.timeout_window).unwrap().len(), 1);
        assert_eq!(list_timeouts(&t, b2 + t.timeout_window), None);
        assert_eq!(get(&t, &tx1), Some(b1 + t.timeout_window));

        t.set(tx1.clone(), b2);

        assert_eq!(t.drop(&(b1 + t.timeout_window)), vec![tx1.clone()]);

        // Now this returns b2
        assert_eq!(get(&t, &tx1), Some(b2 + t.timeout_window));
        assert_eq!(list_timeouts(&t, b1 + t.timeout_window), None);
        assert_eq!(list_timeouts(&t, b2 + t.timeout_window).unwrap().len(), 1);

        assert_eq!(t.drop(&(b2 + t.timeout_window)), vec![tx1.clone()]);
        assert_eq!(get(&t, &tx1), None);
        assert_eq!(list_timeouts(&t, b2 + t.timeout_window), None);
    }
}
