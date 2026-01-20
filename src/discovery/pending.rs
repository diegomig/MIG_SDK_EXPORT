use std::collections::HashSet;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use ethers::types::Address;
use log::{debug, trace, warn};

use crate::pools::Pool;

#[derive(Clone, Debug)]
pub struct PendingQueueConfig {
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub max_attempts: u32,
    pub max_batch: usize,
}

impl Default for PendingQueueConfig {
    fn default() -> Self {
        Self {
            base_delay_ms: 5_000,
            max_delay_ms: 120_000,
            max_attempts: 6,
            max_batch: 64,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PendingItem {
    pub pool: Pool,
    pub reason: String,
    pub attempt: u32,
}

#[derive(Clone, Debug)]
struct PendingEntry {
    pool: Pool,
    reason: String,
    attempt: u32,
    next_retry_at: Instant,
    _first_seen: Instant,
}

impl PendingEntry {
    fn new(pool: Pool, reason: String, attempt: u32, delay: Duration) -> Self {
        let now = Instant::now();
        Self {
            pool,
            reason,
            attempt,
            next_retry_at: now + delay,
            _first_seen: now,
        }
    }

    fn bump(&mut self, reason: &str, delay: Duration) {
        self.reason = reason.to_string();
        self.attempt = self.attempt.saturating_add(1);
        self.next_retry_at = Instant::now() + delay;
    }
}

pub struct PendingQueue {
    entries: DashMap<Address, PendingEntry>,
    config: PendingQueueConfig,
}

impl PendingQueue {
    pub fn new(config: PendingQueueConfig) -> Self {
        Self {
            entries: DashMap::new(),
            config,
        }
    }

    fn calculate_delay(&self, attempt: u32) -> Duration {
        let base = Duration::from_millis(self.config.base_delay_ms.max(1));
        let max = Duration::from_millis(self.config.max_delay_ms.max(self.config.base_delay_ms));
        let factor = 1u64 << attempt.min(16);
        let candidate = base.saturating_mul(factor as u32);
        if candidate > max {
            max
        } else {
            candidate
        }
    }

    pub fn enqueue_new(&self, pool: Pool, reason: &str) {
        let addr = pool.address();
        let delay = self.calculate_delay(0);
        trace!(
            "Queueing new pending pool {:?} (reason={}, delay={:?})",
            addr,
            reason,
            delay
        );
        self.entries
            .insert(addr, PendingEntry::new(pool, reason.to_string(), 0, delay));
    }

    pub fn requeue(&self, pool: Pool, previous_attempt: u32, reason: &str) {
        let addr = pool.address();
        if previous_attempt >= self.config.max_attempts {
            warn!(
                "Dropping pending pool {:?} after {} attempts (reason={})",
                addr, previous_attempt, reason
            );
            self.entries.remove(&addr);
            return;
        }
        let delay = self.calculate_delay(previous_attempt + 1);
        trace!(
            "Requeue pending pool {:?} attempt={} reason={} delay={:?}",
            addr,
            previous_attempt + 1,
            reason,
            delay
        );
        self.entries
            .entry(addr)
            .and_modify(|entry| entry.bump(reason, delay))
            .or_insert_with(|| {
                PendingEntry::new(pool, reason.to_string(), previous_attempt + 1, delay)
            });
    }

    pub fn pop_ready(&self, now: Instant) -> Vec<PendingItem> {
        let limit = self.config.max_batch.max(1);
        let mut ready = Vec::with_capacity(limit);
        let mut seen = HashSet::new();

        for item in self.entries.iter() {
            if ready.len() >= limit {
                break;
            }
            if item.value().next_retry_at > now {
                continue;
            }
            let addr = *item.key();
            if seen.insert(addr) {
                if let Some((_, entry)) = self.entries.remove(&addr) {
                    ready.push(PendingItem {
                        pool: entry.pool,
                        reason: entry.reason,
                        attempt: entry.attempt,
                    });
                }
            }
        }

        if !ready.is_empty() {
            debug!(
                "Popped {} pending pools for retry (total_pending={})",
                ready.len(),
                self.entries.len()
            );
        }

        ready
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
