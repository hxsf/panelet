// SPDX-License-Identifier: MPL-2.0

//! Bounded framebuffer ownership and foreground presentation scheduling.
//!
//! Call `finish` only after the driver has completed its power-off cleanup.
//! Times are monotonic milliseconds supplied by the host, not wall clock time.
use crate::sync::{Arc, Mutex, Weak};

pub const FRAME_BYTES: usize = 30_000;
pub const FRAME_POOL_CAPACITY: usize = 4;
pub const COALESCE_MS: u64 = 200;
/// Software policy, not a panel endurance specification.
pub const MAX_CONSECUTIVE_PARTIALS: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshKind {
    Full,
    Partial(ssd2683::PartialWindow),
}

/// Compare the complete packed pixels against the last successful physical
/// presentation. Byte bounds round x outwards to the controller's four-pixel
/// alignment, including changes to either bit of all four palette values.
fn diff_refresh(previous: &[u8], next: &[u8]) -> RefreshKind {
    let mut min_x = 100;
    let mut max_x = 0;
    let mut min_y = 300;
    let mut max_y = 0;
    for (index, (a, b)) in previous.iter().zip(next).enumerate() {
        if a != b {
            let x = index % 100;
            let y = index / 100;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
    }
    if min_x == 100 || (max_x - min_x + 1) * (max_y - min_y + 1) >= FRAME_BYTES / 2 {
        return RefreshKind::Full;
    }
    RefreshKind::Partial(
        ssd2683::PartialWindow::new(
            (min_x * 4) as u16,
            (max_x * 4 + 3) as u16,
            min_y as u16,
            max_y as u16,
        )
        .expect("fixed framebuffer bounds and byte alignment"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayError {
    PoolExhausted,
    AllocationFailed,
    StaleEpoch,
    ForeignBuffer,
    NoSubmission,
    InvalidCompletion,
}

/// Owns exactly one packed framebuffer. Device pixels explicitly require
/// PSRAM; memory pressure cannot silently consume the internal-RAM reserve.
struct PixelBuffer {
    #[cfg(target_os = "espidf")]
    data: std::ptr::NonNull<u8>,
    #[cfg(not(target_os = "espidf"))]
    data: Vec<u8>,
}
impl PixelBuffer {
    fn new() -> Result<Self, DisplayError> {
        #[cfg(target_os = "espidf")]
        {
            // SAFETY: heap_caps_malloc returns an owned byte-addressable block
            // with the requested capabilities, or null. No DEFAULT fallback.
            let ptr = unsafe {
                esp_idf_sys::heap_caps_malloc(
                    FRAME_BYTES,
                    esp_idf_sys::MALLOC_CAP_SPIRAM | esp_idf_sys::MALLOC_CAP_8BIT,
                )
            } as *mut u8;
            let data = std::ptr::NonNull::new(ptr).ok_or(DisplayError::AllocationFailed)?;
            // SAFETY: all FRAME_BYTES belong exclusively to this new allocation.
            unsafe { data.as_ptr().write_bytes(0x55, FRAME_BYTES) };
            Ok(Self { data })
        }
        #[cfg(not(target_os = "espidf"))]
        {
            let mut data = Vec::new();
            data.try_reserve_exact(FRAME_BYTES)
                .map_err(|_| DisplayError::AllocationFailed)?;
            data.resize(FRAME_BYTES, 0x55);
            Ok(Self { data })
        }
    }
}
impl std::ops::Deref for PixelBuffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        #[cfg(target_os = "espidf")]
        {
            // SAFETY: live allocation is initialized, immutable borrow prevents
            // mutation through this owner for the returned slice lifetime.
            unsafe { std::slice::from_raw_parts(self.data.as_ptr(), FRAME_BYTES) }
        }
        #[cfg(not(target_os = "espidf"))]
        {
            &self.data
        }
    }
}
impl std::ops::DerefMut for PixelBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        #[cfg(target_os = "espidf")]
        {
            // SAFETY: exclusive owner borrow guarantees no live aliased slice.
            unsafe { std::slice::from_raw_parts_mut(self.data.as_ptr(), FRAME_BYTES) }
        }
        #[cfg(not(target_os = "espidf"))]
        {
            &mut self.data
        }
    }
}
#[cfg(target_os = "espidf")]
impl Drop for PixelBuffer {
    fn drop(&mut self) {
        // SAFETY: the buffer owns this heap_caps allocation and releases it once.
        unsafe { esp_idf_sys::heap_caps_free(self.data.as_ptr().cast()) };
    }
}
// SAFETY: owned u8 storage can move between tasks. Shared references expose only
// immutable bytes, and all writes require an exclusive mutable borrow. Reference
// counts and mutexes protecting these owners are separately allocated internally.
#[cfg(target_os = "espidf")]
unsafe impl Send for PixelBuffer {}
#[cfg(target_os = "espidf")]
unsafe impl Sync for PixelBuffer {}

