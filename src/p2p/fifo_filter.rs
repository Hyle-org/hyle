use std::collections::{HashSet, VecDeque};

pub struct FifoFilter<K> {
    capacity: usize,
    queue: VecDeque<K>,
    map: HashSet<K>,
}

impl<K: Eq + std::hash::Hash + Clone> FifoFilter<K> {
    pub fn new(capacity: usize) -> Self {
        FifoFilter {
            capacity,
            queue: VecDeque::with_capacity(capacity),
            map: HashSet::with_capacity(capacity),
        }
    }

    pub fn set(&mut self, key: K) {
        if self.map.len() == self.capacity {
            if let Some(old_key) = self.queue.pop_front() {
                self.map.remove(&old_key);
            }
        }
        self.queue.push_back(key.clone());
        self.map.insert(key);
    }

    pub fn check(&self, key: &K) -> bool {
        self.map.contains(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_contains() {
        let mut cache = FifoFilter::new(3);

        cache.set("apple");
        cache.set("banana");
        cache.set("orange");

        assert!(cache.check(&"apple"));
        assert!(cache.check(&"banana"));
        assert!(cache.check(&"orange"));
        assert!(!cache.check(&"grape"));
    }

    #[test]
    fn test_eviction() {
        let mut cache = FifoFilter::new(3);

        cache.set("apple");
        cache.set("banana");
        cache.set("orange");

        // Adding a fourth element should evict "apple"
        cache.set("grape");

        assert!(!cache.check(&"apple"));
        assert!(cache.check(&"banana"));
        assert!(cache.check(&"orange"));
        assert!(cache.check(&"grape"));
    }

    #[test]
    fn test_eviction_order() {
        let mut cache = FifoFilter::new(3);

        cache.set("apple");
        cache.set("banana");
        cache.set("orange");

        // Adding a fourth element should evict "apple"
        cache.set("grape");
        assert!(!cache.check(&"apple"));

        // Adding a fifth element should evict "banana"
        cache.set("melon");
        assert!(!cache.check(&"banana"));

        // Adding a sixth element should evict "orange"
        cache.set("berry");
        assert!(!cache.check(&"orange"));

        assert!(cache.check(&"grape"));
        assert!(cache.check(&"melon"));
        assert!(cache.check(&"berry"));
    }
}
