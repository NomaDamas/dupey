//! Byte budget for the large-file extraction threads.
//!
//! Only the dedicated large-file threads block on it, never rayon workers,
//! so an extractor that uses rayon internally cannot deadlock the scan.

use std::sync::{Condvar, Mutex};

/// Counting semaphore over estimated bytes. A single cost above the cap is
/// still admitted, alone, so an oversized file never stalls the scan.
pub struct MemoryBudget {
    cap: u64,
    used: Mutex<u64>,
    freed: Condvar,
}

pub struct BudgetPermit<'a> {
    budget: &'a MemoryBudget,
    cost: u64,
}

impl MemoryBudget {
    pub fn new(cap: u64) -> Self {
        Self {
            cap,
            used: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// Block until `cost` fits under the cap, or nothing else is admitted.
    pub fn acquire(&self, cost: u64) -> BudgetPermit<'_> {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        while *used != 0 && used.saturating_add(cost) > self.cap {
            used = self.freed.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += cost;
        BudgetPermit { budget: self, cost }
    }
}

impl Drop for BudgetPermit<'_> {
    fn drop(&mut self) {
        let mut used = self.budget.used.lock().unwrap_or_else(|e| e.into_inner());
        *used -= self.cost;
        self.budget.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn cost_above_cap_is_admitted_when_idle() {
        let budget = MemoryBudget::new(10);
        drop(budget.acquire(1_000));
        // Still usable afterwards: the oversized cost was fully released.
        let _again = budget.acquire(10);
    }

    #[test]
    fn acquisition_over_cap_waits_for_release() {
        let budget = Arc::new(MemoryBudget::new(10));
        let first = budget.acquire(8);
        let admitted = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (budget, admitted) = (Arc::clone(&budget), Arc::clone(&admitted));
            std::thread::spawn(move || {
                let _permit = budget.acquire(5);
                admitted.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !admitted.load(Ordering::SeqCst),
            "8 + 5 exceeds the cap of 10; the second file must wait"
        );
        drop(first);
        waiter.join().unwrap();
        assert!(admitted.load(Ordering::SeqCst));
    }

    #[test]
    fn costs_within_cap_run_together() {
        let budget = MemoryBudget::new(10);
        let _a = budget.acquire(4);
        let _b = budget.acquire(6);
    }
}
