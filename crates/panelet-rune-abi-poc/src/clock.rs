// SPDX-License-Identifier: MPL-2.0

//! Monotonic host clock. Bare browser WASM has no `std::time::Instant` support.
//! The browser worker supplies `performance.now()` through one explicit import.

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use std::time::Instant;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[derive(Clone, Copy, Debug)]
pub struct Instant(f64);

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[link(wasm_import_module = "env")]
extern "C" {
    fn now_millis() -> f64;
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl Instant {
    pub fn now() -> Self {
        // The only WASM host import; it does not inspect any linear memory.
        Self(unsafe { now_millis() })
    }

    pub fn elapsed(&self) -> std::time::Duration {
        std::time::Duration::from_millis((Self::now().0 - self.0).max(0.0) as u64)
    }
}
