use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct MetricHistory {
    samples: VecDeque<u64>,
    capacity: usize,
}

impl MetricHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, value: u64) {
        if self.samples.len() >= self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    #[allow(dead_code)]
    pub fn samples(&self) -> &VecDeque<u64> {
        &self.samples
    }

    pub fn as_slice_vec(&self) -> Vec<u64> {
        self.samples.iter().copied().collect()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    #[allow(dead_code)]
    pub fn last(&self) -> Option<u64> {
        self.samples.back().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_capacity() {
        let mut h = MetricHistory::new(3);
        h.push(10);
        h.push(20);
        h.push(30);
        assert_eq!(h.len(), 3);
        h.push(40);
        assert_eq!(h.len(), 3);
        assert_eq!(h.as_slice_vec(), vec![20, 30, 40]);
    }

    #[test]
    fn test_last() {
        let mut h = MetricHistory::new(5);
        assert_eq!(h.last(), None);
        h.push(42);
        assert_eq!(h.last(), Some(42));
    }
}
