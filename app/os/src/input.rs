// SPDX-License-Identifier: MPL-2.0

//! GPIO sampling owns the pins and continues while the display/main loop blocks.
use panelet_rune_abi_poc::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub use panelet_core::input::{InputMode, InputPayload, QueuedInput};
use panelet_core::{input::InputRouter, ButtonLevels};

// Static storage is internal DRAM, independent of the PSRAM allocator policy.
static ROUTER: Mutex<InputRouter> = Mutex::new(InputRouter::new());
static STACK_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn start(pins: crate::board::ButtonPins) -> std::io::Result<()> {
    if STARTED.swap(true, Ordering::AcqRel) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "input sampler already started",
        ));
    }
    ROUTER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .sample(pins.levels(), 0);
    let result = std::thread::Builder::new()
        .name("panelet-input".into())
        .stack_size(4096)
        .spawn(move || {
            let start = Instant::now();
            let interval = Duration::from_millis(panelet_core::input::SAMPLE_INTERVAL_MS);
            loop {
                let tick = Instant::now();
                let levels = pins.levels();
                let now_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
                ROUTER
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .sample(levels, now_ms);
                let free =
                    unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) }
                        as u32
                        * std::mem::size_of::<esp_idf_svc::sys::StackType_t>() as u32;
                STACK_MIN.fetch_min(free, Ordering::Relaxed);
                // Never perform catch-up samples after a scheduling delay: doing so
                // would treat one GPIO observation as multiple debounce samples.
                std::thread::sleep(interval.saturating_sub(tick.elapsed()));
            }
        });
    match result {
        Ok(_handle) => Ok(()), // Task intentionally lives for this boot.
        Err(error) => {
            STARTED.store(false, Ordering::Release);
            Err(error)
        }
    }
}

/// Call for a changed foreground generation/mode, before dispatching its events.
/// Repeating the same pair is harmless. A new epoch is required on VM rebuild.
pub fn transition(epoch: u64, mode: InputMode) {
    ROUTER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .transition(epoch, mode);
}
pub fn peek(mode: InputMode) -> Option<QueuedInput> {
    ROUTER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .peek_mode(mode)
}
pub fn pop(mode: InputMode) -> Option<QueuedInput> {
    ROUTER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_mode(mode)
}
pub fn levels() -> ButtonLevels {
    ROUTER.lock().unwrap_or_else(|e| e.into_inner()).levels()
}
#[derive(Clone, Copy, Debug)]
pub struct InputStats {
    pub captured: u64,
    pub overflow: u64,
    pub pending: usize,
    pub minimum_stack_free_bytes: u32,
}
pub fn stats() -> InputStats {
    let router = ROUTER.lock().unwrap_or_else(|e| e.into_inner());
    InputStats {
        captured: router.captured(),
        overflow: router.overflow(),
        pending: router.pending(),
        minimum_stack_free_bytes: STACK_MIN.load(Ordering::Relaxed),
    }
}
