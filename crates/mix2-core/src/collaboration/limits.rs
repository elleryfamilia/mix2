use std::sync::atomic::{AtomicU32, Ordering};

/// Per-turn consultation budget, owned by the runtime (never by the model).
/// Acquisition is atomic so concurrent consult attempts cannot exceed it.
#[derive(Debug)]
pub struct ConsultBudget {
    max: u32,
    used: AtomicU32,
}

impl ConsultBudget {
    pub fn new(max: u32) -> Self {
        Self {
            max,
            used: AtomicU32::new(0),
        }
    }

    /// Try to reserve one consultation slot. Returns the 1-based index of the
    /// acquired consultation, or `None` if the budget is exhausted.
    pub fn try_acquire(&self) -> Option<u32> {
        let mut current = self.used.load(Ordering::SeqCst);
        loop {
            if current >= self.max {
                return None;
            }
            match self.used.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Some(current + 1),
                Err(actual) => current = actual,
            }
        }
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    pub fn used(&self) -> u32 {
        self.used.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn budget_counts_up_and_stops() {
        let b = ConsultBudget::new(2);
        assert_eq!(b.try_acquire(), Some(1));
        assert_eq!(b.try_acquire(), Some(2));
        assert_eq!(b.try_acquire(), None);
        assert_eq!(b.used(), 2);
    }

    #[test]
    fn concurrent_acquire_never_exceeds_budget() {
        let b = Arc::new(ConsultBudget::new(2));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let b = Arc::clone(&b);
                std::thread::spawn(move || b.try_acquire())
            })
            .collect();
        let granted = handles
            .into_iter()
            .filter_map(|h| h.join().unwrap())
            .count();
        assert_eq!(granted, 2);
    }

    #[test]
    fn zero_budget_grants_nothing() {
        let b = ConsultBudget::new(0);
        assert_eq!(b.try_acquire(), None);
    }
}
