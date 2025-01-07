use std::collections::HashMap;

use crate::model::BlockHeight;
use bincode::{Decode, Encode};
use hyle_contract_sdk::TxHash;

#[derive(Default, Debug, Clone, Encode, Decode)]
pub struct Timeouts {
    by_block: HashMap<BlockHeight, Vec<TxHash>>,
}

impl Timeouts {
    pub fn drop(&mut self, at: &BlockHeight) -> Vec<TxHash> {
        self.by_block.remove(at).unwrap_or_default()
    }

    /// Set timeout for a tx.
    /// This does not check if the TX is already set to timeout at a different (or same) block.
    pub fn set(&mut self, tx: TxHash, at: BlockHeight) {
        match self.by_block.get_mut(&at) {
            Some(vec) => {
                vec.push(tx);
            }
            None => {
                self.by_block.insert(at, vec![tx]);
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn list_timeouts<'a>(t: &'a Timeouts, at: &BlockHeight) -> Option<&'a Vec<TxHash>> {
        t.by_block.get(at)
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

    #[test]
    fn timeout() {
        let mut t = Timeouts::default();
        let b1 = BlockHeight(0);
        let b2 = BlockHeight(1);
        let tx1 = TxHash::new("tx1");

        t.set(tx1.clone(), b1);

        assert_eq!(list_timeouts(&t, &b1).unwrap().len(), 1);
        assert_eq!(list_timeouts(&t, &b2), None);
        assert_eq!(get(&t, &tx1), Some(b1));

        t.set(tx1.clone(), b2);

        assert_eq!(t.drop(&b1), vec![tx1.clone()]);

        // Now this returns b2
        assert_eq!(get(&t, &tx1), Some(b2));
        assert_eq!(list_timeouts(&t, &b1), None);
        assert_eq!(list_timeouts(&t, &b2).unwrap().len(), 1);

        assert_eq!(t.drop(&b2), vec![tx1.clone()]);
        assert_eq!(get(&t, &tx1), None);
        assert_eq!(list_timeouts(&t, &b2), None);
    }
}