struct PoolState {
    allocated: usize,
    free: Vec<PixelBuffer>,
}
struct Pool(Mutex<PoolState>);
impl Pool {
    fn take(pool: &Arc<Self>) -> Result<Buffer, DisplayError> {
        let mut state = pool.0.lock().unwrap();
        let bytes = if let Some(bytes) = state.free.pop() {
            bytes
        } else if state.allocated < FRAME_POOL_CAPACITY {
            let bytes = PixelBuffer::new()?;
            state.allocated += 1;
            bytes
        } else {
            return Err(DisplayError::PoolExhausted);
        };
        Ok(Buffer {
            bytes: Some(bytes),
            pool: Arc::downgrade(pool),
        })
    }
}
struct Buffer {
    bytes: Option<PixelBuffer>,
    pool: Weak<Pool>,
}
impl Drop for Buffer {
    fn drop(&mut self) {
        if let (Some(pool), Some(bytes)) = (self.pool.upgrade(), self.bytes.take()) {
            pool.0.lock().unwrap().free.push(bytes);
        }
    }
}

/// Invocation-owned mutable pixels. Revoke a Canvas by dropping its working
/// frame from the invocation's shared cell; stale script handles must not own it.
pub struct WorkingFrame {
    epoch: u64,
    buffer: Buffer,
}
impl WorkingFrame {
    pub fn pixels(&self) -> &[u8] {
        self.buffer.bytes.as_ref().unwrap()
    }
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        self.buffer.bytes.as_mut().unwrap()
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// Immutable pooled pixels. Cloning only clones a small reference-counted header.
#[derive(Clone)]
pub struct FrameSnapshot(Arc<Buffer>);
impl FrameSnapshot {
    pub fn pixels(&self) -> &[u8] {
        self.0.bytes.as_ref().unwrap()
    }
}

pub struct Presentation {
    pub epoch: u64,
    pub submission_id: u64,
    pub frame: FrameSnapshot,
    pub refresh: RefreshKind,
    fingerprint: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionState {
    Idle,
    Pending,
    Presenting,
    Presented,
    Unchanged,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayStatus {
    pub epoch: u64,
    pub submission_id: Option<u64>,
    pub state: SubmissionState,
    /// May be true while the current epoch has no submission: an old frame is
    /// already on the wire and is allowed to finish safely.
    pub refreshing: bool,
    pub error: Option<String>,
    pub refresh: Option<RefreshKind>,
}
struct Accepted {
    id: u64,
    frame: FrameSnapshot,
    fingerprint: [u8; 32],
}
struct State {
    epoch: u64,
    next_id: u64,
    latest: Option<Accepted>,
    inherit_latest: bool,
    pending_since: Option<u64>,
    inflight: Option<(u64, u64)>,
    shown_fingerprint: Option<[u8; 32]>,
    // One additional PSRAM buffer, outside the four submission buffers: retaining
    // a pooled snapshot would starve submissions while a refresh is in flight.
    shown_pixels: Option<PixelBuffer>,
    shown_epoch: u64,
    consecutive_partials: u8,
    refresh: Option<RefreshKind>,
    status: SubmissionState,
    error: Option<String>,
}

/// One broker per physical screen; clones share all queues and the four-buffer
/// pool. Do not hold a hardware lock when submitting a frame.
#[derive(Clone)]
pub struct DisplayBroker {
    pool: Arc<Pool>,
    state: Arc<Mutex<State>>,
}
impl Default for DisplayBroker {
    fn default() -> Self {
        Self::new()
    }
}
impl DisplayBroker {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(Pool(Mutex::new(PoolState {
                allocated: 0,
                free: Vec::with_capacity(FRAME_POOL_CAPACITY),
            }))),
            state: Arc::new(Mutex::new(State {
                epoch: 1,
                next_id: 0,
                latest: None,
                inherit_latest: false,
                pending_since: None,
                inflight: None,
                shown_fingerprint: None,
                shown_pixels: None,
                shown_epoch: 0,
                consecutive_partials: 0,
                refresh: None,
                status: SubmissionState::Idle,
                error: None,
            })),
        }
    }
    pub fn epoch(&self) -> u64 {
        self.state.lock().unwrap().epoch
    }
    /// Explicit foreground/configuration/mode transition. An ordinary VM error
    /// must not call this: accepted snapshots survive such an error.
    pub fn begin_epoch(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.epoch = state.epoch.checked_add(1).expect("display epoch exhausted");
        state.latest = None;
        state.inherit_latest = false;
        state.pending_since = None;
        state.refresh = None;
        state.status = SubmissionState::Idle;
        state.error = None;
        state.epoch
    }
    /// A faulted VM's replacement starts from white without cancelling any
    /// already accepted presentation. Revoke old working handles before calling.
    pub fn reset_canvas_base(&self) {
        self.state.lock().unwrap().inherit_latest = false;
    }
    pub fn acquire_working(&self, epoch: u64) -> Result<WorkingFrame, DisplayError> {
        let state = self.state.lock().unwrap();
        if state.epoch != epoch {
            return Err(DisplayError::StaleEpoch);
        }
        let mut buffer = Pool::take(&self.pool)?;
        if let Some(latest) = state.latest.as_ref().filter(|_| state.inherit_latest) {
            buffer
                .bytes
                .as_mut()
                .unwrap()
                .copy_from_slice(latest.frame.pixels());
        } else {
            buffer.bytes.as_mut().unwrap().fill(0x55);
        }
        Ok(WorkingFrame { epoch, buffer })
    }
    /// Acceptance is independent of eventual physical presentation. Caller may
    /// continue editing its working frame immediately after this returns.
    pub fn submit(
        &self,
        epoch: u64,
        working: &WorkingFrame,
        now_ms: u64,
    ) -> Result<u64, DisplayError> {
        let mut state = self.state.lock().unwrap();
        if state.epoch != epoch || working.epoch != epoch {
            return Err(DisplayError::StaleEpoch);
        }
        if !Weak::ptr_eq(&working.buffer.pool, &Arc::downgrade(&self.pool)) {
            return Err(DisplayError::ForeignBuffer);
        }
        let mut buffer = Pool::take(&self.pool)?;
        buffer
            .bytes
            .as_mut()
            .unwrap()
            .copy_from_slice(working.pixels());
        let fingerprint = fingerprint(working.pixels());
        state.next_id = state
            .next_id
            .checked_add(1)
            .expect("submission id exhausted");
        let id = state.next_id;
        state.latest = Some(Accepted {
            id,
            frame: FrameSnapshot(Arc::new(buffer)),
            fingerprint,
        });
        state.inherit_latest = true;
        state.pending_since.get_or_insert(now_ms);
        state.status = SubmissionState::Pending;
        state.refresh = None;
        state.error = None;
        Ok(id)
    }
    /// Returns at most one frame for the driver. An unchanged frame is consumed
    /// internally and reflected in status without touching the display hardware.
    pub fn poll(&self, now_ms: u64) -> Option<Presentation> {
        let mut state = self.state.lock().unwrap();
        if state.inflight.is_some() {
            return None;
        }
        let since = state.pending_since?;
        if now_ms.saturating_sub(since) < COALESCE_MS {
            return None;
        }
        state.pending_since = None;
        let latest = state.latest.as_ref()?;
        if state.shown_fingerprint == Some(latest.fingerprint) {
            state.refresh = None;
            state.status = SubmissionState::Unchanged;
            return None;
        }
        let presentation = Presentation {
            epoch: state.epoch,
            submission_id: latest.id,
            frame: latest.frame.clone(),
            fingerprint: latest.fingerprint,
            refresh: match state.shown_pixels.as_ref() {
                Some(previous)
                    if state.shown_fingerprint.is_some()
                        && state.shown_epoch == state.epoch
                        && state.consecutive_partials < MAX_CONSECUTIVE_PARTIALS =>
                {
                    diff_refresh(previous, latest.frame.pixels())
                }
                _ => RefreshKind::Full,
            },
        };
        state.refresh = Some(presentation.refresh);
        state.inflight = Some((presentation.epoch, presentation.submission_id));
        state.status = SubmissionState::Presenting;
        Some(presentation)
    }
    /// `Ok` means both the physical refresh AND safe driver cleanup succeeded.
    /// A failure parks the latest target until a new submission or explicit retry.
    pub fn finish(
        &self,
        presentation: Presentation,
        result: Result<(), String>,
    ) -> Result<(), DisplayError> {
        let mut state = self.state.lock().unwrap();
        if state.inflight != Some((presentation.epoch, presentation.submission_id)) {
            return Err(DisplayError::InvalidCompletion);
        }
        state.inflight = None;
        match result {
            Ok(()) => {
                state.shown_fingerprint = Some(presentation.fingerprint);
                if state.shown_pixels.is_none() {
                    // Allocation failure only disables automatic partial refresh.
                    state.shown_pixels = PixelBuffer::new().ok();
                }
                if let Some(previous) = state.shown_pixels.as_mut() {
                    previous.copy_from_slice(presentation.frame.pixels());
                }
                state.shown_epoch = presentation.epoch;
                state.consecutive_partials = match presentation.refresh {
                    RefreshKind::Full => 0,
                    RefreshKind::Partial(_) => state.consecutive_partials.saturating_add(1),
                };
                if state.epoch == presentation.epoch
                    && state.latest.as_ref().map(|v| v.id) == Some(presentation.submission_id)
                {
                    state.status = SubmissionState::Presented;
                    state.error = None;
                }
            }
            Err(error) => {
                state.shown_fingerprint = None;
                // A retired epoch must not park a newer foreground's submission.
                if state.epoch == presentation.epoch {
                    state.pending_since = None;
                    state.status = SubmissionState::Failed;
                    state.error = Some(error);
                }
            }
        }
        Ok(())
    }
    pub fn retry(&self, now_ms: u64) -> Result<(), DisplayError> {
        let mut state = self.state.lock().unwrap();
        if state.latest.is_none() {
            return Err(DisplayError::NoSubmission);
        }
        state.pending_since.get_or_insert(now_ms);
        state.status = SubmissionState::Pending;
        state.refresh = None;
        state.error = None;
        Ok(())
    }
    pub fn status(&self) -> DisplayStatus {
        let state = self.state.lock().unwrap();
        DisplayStatus {
            epoch: state.epoch,
            submission_id: state.latest.as_ref().map(|v| v.id),
            state: state.status,
            refreshing: state.inflight.is_some(),
            error: state.error.clone(),
            refresh: state.refresh,
        }
    }
    /// Call if a diagnostic/status renderer changes the physical screen outside
    /// this broker, so a later identical widget frame is not incorrectly skipped.
    pub fn invalidate_presented(&self) {
        self.state.lock().unwrap().shown_fingerprint = None;
    }
    pub fn pool_usage(&self) -> (usize, usize) {
        let state = self.pool.0.lock().unwrap();
        (state.allocated - state.free.len(), state.allocated)
    }
}
fn fingerprint(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn white_base_immutable_submissions_and_unflushed_discard() {
        let broker = DisplayBroker::new();
        let epoch = broker.epoch();
        let mut work = broker.acquire_working(epoch).unwrap();
        assert!(work.pixels().iter().all(|b| *b == 0x55));
        work.pixels_mut()[0] = 42;
        broker.submit(epoch, &work, 0).unwrap();
        work.pixels_mut()[0] = 99;
        let show = broker.poll(200).unwrap();
        assert_eq!(show.frame.pixels()[0], 42);
        broker.finish(show, Ok(())).unwrap();
        drop(work);
        assert_eq!(broker.acquire_working(epoch).unwrap().pixels()[0], 42);
    }
    #[test]
    fn fixed_window_and_latest_while_busy() {
        let b = DisplayBroker::new();
        let e = b.epoch();
        let mut w = b.acquire_working(e).unwrap();
        b.submit(e, &w, 0).unwrap();
        w.pixels_mut()[0] = 1;
        let second = b.submit(e, &w, 190).unwrap();
        assert!(b.poll(199).is_none());
        let show = b.poll(200).unwrap();
        assert_eq!(show.submission_id, second);
        w.pixels_mut()[0] = 2;
        b.submit(e, &w, 201).unwrap();
        w.pixels_mut()[0] = 3;
        let last = b.submit(e, &w, 202).unwrap();
        assert!(b.poll(1000).is_none());
        assert!(b.pool_usage().1 <= 4);
        b.finish(show, Ok(())).unwrap();
        assert_eq!(b.poll(1000).unwrap().submission_id, last);
    }
    #[test]
    fn pool_bounded_and_drop_reclaims() {
        let b = DisplayBroker::new();
        let e = b.epoch();
        let mut held = (0..4)
            .map(|_| b.acquire_working(e).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(
            b.acquire_working(e),
            Err(DisplayError::PoolExhausted)
        ));
        held.pop();
        assert!(b.acquire_working(e).is_ok());
        drop(held);
        assert_eq!(b.pool_usage(), (0, 4));
    }
    #[test]
    fn failures_park_latest_and_retry_even_identical() {
        let b = DisplayBroker::new();
        let e = b.epoch();
        let mut w = b.acquire_working(e).unwrap();
        b.submit(e, &w, 0).unwrap();
        let show = b.poll(200).unwrap();
        b.finish(show, Ok(())).unwrap();
        b.submit(e, &w, 300).unwrap();
        assert!(b.poll(500).is_none());
        assert_eq!(b.status().state, SubmissionState::Unchanged);
        w.pixels_mut()[0] = 3;
        b.submit(e, &w, 600).unwrap();
        let show = b.poll(800).unwrap();
        w.pixels_mut()[0] = 4;
        b.submit(e, &w, 801).unwrap();
        b.finish(show, Err("cleanup failed".into())).unwrap();
        assert!(b.poll(9999).is_none());
        assert_eq!(b.status().state, SubmissionState::Failed);
        b.retry(10000).unwrap();
        assert!(b.poll(10199).is_none());
        let show = b.poll(10200).unwrap();
        assert_eq!(show.frame.pixels()[0], 4);
        b.finish(show, Ok(())).unwrap();
    }
    #[test]
    fn switch_discards_old_pending_but_allows_inflight_completion() {
        let b = DisplayBroker::new();
        let old = b.epoch();
        let w = b.acquire_working(old).unwrap();
        b.submit(old, &w, 0).unwrap();
        let show = b.poll(200).unwrap();
        b.submit(old, &w, 201).unwrap();
        let new = b.begin_epoch();
        assert_eq!(b.submit(old, &w, 300), Err(DisplayError::StaleEpoch));
        assert!(b.poll(1000).is_none());
        assert_eq!(b.status().state, SubmissionState::Idle);
        let mut fresh = b.acquire_working(new).unwrap();
        fresh.pixels_mut()[0] = 12;
        b.submit(new, &fresh, 300).unwrap();
        b.finish(show, Err("retired failure".into())).unwrap();
        assert_eq!(b.poll(1000).unwrap().frame.pixels()[0], 12);
    }
    #[test]
    fn repeated_busy_submissions_and_failed_retry_never_exceed_four_buffers() {
        let b = DisplayBroker::new();
        let e = b.epoch();
        let mut w = b.acquire_working(e).unwrap();
        b.submit(e, &w, 0).unwrap();
        let first = b.poll(200).unwrap();
        for i in 0..1000 {
            w.pixels_mut()[0] = (i % 256) as u8;
            b.submit(e, &w, 201 + i).unwrap();
            assert!(b.pool_usage().0 <= 3);
            assert!(b.pool_usage().1 <= FRAME_POOL_CAPACITY);
        }
        b.finish(first, Err("BUSY timeout".into())).unwrap();
        assert_eq!(b.pool_usage().0, 2);
        b.retry(2000).unwrap();
        let retry = b.poll(2200).unwrap();
        assert_eq!(retry.frame.pixels()[0], (999 % 256) as u8);
        b.finish(retry, Ok(())).unwrap();
        drop(w);
        b.begin_epoch();
        assert_eq!(b.pool_usage(), (0, FRAME_POOL_CAPACITY));
    }

    #[test]
    fn reset_canvas_base_keeps_accepted_and_inflight_frames_but_starts_white() {
        let b = DisplayBroker::new();
        let e = b.epoch();
        let mut old = b.acquire_working(e).unwrap();
        old.pixels_mut()[0] = 7;
        let first = b.submit(e, &old, 0).unwrap();
        drop(old);
        b.reset_canvas_base();
        assert_eq!(b.status().submission_id, Some(first));
        let presented = b.poll(200).unwrap();
        assert_eq!(presented.frame.pixels()[0], 7);
        let mut fresh = b.acquire_working(e).unwrap();
        assert!(fresh.pixels().iter().all(|byte| *byte == 0x55));
        fresh.pixels_mut()[1] = 8;
        let second = b.submit(e, &fresh, 201).unwrap();
        drop(fresh);
        let inherited = b.acquire_working(e).unwrap();
        assert_eq!(inherited.pixels()[0], 0x55);
        assert_eq!(inherited.pixels()[1], 8);
        drop(inherited);
        b.reset_canvas_base();
        assert!(b.status().refreshing);
        assert_eq!(b.status().submission_id, Some(second));
        let white = b.acquire_working(e).unwrap();
        assert!(white.pixels().iter().all(|byte| *byte == 0x55));
        assert!(b.pool_usage().1 <= FRAME_POOL_CAPACITY);
        b.finish(presented, Ok(())).unwrap();
        let next = b.poll(401).unwrap();
        assert_eq!(next.submission_id, second);
        assert_eq!(next.frame.pixels()[1], 8);
        b.finish(next, Ok(())).unwrap();
        drop(white);
        b.begin_epoch();
        assert_eq!(b.pool_usage().0, 0);
    }

    #[test]
    fn diagnostic_invalidation_and_foreign_pool() {
        let b = DisplayBroker::new();
        let e = b.epoch();
        let w = b.acquire_working(e).unwrap();
        assert_eq!(
            DisplayBroker::new().submit(e, &w, 0),
            Err(DisplayError::ForeignBuffer)
        );
        b.submit(e, &w, 0).unwrap();
        b.finish(b.poll(200).unwrap(), Ok(())).unwrap();
        b.invalidate_presented();
        b.submit(e, &w, 300).unwrap();
        assert!(b.poll(500).is_some());
    }
    #[test]
    fn diff_bounds_cover_each_palette_bit_and_screen_edges() {
        let white = vec![0x55; FRAME_BYTES];
        for (index, pixel) in [(0, 0), (99, 3), (29900, 0), (29999, 3), (12345, 2)] {
            for color in [0, 2, 3] {
                let mut next = white.clone();
                let shift = 6 - pixel * 2;
                next[index] = (next[index] & !(3 << shift)) | (color << shift);
                let RefreshKind::Partial(w) = diff_refresh(&white, &next) else {
                    panic!("small diff");
                };
                assert_eq!(
                    (w.x_start(), w.x_end()),
                    (((index % 100) * 4) as u16, ((index % 100) * 4 + 3) as u16)
                );
                assert_eq!(
                    (w.y_start(), w.y_end()),
                    ((index / 100) as u16, (index / 100) as u16)
                );
            }
        }
        let mut next = white.clone();
        next[0] = 0;
        next[14999] = 0;
        assert_eq!(diff_refresh(&white, &next), RefreshKind::Full);
    }

    #[test]
    fn automatic_partial_uses_physical_baseline_not_intermediate_submission() {
        let b = DisplayBroker::new();
        let e = b.epoch();
        let mut w = b.acquire_working(e).unwrap();
        b.submit(e, &w, 0).unwrap();
        let first = b.poll(200).unwrap();
        assert_eq!(first.refresh, RefreshKind::Full);
        b.finish(first, Ok(())).unwrap();
        // Intermediate change is reverted before presentation; only the latest
        // target must be compared with the physically presented white frame.
        w.pixels_mut()[1] = 0;
        b.submit(e, &w, 300).unwrap();
        w.pixels_mut()[1] = 0x55;
        w.pixels_mut()[12345] = 0xff;
        b.submit(e, &w, 400).unwrap();
        let first_partial = b.poll(500).unwrap();
        assert_eq!(
            first_partial.refresh,
            RefreshKind::Partial(ssd2683::PartialWindow::new(180, 183, 123, 123).unwrap())
        );
        // Pending while in flight must be compared with that frame after success.
        w.pixels_mut()[12346] = 0xaa;
        b.submit(e, &w, 501).unwrap();
        assert!(b.poll(1000).is_none());
        b.finish(first_partial, Ok(())).unwrap();
        let second = b.poll(1000).unwrap();
        assert_eq!(
            second.refresh,
            RefreshKind::Partial(ssd2683::PartialWindow::new(184, 187, 123, 123).unwrap())
        );
        b.finish(second, Err("PTLW cleanup failed".into())).unwrap();
        assert!(b.poll(2000).is_none());
        b.retry(2000).unwrap();
        assert_eq!(b.poll(2200).unwrap().refresh, RefreshKind::Full);
    }

    #[test]
    fn periodic_full_refresh_and_diagnostic_epoch_invalidation() {
        let b = DisplayBroker::new();
        let e = b.epoch();
        let mut w = b.acquire_working(e).unwrap();
        b.submit(e, &w, 0).unwrap();
        b.finish(b.poll(200).unwrap(), Ok(())).unwrap();
        for n in 1..=9u64 {
            w.pixels_mut()[0] = n as u8;
            b.submit(e, &w, n * 300).unwrap();
            let show = b.poll(n * 300 + 200).unwrap();
            assert_eq!(matches!(show.refresh, RefreshKind::Partial(_)), n <= 8);
            b.finish(show, Ok(())).unwrap();
        }
        b.invalidate_presented();
        b.submit(e, &w, 3000).unwrap();
        let show = b.poll(3200).unwrap();
        assert_eq!(show.refresh, RefreshKind::Full);
        b.finish(show, Ok(())).unwrap();
        drop(w);
        let new = b.begin_epoch();
        let fresh = b.acquire_working(new).unwrap();
        b.submit(new, &fresh, 3300).unwrap();
        assert_eq!(b.poll(3500).unwrap().refresh, RefreshKind::Full);
    }
}
