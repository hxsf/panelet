// SPDX-License-Identifier: MPL-2.0

//! Volatile identity of the last frame sent successfully to the panel.
use sha2::{Digest, Sha256};

#[derive(Default)]
pub struct DisplayCache {
    successful: Option<[u8; 32]>,
}

impl DisplayCache {
    pub fn matches(&self, frame: &[u8]) -> bool {
        self.successful
            .is_some_and(|digest| digest == Self::digest(frame))
    }

    /// Invalidate before any panel operation whose outcome may change pixels.
    pub fn invalidate(&mut self) {
        self.successful = None;
    }

    /// Call only after both full refresh and all safety cleanup succeeded.
    pub fn record_success(&mut self, frame: &[u8]) {
        self.successful = Some(Self::digest(frame));
    }

    fn digest(frame: &[u8]) -> [u8; 32] {
        Sha256::digest(frame).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changes_errors_other_screens_and_reboots_require_a_refresh() {
        let a = [0x55; 30_000];
        let mut b = a;
        b[29_999] = 0x54;
        let mut cache = DisplayCache::default();
        assert!(!cache.matches(&a));
        cache.record_success(&a);
        assert!(cache.matches(&a));
        assert!(!cache.matches(&b));
        cache.invalidate(); // failed/partial operation: physical content uncertain
        assert!(!cache.matches(&a));
        cache.record_success(&b); // another widget or setup screen replaced it
        assert!(!cache.matches(&a));
        assert!(cache.matches(&b));
        assert!(!DisplayCache::default().matches(&b));
    }
}
