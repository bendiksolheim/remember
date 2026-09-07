//! Time and identity are injected everywhere else in `core` so that tests are
//! deterministic. `SystemTime::now()` and `Uuid::new_v4()` must never appear
//! outside this module.

use uuid::Uuid;

pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

pub trait IdSource: Send + Sync {
    fn new_id(&self) -> String;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

pub struct UuidSource;

impl IdSource for UuidSource {
    fn new_id(&self) -> String {
        Uuid::new_v4().to_string()
    }
}

#[cfg(any(test, feature = "testing"))]
pub struct FixedClock(pub i64);

#[cfg(any(test, feature = "testing"))]
impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0
    }
}

#[cfg(any(test, feature = "testing"))]
pub struct SeqIdSource(std::sync::atomic::AtomicU64);

#[cfg(any(test, feature = "testing"))]
impl SeqIdSource {
    pub fn new() -> Self {
        Self(std::sync::atomic::AtomicU64::new(0))
    }
}

#[cfg(any(test, feature = "testing"))]
impl Default for SeqIdSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "testing"))]
impl IdSource for SeqIdSource {
    fn new_id(&self) -> String {
        self.0
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_returns_fixed_time() {
        let clock = FixedClock(42);
        assert_eq!(clock.now(), 42);
        assert_eq!(clock.now(), 42);
    }

    #[test]
    fn seq_id_source_increments() {
        let ids = SeqIdSource::new();
        assert_eq!(ids.new_id(), "0");
        assert_eq!(ids.new_id(), "1");
        assert_eq!(ids.new_id(), "2");
    }

    #[test]
    fn seq_id_source_default_starts_at_zero() {
        let ids = SeqIdSource::default();
        assert_eq!(ids.new_id(), "0");
    }

    #[test]
    fn system_clock_returns_positive_time() {
        // `> 0` alone would pass for a mutant hard-coding e.g. `1` — compare
        // against an independent read of the real clock instead.
        let independent = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        assert!((SystemClock.now() - independent).abs() < 5);
    }

    #[test]
    fn uuid_source_generates_distinct_ids() {
        let ids = UuidSource;
        assert_ne!(ids.new_id(), ids.new_id());
    }
}
