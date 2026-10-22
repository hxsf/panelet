// SPDX-License-Identifier: MPL-2.0

//! Panelet Rune host implementation; preserves the versioned NOTE4C ABI identifiers.
//!
//! The ABI exposes one owned [`Std`] object.  It contains small,
//! capability-scoped facades (`time`, `http`, `screen`, `runtime`, `storage`,
//! `resource`, `fs`, `log`, `json`, `power`, `wifi`, and `device`). Platform code
//! only implements [`HostIo`]; no
//! ESP-IDF, socket, or display-driver type crosses the Rune boundary.
//!
//! Host operations are represented by Rune futures.  Native functions only
//! copy their arguments and an `Arc` to the invocation when a future is
//! created.  Capability checks, quota reservation, and the host call happen
//! from the future body, so an un-awaited future has no side effect.

use crate::clock::Instant;
use crate::sync::{Arc as InternalArc, Mutex as InternalMutex};
use core::future::Future as CoreFuture;
use core::pin::Pin;
use core::task::{Context as TaskContext, Poll};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use rune::runtime::{Future as RuneFuture, Ref, Value, VmResult};
use rune::{Any, Context, ContextError, Module};

use crate::minimal_context;
use panelet_core::solar_to_lunar;

pub const ABI_VERSION: u16 = 3;
pub const ABI_ID: &str = "note4c.rune/3";

pub const MAX_STARTED_CALLS: usize = 64;
pub const MAX_WAIT_ALL: usize = 8;
pub const MAX_CANVAS_COMMANDS: usize = 128;
pub const MAX_CANVAS_TEXT_BYTES: usize = 1024;
pub const CANVAS_WIDTH: u32 = 400;
pub const CANVAS_HEIGHT: u32 = 300;
pub const CANVAS_FRAME_BYTES: usize = 30_000;
pub const LOG_MESSAGE_MAX_BYTES: usize = 256;
pub const HTTP_URL_MAX_BYTES: usize = 512;
pub const HTTP_BODY_MAX_BYTES: usize = 16 * 1024;
pub const HTTP_REQUEST_HEADERS_MAX: usize = 16;
pub const HTTP_HEADERS_MAX_BYTES: usize = 2 * 1024;
pub const HTTP_REDIRECT_MAX: u8 = 3;
pub const HTTP_TIMEOUT_DEFAULT_MS: u64 = 10_000;
pub const HTTP_TIMEOUT_MAX_MS: u64 = 30_000;
pub const KV_KEY_MAX_BYTES: usize = 32;
pub const KV_VALUE_MAX_BYTES: usize = 1024;
pub const RESOURCE_PATH_MAX_BYTES: usize = 64;
pub const RESOURCE_COUNT_MAX: usize = 8;
pub const RESOURCE_ITEM_MAX_BYTES: usize = 1024;
pub const RESOURCE_TOTAL_MAX_BYTES: usize = 4 * 1024;
/// Maximum UTF-8 path length accepted by either filesystem namespace.
pub const FS_PATH_MAX_BYTES: usize = 128;
/// Maximum bytes in one filesystem payload. For `list`, this bounds the
/// aggregate UTF-8 bytes in the returned path names.
pub const FS_SINGLE_MAX_BYTES: usize = 4 * 1024;
/// Maximum filesystem payload bytes charged to one invocation.
pub const FS_TOTAL_MAX_BYTES: usize = 16 * 1024;
/// Maximum filesystem operations started by one invocation.
pub const FS_MAX_CALLS: usize = 32;
/// Maximum number of entries returned by one filesystem `list` operation.
pub const FS_LIST_MAX_ENTRIES: usize = 64;
pub const JSON_MAX_DEPTH: usize = 16;
pub const JSON_MAX_NODES: usize = 512;
pub const MAX_SLEEP_MS: u64 = 120_000;

// These names are the complete v2 capability vocabulary. `runtime`, `json`,
// and resource reads are local and do not consume a capability grant.
pub const CAP_TIME_READ: &str = "time.read";
pub const CAP_POWER_READ: &str = "power.read";
pub const CAP_WIFI_READ: &str = "wifi.read";
pub const CAP_DEVICE_READ: &str = "device.read";
pub const CAP_HTTP_FETCH: &str = "http.fetch";
pub const CAP_HTTP_INSECURE_TLS: &str = "http.insecure_tls";
pub const CAP_SCREEN_PRESENT: &str = "screen.present";
pub const CAP_STORAGE_KV: &str = "storage.kv";
pub const CAP_LOG_WRITE: &str = "log.write";
pub const CAP_FS_PRIVATE: &str = "fs.private";
pub const CAP_FS_SHARED_READ: &str = "fs.shared.read";
pub const CAP_FS_SHARED_WRITE: &str = "fs.shared.write";

/// Async host operation returned by every potentially blocking adapter call.
pub type HostFuture<T> = Pin<Box<dyn CoreFuture<Output = HostResult<T>> + Send>>;

fn ready_host<T: Send + 'static>(result: HostResult<T>) -> HostFuture<T> {
    Box::pin(std::future::ready(result))
}

/// The stable structured error returned in every host-facing `Result`.
#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct HostError {
    #[rune(get)]
    pub code: String,
    #[rune(get)]
    pub group: String,
    #[rune(get)]
    pub message: String,
    #[rune(get)]
    pub retryable: bool,
}

impl HostError {
    pub fn new(
        code: impl Into<String>,
        group: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.into(),
            group: group.into(),
            message: message.into(),
            retryable,
        }
    }

    pub fn capability_denied(capability: &str) -> Self {
        Self::new(
            "capability_denied",
            "capability",
            format!("capability is not granted: {capability}"),
            false,
        )
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new("invalid_argument", "argument", message, false)
    }

    pub fn unavailable(group: &str) -> Self {
        Self::new(
            "unavailable",
            group,
            format!("{group} host service is unavailable"),
            true,
        )
    }

    pub fn quota(message: impl Into<String>) -> Self {
        Self::new("quota_exceeded", "runtime", message, false)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new("timeout", "runtime", message, true)
    }

    pub fn cancelled() -> Self {
        Self::new("cancelled", "runtime", "operation was cancelled", true)
    }
}

/// Rust-side alias used by adapters. Rune sees its normal `Ok(value)` /
/// `Err(error)` enum from `std::result`.
pub type HostResult<T> = Result<T, HostError>;

/// Per-invocation device facts and sandbox policy.
#[derive(Clone)]
pub struct StdConfig {
    /// JSON object scoped to this widget, supplied by its installation settings.
    pub widget_config: String,
    pub uptime_ms: u64,
    pub unix_ms: Option<i64>,
    pub local_time: Option<LocalTimeReading>,
    pub capabilities: Vec<String>,
    /// Manifest permissions are carried to the platform adapter. The ABI
    /// validates only URL syntax and unsafe address classes; complex host
    /// patterns stay in the platform's policy implementation.
    pub host_permissions: Vec<String>,
    pub device: DeviceInfo,
    pub wall_time_ms: u64,
}

impl std::fmt::Debug for StdConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdConfig")
            .field("widget_config", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl Default for StdConfig {
    fn default() -> Self {
        Self {
            widget_config: "{}".to_owned(),
            uptime_ms: 0,
            unix_ms: None,
            local_time: None,
            capabilities: Vec::new(),
            host_permissions: Vec::new(),
            device: DeviceInfo::default(),
            wall_time_ms: 120_000,
        }
    }
}

impl StdConfig {
    pub fn unrestricted_for_tests() -> Self {
        Self {
            capabilities: all_capabilities().into_iter().map(str::to_owned).collect(),
            host_permissions: vec!["*".to_owned()],
            ..Self::default()
        }
    }
}

fn all_capabilities() -> [&'static str; 12] {
    [
        CAP_TIME_READ,
        CAP_POWER_READ,
        CAP_WIFI_READ,
        CAP_DEVICE_READ,
        CAP_HTTP_FETCH,
        CAP_HTTP_INSECURE_TLS,
        CAP_SCREEN_PRESENT,
        CAP_STORAGE_KV,
        CAP_LOG_WRITE,
        CAP_FS_PRIVATE,
        CAP_FS_SHARED_READ,
        CAP_FS_SHARED_WRITE,
    ]
}

/// Host adapter. Implementations should hand work to their own actors or
/// queues; the ABI calls these methods from a cold future's first poll.
/// Defaults make an unimplemented service fail closed.
pub trait HostIo: Send + Sync + 'static {
    fn screen_submitted(&self) {}
    #[cfg(test)]
    fn on_submission(&self, _frame: CanvasFrame) {}

    fn http_fetch(&self, _request: HttpRequest) -> HostFuture<HttpResponse> {
        ready_host(Err(HostError::unavailable("http")))
    }

    fn storage_has(&self, _key: String) -> HostFuture<bool> {
        ready_host(Err(HostError::unavailable("storage")))
    }

    fn storage_get(&self, _key: String) -> HostFuture<Option<String>> {
        ready_host(Err(HostError::unavailable("storage")))
    }

    fn storage_set(&self, _key: String, _value: String) -> HostFuture<()> {
        ready_host(Err(HostError::unavailable("storage")))
    }

    fn storage_delete(&self, _key: String) -> HostFuture<()> {
        ready_host(Err(HostError::unavailable("storage")))
    }

    fn resource_get(&self, _path: String) -> HostFuture<Option<String>> {
        ready_host(Err(HostError::unavailable("resource")))
    }

    fn resource_list(&self) -> HostFuture<Vec<String>> {
        ready_host(Err(HostError::unavailable("resource")))
    }

    // Filesystem operations receive an already-normalized relative path from
    // the ABI. Private storage has one capability for all operations. Shared
    // reads and writes are deliberately separate capabilities.
    fn fs_private_exists(&self, _path: String) -> HostFuture<bool> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_private_read_text(&self, _path: String) -> HostFuture<Option<String>> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_private_write_text(&self, _path: String, _text: String) -> HostFuture<()> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_private_delete(&self, _path: String) -> HostFuture<()> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_private_list(&self, _prefix: String) -> HostFuture<Vec<String>> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_shared_exists(&self, _path: String) -> HostFuture<bool> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_shared_read_text(&self, _path: String) -> HostFuture<Option<String>> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_shared_write_text(&self, _path: String, _text: String) -> HostFuture<()> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_shared_delete(&self, _path: String) -> HostFuture<()> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn fs_shared_list(&self, _prefix: String) -> HostFuture<Vec<String>> {
        ready_host(Err(HostError::unavailable("fs")))
    }

    fn log(&self, _level: String, _message: String) -> HostFuture<()> {
        ready_host(Err(HostError::unavailable("log")))
    }

    /// A host timer. The default is immediately ready; a device adapter
    /// should return a real timer future which wakes its task at the deadline.
    fn sleep_ms(&self, _duration_ms: u64) -> HostFuture<()> {
        ready_host(Ok(()))
    }

    fn battery(&self) -> HostFuture<BatteryReading> {
        ready_host(Err(HostError::unavailable("power")))
    }

    fn rtc(&self) -> HostFuture<RtcReading> {
        ready_host(Err(HostError::unavailable("time")))
    }

    fn wifi_status(&self) -> HostFuture<WifiStatus> {
        ready_host(Err(HostError::unavailable("wifi")))
    }

    fn cancelled(&self) -> bool {
        false
    }
}

struct Invocation {
    io: Arc<dyn HostIo>,
    config: StdConfig,
    state: InternalMutex<InvocationState>,
    started_at: Instant,
    drawing: InternalMutex<Drawing>,
}

#[derive(Default)]
struct InvocationState {
    revoked: bool,
    started_calls: usize,
    resource_bytes: usize,
    fs_calls: usize,
    fs_bytes: usize,
}

impl Invocation {
    fn allows(&self, capability: &str) -> bool {
        self.config
            .capabilities
            .iter()
            .any(|item| item == capability)
    }

    fn start(&self, capability: &str) -> HostResult<()> {
        if !self.allows(capability) {
            return Err(HostError::capability_denied(capability));
        }

        self.start_unprivileged()
    }

    fn start_unprivileged(&self) -> HostResult<()> {
        self.check_cancelled()?;
        let mut state = self.state.lock().map_err(|_| {
            HostError::new("internal", "runtime", "invocation lock poisoned", false)
        })?;
        if state.started_calls >= MAX_STARTED_CALLS {
            return Err(HostError::quota("started host-call limit reached"));
        }
        state.started_calls += 1;
        Ok(())
    }

    fn start_fs(&self, capability: &str) -> HostResult<()> {
        self.check_cancelled()?;
        if !self.allows(capability) {
            return Err(HostError::capability_denied(capability));
        }

        let mut state = self.state.lock().map_err(|_| {
            HostError::new("internal", "runtime", "invocation lock poisoned", false)
        })?;
        if state.started_calls >= MAX_STARTED_CALLS {
            return Err(HostError::quota("started host-call limit reached"));
        }
        if state.fs_calls >= FS_MAX_CALLS {
            return Err(HostError::quota("filesystem call limit reached"));
        }
        state.started_calls += 1;
        state.fs_calls += 1;
        Ok(())
    }

    fn remaining_ms(&self) -> u64 {
        self.config
            .wall_time_ms
            .saturating_sub(self.started_at.elapsed().as_millis() as u64)
    }

    fn check_cancelled(&self) -> HostResult<()> {
        if self.io.cancelled()
            || self.remaining_ms() == 0
            || self.state.lock().map(|s| s.revoked).unwrap_or(true)
        {
            Err(HostError::cancelled())
        } else {
            Ok(())
        }
    }

    fn reserve_resource_bytes(&self, bytes: usize) -> HostResult<()> {
        let mut state = self.state.lock().map_err(|_| {
            HostError::new("internal", "runtime", "invocation lock poisoned", false)
        })?;
        let total = state
            .resource_bytes
            .checked_add(bytes)
            .ok_or_else(|| HostError::quota("resource byte limit reached"))?;
        if total > RESOURCE_TOTAL_MAX_BYTES {
            return Err(HostError::quota("resource byte limit reached"));
        }
        state.resource_bytes = total;
        Ok(())
    }

    fn reserve_fs_bytes(&self, bytes: usize) -> HostResult<()> {
        let mut state = self.state.lock().map_err(|_| {
            HostError::new("internal", "runtime", "invocation lock poisoned", false)
        })?;
        let total = state
            .fs_bytes
            .checked_add(bytes)
            .ok_or_else(|| HostError::quota("filesystem byte limit reached"))?;
        if total > FS_TOTAL_MAX_BYTES {
            return Err(HostError::quota("filesystem byte limit reached"));
        }
        state.fs_bytes = total;
        Ok(())
    }

    fn validate_url(&self, request: &HttpRequest) -> HostResult<()> {
        if request.url.is_empty() || request.url.len() > HTTP_URL_MAX_BYTES {
            return Err(HostError::new(
                "url_too_long",
                "http",
                format!("URL must be 1..={HTTP_URL_MAX_BYTES} bytes"),
                false,
            ));
        }
        let (scheme, authority) = request
            .url
            .split_once("://")
            .ok_or_else(|| HostError::invalid_argument("URL must include a scheme"))?;
        if scheme != "http" && scheme != "https" {
            return Err(HostError::new(
                "unsupported_scheme",
                "http",
                "only http:// and https:// are supported",
                false,
            ));
        }
        let authority = authority.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() || authority.contains('@') {
            return Err(HostError::invalid_argument("URL authority is invalid"));
        }
        let host = authority_host(authority)?;
        if is_rejected_network_host(host)? {
            return Err(HostError::new(
                "address_not_allowed",
                "http",
                "multicast, broadcast, and unspecified addresses are denied",
                false,
            ));
        }
        Ok(())
    }
}

fn authority_host(authority: &str) -> HostResult<&str> {
    if authority.starts_with('[') {
        let end = authority
            .find(']')
            .ok_or_else(|| HostError::invalid_argument("invalid IPv6 authority"))?;
        if end == 1 {
            return Err(HostError::invalid_argument("empty host"));
        }
        return Ok(&authority[1..end]);
    }
    let host = authority.rsplit_once(':').and_then(|(host, port)| {
        if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
            Some(host)
        } else {
            None
        }
    });
    Ok(host.unwrap_or(authority))
}

fn is_rejected_network_host(host: &str) -> HostResult<bool> {
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(true);
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(match ip {
            IpAddr::V4(ip) => {
                ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_loopback()
                    || ip == Ipv4Addr::BROADCAST
            }
            IpAddr::V6(ip) => ip.is_unspecified() || ip.is_multicast() || ip.is_loopback(),
        });
    }
    if host.contains(':') {
        return Err(HostError::invalid_argument(
            "IPv6 URL must use bracketed authority",
        ));
    }
    Ok(false)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= KV_KEY_MAX_BYTES
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn validate_resource_path(path: &str) -> HostResult<()> {
    if path.is_empty()
        || path.len() > RESOURCE_PATH_MAX_BYTES
        || path.starts_with('/')
        || path.split('/').any(|part| part == ".." || part.is_empty())
    {
        return Err(HostError::invalid_argument("resource path is invalid"));
    }
    Ok(())
}

/// Normalize a filesystem path before it crosses the host boundary.
///
/// Filesystem paths are strictly relative. Empty segments, parent traversal,
/// backslashes, colons, NUL/control characters, and a path which normalizes
/// to the root are rejected. Dot segments are the only accepted
/// normalization: for example, `notes/./today.txt` becomes
/// `notes/today.txt`.
fn normalize_fs_path(path: &str) -> HostResult<String> {
    if path.is_empty() || path.len() > FS_PATH_MAX_BYTES {
        return Err(HostError::invalid_argument(format!(
            "filesystem path must be 1..={FS_PATH_MAX_BYTES} bytes"
        )));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(HostError::invalid_argument(
            "filesystem path must be relative",
        ));
    }
    if path
        .chars()
        .any(|character| character == '\\' || character == ':' || character.is_control())
    {
        return Err(HostError::invalid_argument(
            "filesystem path contains an invalid character",
        ));
    }

    let mut normalized = String::with_capacity(path.len());
    for segment in path.split('/') {
        match segment {
            "" => {
                return Err(HostError::invalid_argument(
                    "filesystem path contains an empty segment",
                ));
            }
            "." => continue,
            ".." => {
                return Err(HostError::invalid_argument(
                    "filesystem path must not contain parent traversal",
                ));
            }
            segment => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(segment);
            }
        }
    }
    if normalized.is_empty() || normalized.len() > FS_PATH_MAX_BYTES {
        return Err(HostError::invalid_argument(
            "filesystem path does not name a relative item",
        ));
    }
    Ok(normalized)
}

/// Normalize the prefix accepted by `fs.*.list`. An empty prefix denotes the
/// namespace root; non-empty prefixes use the same strict path rules as file
/// names. A trailing slash is intentionally rejected so that the host never
/// has to interpret two spellings of the same prefix.
fn normalize_fs_prefix(prefix: &str) -> HostResult<String> {
    if prefix.is_empty() {
        Ok(String::new())
    } else {
        normalize_fs_path(prefix)
    }
}

fn fs_path_matches_prefix(path: &str, prefix: &str) -> bool {
    prefix.is_empty() || path.starts_with(prefix)
}

/// Validate and canonicalize a host-provided listing. The adapter is expected
/// to apply the prefix itself, but the ABI checks that contract again before a
/// path crosses back into Rune. This keeps a buggy adapter from leaking a
/// path from a different namespace/prefix and keeps list data bounded.
fn validate_fs_list(prefix: &str, paths: Vec<String>) -> HostResult<Vec<String>> {
    if paths.len() > FS_LIST_MAX_ENTRIES {
        return Err(HostError::quota("filesystem list entry limit reached"));
    }

    let mut total_bytes = 0usize;
    let mut normalized = Vec::with_capacity(paths.len());
    for path in paths {
        let path = normalize_fs_path(&path)?;
        if !fs_path_matches_prefix(&path, prefix) {
            return Err(HostError::invalid_argument(
                "filesystem list returned a path outside its prefix",
            ));
        }
        total_bytes = total_bytes
            .checked_add(path.len())
            .ok_or_else(|| HostError::quota("filesystem list byte limit reached"))?;
        if total_bytes > FS_SINGLE_MAX_BYTES {
            return Err(HostError::quota(
                "filesystem list exceeds per-call byte limit",
            ));
        }
        normalized.push(path);
    }
    Ok(normalized)
}

fn validate_color(color: i64) -> HostResult<u8> {
    if (0..=3).contains(&color) {
        Ok(color as u8)
    } else {
        Err(HostError::invalid_argument("color must be 0, 1, 2, or 3"))
    }
}

fn checked_i32(value: i64, what: &str) -> HostResult<i32> {
    value
        .try_into()
        .map_err(|_| HostError::invalid_argument(format!("{what} is outside i32 range")))
}

fn validate_coordinate(x: i64, y: i64) -> HostResult<(i32, i32)> {
    Ok((checked_i32(x, "x")?, checked_i32(y, "y")?))
}

fn validate_rect(
    x: i64,
    y: i64,
    width: i64,
    height: i64,
) -> HostResult<Option<(i32, i32, u32, u32)>> {
    let (x, y) = validate_coordinate(x, y)?;
    if width < 0 || height < 0 {
        return Err(HostError::invalid_argument(
            "rectangle dimensions cannot be negative",
        ));
    }
    if width == 0 || height == 0 {
        return Ok(None);
    }
    Ok(Some((
        x,
        y,
        width
            .try_into()
            .map_err(|_| HostError::invalid_argument("rectangle width is outside u32 range"))?,
        height
            .try_into()
            .map_err(|_| HostError::invalid_argument("rectangle height is outside u32 range"))?,
    )))
}

/// A header that is safe to pass through Rune and host adapters.
#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct HttpHeader {
    #[rune(get)]
    pub name: String,
    #[rune(get)]
    pub value: String,
}

impl HttpHeader {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Header collection with a fallible Rune clone boundary. A wrapper avoids
/// exposing the host allocator's `Vec` directly as an `Any` getter.
#[derive(Any, Clone, Debug, Eq, PartialEq)]
#[rune(item = ::note4c)]
pub struct HttpHeaders {
    entries: Vec<HttpHeader>,
}

impl HttpHeaders {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn from_vec(entries: Vec<HttpHeader>) -> Self {
        Self { entries }
    }

    pub fn as_slice(&self) -> &[HttpHeader] {
        &self.entries
    }

    #[rune::function(instance)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[rune::function(instance)]
    pub fn get(&self, index: usize) -> Option<HttpHeader> {
        self.entries.get(index).cloned()
    }

    fn push(&mut self, header: HttpHeader) {
        self.entries.push(header);
    }
}

impl Default for HttpHeaders {
    fn default() -> Self {
        Self::new()
    }
}

impl rune::alloc::clone::TryClone for HttpHeaders {
    fn try_clone(&self) -> Result<Self, rune::alloc::Error> {
        Ok(self.clone())
    }
}

/// Full HTTP request. `tls_verify` is request-local; there is no global
/// “skip certificate verification” switch in the ABI.
#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct HttpRequest {
    #[rune(get)]
    pub method: String,
    #[rune(get)]
    pub url: String,
    #[rune(get)]
    pub headers: HttpHeaders,
    #[rune(get)]
    pub body: Option<String>,
    #[rune(get)]
    pub timeout_ms: u64,
    #[rune(get)]
    pub max_redirects: u8,
    #[rune(get)]
    pub max_body_bytes: usize,
    #[rune(get)]
    pub tls_verify: bool,
}

impl HttpRequest {
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> HostResult<Self> {
        let method = method.into();
        let url = url.into();
        let method = method.to_ascii_uppercase();
        if !matches!(
            method.as_str(),
            "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
        ) {
            return Err(HostError::invalid_argument("HTTP method is invalid"));
        }
        Ok(Self {
            method,
            url,
            headers: HttpHeaders::new(),
            body: None,
            timeout_ms: HTTP_TIMEOUT_DEFAULT_MS,
            max_redirects: HTTP_REDIRECT_MAX,
            max_body_bytes: HTTP_BODY_MAX_BYTES,
            tls_verify: true,
        })
    }

    pub fn get(url: impl Into<String>) -> HostResult<Self> {
        Self::new("GET", url)
    }

    #[rune::function(instance)]
    pub fn add_header(&mut self, name: &str, value: &str) -> HostResult<()> {
        self.add_header_impl(name, value)
    }

    fn add_header_impl(&mut self, name: &str, value: &str) -> HostResult<()> {
        let normalized_name = name.to_ascii_lowercase();
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii() && byte > 0x20 && byte != b':')
            || matches!(
                normalized_name.as_str(),
                "host" | "content-length" | "transfer-encoding" | "connection" | "upgrade"
            )
            || normalized_name.starts_with("proxy-")
            || value.len() > 512
            || value.bytes().any(|byte| byte == b'\r' || byte == b'\n')
        {
            return Err(HostError::invalid_argument("HTTP header is invalid"));
        }
        if self.headers.entries.len() >= HTTP_REQUEST_HEADERS_MAX {
            return Err(HostError::quota("HTTP header count limit reached"));
        }
        let total = self
            .headers
            .entries
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>();
        if total + name.len() + value.len() > HTTP_HEADERS_MAX_BYTES {
            return Err(HostError::quota("HTTP header byte limit reached"));
        }
        self.headers.push(HttpHeader::new(name, value));
        Ok(())
    }

    /// Basic credentials are scoped to an HTTPS request. Neither
    /// validation errors nor this helper include credentials in diagnostics.
    #[rune::function(instance)]
    pub fn set_basic_auth(&mut self, username: &str, password: &str) -> HostResult<()> {
        self.apply_basic_auth(username, password)
    }

    fn apply_basic_auth(&mut self, username: &str, password: &str) -> HostResult<()> {
        if !self.url.starts_with("https://") {
            return Err(HostError::invalid_argument(
                "Basic authentication requires HTTPS",
            ));
        }
        if username.is_empty()
            || username.contains(':')
            || username
                .chars()
                .chain(password.chars())
                .any(char::is_control)
            || username.len() + password.len() > 1024
        {
            return Err(HostError::invalid_argument("invalid Basic credentials"));
        }
        let input = format!("{username}:{password}");
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut header = String::from("Basic ");
        for chunk in input.as_bytes().chunks(3) {
            let a = chunk[0];
            let b = *chunk.get(1).unwrap_or(&0);
            let c = *chunk.get(2).unwrap_or(&0);
            header.push(ALPHABET[(a >> 2) as usize] as char);
            header.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
            header.push(if chunk.len() > 1 {
                ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char
            } else {
                '='
            });
            header.push(if chunk.len() > 2 {
                ALPHABET[(c & 63) as usize] as char
            } else {
                '='
            });
        }
        self.headers
            .entries
            .retain(|entry| !entry.name.eq_ignore_ascii_case("Authorization"));
        self.add_header_impl("Authorization", &header)
    }

    #[rune::function(instance)]
    pub fn set_body(&mut self, body: &str) -> HostResult<()> {
        if body.len() > HTTP_BODY_MAX_BYTES {
            return Err(HostError::new(
                "body_too_large",
                "http",
                format!("request body exceeds {HTTP_BODY_MAX_BYTES} bytes"),
                false,
            ));
        }
        self.body = Some(body.to_owned());
        Ok(())
    }

    #[rune::function(instance)]
    pub fn set_timeout(&mut self, timeout_ms: i64) -> HostResult<()> {
        if timeout_ms <= 0 || timeout_ms as u64 > HTTP_TIMEOUT_MAX_MS {
            return Err(HostError::invalid_argument(format!(
                "timeout must be 1..={HTTP_TIMEOUT_MAX_MS} ms"
            )));
        }
        self.timeout_ms = timeout_ms as u64;
        Ok(())
    }

    #[rune::function(instance)]
    pub fn set_tls_verify(&mut self, enabled: bool) {
        self.tls_verify = enabled;
    }

    #[rune::function(instance)]
    pub fn set_max_redirects(&mut self, max_redirects: i64) -> HostResult<()> {
        if !(0..=i64::from(HTTP_REDIRECT_MAX)).contains(&max_redirects) {
            return Err(HostError::invalid_argument(format!(
                "max_redirects must be 0..={HTTP_REDIRECT_MAX}"
            )));
        }
        self.max_redirects = max_redirects as u8;
        Ok(())
    }
}

/// UTF-8 HTTP response. A future ABI can add binary body support without
/// changing request scheduling or capability semantics.
#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct HttpResponse {
    #[rune(get)]
    pub status: u16,
    #[rune(get)]
    pub headers: HttpHeaders,
    #[rune(get)]
    pub body: String,
}

impl HttpResponse {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: HttpHeaders::new(),
            body: body.into(),
        }
    }

    pub fn with_headers(mut self, headers: Vec<HttpHeader>) -> Self {
        self.headers = HttpHeaders::from_vec(headers);
        self
    }

    #[rune::function(instance)]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    #[rune::function(instance)]
    pub fn text(&self) -> HostResult<String> {
        Ok(self.body.clone())
    }

    #[rune::function(instance)]
    pub fn json(&self) -> HostResult<JsonValue> {
        JsonValue::parse(&self.body)
    }
}

/// A parsed JSON value. The raw UTF-8 representation is kept so this type is
/// cheap to clone and does not expose a large recursive host object graph.
#[derive(Any, Clone, Debug, Eq, PartialEq)]
#[rune(item = ::note4c)]
pub struct JsonValue {
    #[rune(get)]
    pub raw: String,
}

impl JsonValue {
    pub fn parse(text: &str) -> HostResult<Self> {
        if text.len() > HTTP_BODY_MAX_BYTES {
            return Err(HostError::new(
                "json_too_large",
                "json",
                format!("JSON exceeds {HTTP_BODY_MAX_BYTES} bytes"),
                false,
            ));
        }
        let mut parser = JsonParser::new(text);
        parser.parse_value(0)?;
        parser.skip_ws();
        if parser.position != text.len() {
            return Err(HostError::new(
                "json_invalid",
                "json",
                "trailing bytes after JSON value",
                false,
            ));
        }
        Ok(Self {
            raw: text.to_owned(),
        })
    }

    fn node(&self) -> HostResult<JsonNode> {
        let mut parser = JsonParser::new(&self.raw);
        let node = parser.parse_value(0)?;
        parser.skip_ws();
        if parser.position != self.raw.len() {
            return Err(HostError::new(
                "json_invalid",
                "json",
                "invalid JSON",
                false,
            ));
        }
        Ok(node)
    }

    #[rune::function(instance)]
    pub fn kind(&self) -> HostResult<String> {
        Ok(match self.node()? {
            JsonNode::Null => "null",
            JsonNode::Bool(_) => "bool",
            JsonNode::Number(_) => "number",
            JsonNode::String(_) => "string",
            JsonNode::Array(_) => "array",
            JsonNode::Object(_) => "object",
        }
        .to_owned())
    }

    #[rune::function(instance)]
    pub fn get(&self, key: &str) -> HostResult<Option<Self>> {
        let JsonNode::Object(entries) = self.node()? else {
            return Ok(None);
        };
        Ok(entries
            .into_iter()
            .find(|(item, _)| item == key)
            .map(|(_, value)| Self {
                raw: value.to_json(),
            }))
    }

    #[rune::function(instance)]
    pub fn at(&self, index: usize) -> HostResult<Option<Self>> {
        let JsonNode::Array(values) = self.node()? else {
            return Ok(None);
        };
        Ok(values.get(index).map(|value| Self {
            raw: value.to_json(),
        }))
    }

    #[rune::function(instance)]
    pub fn string(&self) -> HostResult<Option<String>> {
        Ok(match self.node()? {
            JsonNode::String(value) => Some(value),
            _ => None,
        })
    }

    #[rune::function(instance)]
    pub fn integer(&self) -> HostResult<Option<i64>> {
        Ok(match self.node()? {
            JsonNode::Number(value) => value.parse::<i64>().ok(),
            _ => None,
        })
    }

    #[rune::function(instance)]
    pub fn boolean(&self) -> HostResult<Option<bool>> {
        Ok(match self.node()? {
            JsonNode::Bool(value) => Some(value),
            _ => None,
        })
    }

    #[rune::function(instance)]
    pub fn len(&self) -> HostResult<usize> {
        Ok(match self.node()? {
            JsonNode::Array(values) => values.len(),
            JsonNode::Object(values) => values.len(),
            _ => 0,
        })
    }

    #[rune::function(instance)]
    pub fn stringify(&self) -> String {
        self.raw.clone()
    }
}

#[derive(Clone, Debug)]
enum JsonNode {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<JsonNode>),
    Object(Vec<(String, JsonNode)>),
}

impl JsonNode {
    fn to_json(&self) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => value.clone(),
            Self::String(value) => format!("\"{}\"", escape_json(value)),
            Self::Array(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(Self::to_json)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Self::Object(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|(key, value)| format!("\"{}\":{}", escape_json(key), value.to_json()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\u{0008}' => escaped.push_str("\\b"),
            '\u{000c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{0000}'..='\u{001f}' => {
                use std::fmt::Write as _;
                write!(&mut escaped, "\\u{:04x}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

struct JsonParser<'a> {
    input: &'a str,
    position: usize,
    nodes: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            position: 0,
            nodes: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self
            .input
            .as_bytes()
            .get(self.position)
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.position += 1;
        }
    }

    fn parse_value(&mut self, depth: usize) -> HostResult<JsonNode> {
        if depth > JSON_MAX_DEPTH {
            return Err(HostError::quota("JSON nesting limit reached"));
        }
        if self.nodes >= JSON_MAX_NODES {
            return Err(HostError::quota("JSON node limit reached"));
        }
        self.nodes += 1;
        self.skip_ws();
        match self.input.as_bytes().get(self.position).copied() {
            Some(b'n') => self.literal("null", JsonNode::Null),
            Some(b't') => self.literal("true", JsonNode::Bool(true)),
            Some(b'f') => self.literal("false", JsonNode::Bool(false)),
            Some(b'"') => Ok(JsonNode::String(self.parse_string()?)),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b'{') => self.parse_object(depth + 1),
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => self.parse_number(),
            _ => Err(HostError::new(
                "json_invalid",
                "json",
                "expected JSON value",
                false,
            )),
        }
    }

    fn literal(&mut self, literal: &str, value: JsonNode) -> HostResult<JsonNode> {
        if self.input[self.position..].starts_with(literal) {
            self.position += literal.len();
            Ok(value)
        } else {
            Err(HostError::new(
                "json_invalid",
                "json",
                "invalid literal",
                false,
            ))
        }
    }

    fn parse_string(&mut self) -> HostResult<String> {
        if self.input.as_bytes().get(self.position) != Some(&b'"') {
            return Err(HostError::new(
                "json_invalid",
                "json",
                "expected string",
                false,
            ));
        }
        self.position += 1;
        let mut output = String::new();
        while let Some(byte) = self.input.as_bytes().get(self.position).copied() {
            self.position += 1;
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escaped = self
                        .input
                        .as_bytes()
                        .get(self.position)
                        .copied()
                        .ok_or_else(|| {
                            HostError::new("json_invalid", "json", "unterminated escape", false)
                        })?;
                    self.position += 1;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let end = self.position.checked_add(4).ok_or_else(|| {
                                HostError::new(
                                    "json_invalid",
                                    "json",
                                    "invalid unicode escape",
                                    false,
                                )
                            })?;
                            let hex = self.input.get(self.position..end).ok_or_else(|| {
                                HostError::new(
                                    "json_invalid",
                                    "json",
                                    "invalid unicode escape",
                                    false,
                                )
                            })?;
                            let value = u16::from_str_radix(hex, 16).map_err(|_| {
                                HostError::new(
                                    "json_invalid",
                                    "json",
                                    "invalid unicode escape",
                                    false,
                                )
                            })?;
                            self.position = end;
                            let scalar = if (0xd800..=0xdbff).contains(&value) {
                                let suffix_end = self.position.checked_add(6).ok_or_else(|| {
                                    HostError::new(
                                        "json_invalid",
                                        "json",
                                        "invalid unicode surrogate pair",
                                        false,
                                    )
                                })?;
                                let suffix =
                                    self.input.get(self.position..suffix_end).ok_or_else(|| {
                                        HostError::new(
                                            "json_invalid",
                                            "json",
                                            "invalid unicode surrogate pair",
                                            false,
                                        )
                                    })?;
                                let low = suffix
                                    .strip_prefix("\\u")
                                    .and_then(|hex| u16::from_str_radix(hex, 16).ok())
                                    .filter(|low| (0xdc00..=0xdfff).contains(low))
                                    .ok_or_else(|| {
                                        HostError::new(
                                            "json_invalid",
                                            "json",
                                            "invalid unicode surrogate pair",
                                            false,
                                        )
                                    })?;
                                self.position = suffix_end;
                                0x1_0000
                                    + ((u32::from(value) - 0xd800) << 10)
                                    + (u32::from(low) - 0xdc00)
                            } else if (0xdc00..=0xdfff).contains(&value) {
                                return Err(HostError::new(
                                    "json_invalid",
                                    "json",
                                    "isolated low unicode surrogate",
                                    false,
                                ));
                            } else {
                                u32::from(value)
                            };
                            let character = char::from_u32(scalar).ok_or_else(|| {
                                HostError::new(
                                    "json_invalid",
                                    "json",
                                    "invalid unicode scalar",
                                    false,
                                )
                            })?;
                            output.push(character);
                        }
                        _ => {
                            return Err(HostError::new(
                                "json_invalid",
                                "json",
                                "unknown escape",
                                false,
                            ));
                        }
                    }
                }
                byte if byte < 0x20 => {
                    return Err(HostError::new(
                        "json_invalid",
                        "json",
                        "control character in string",
                        false,
                    ));
                }
                byte => {
                    let start = self.position - 1;
                    let character = self.input[start..].chars().next().ok_or_else(|| {
                        HostError::new("json_invalid", "json", "invalid UTF-8", false)
                    })?;
                    if character.len_utf8() > 1 {
                        self.position = start + character.len_utf8();
                        output.push(character);
                    } else {
                        output.push(byte as char);
                    }
                }
            }
        }
        Err(HostError::new(
            "json_invalid",
            "json",
            "unterminated string",
            false,
        ))
    }

    fn parse_number(&mut self) -> HostResult<JsonNode> {
        let start = self.position;
        if self.input.as_bytes().get(self.position) == Some(&b'-') {
            self.position += 1;
        }
        let integer_start = self.position;
        while self
            .input
            .as_bytes()
            .get(self.position)
            .is_some_and(u8::is_ascii_digit)
        {
            self.position += 1;
        }
        if integer_start == self.position {
            return Err(HostError::new(
                "json_invalid",
                "json",
                "invalid number",
                false,
            ));
        }
        if self.input.as_bytes().get(self.position) == Some(&b'.') {
            self.position += 1;
            let fraction_start = self.position;
            while self
                .input
                .as_bytes()
                .get(self.position)
                .is_some_and(u8::is_ascii_digit)
            {
                self.position += 1;
            }
            if fraction_start == self.position {
                return Err(HostError::new(
                    "json_invalid",
                    "json",
                    "invalid fraction",
                    false,
                ));
            }
        }
        if self
            .input
            .as_bytes()
            .get(self.position)
            .is_some_and(|byte| *byte == b'e' || *byte == b'E')
        {
            self.position += 1;
            if self
                .input
                .as_bytes()
                .get(self.position)
                .is_some_and(|byte| *byte == b'+' || *byte == b'-')
            {
                self.position += 1;
            }
            let exponent_start = self.position;
            while self
                .input
                .as_bytes()
                .get(self.position)
                .is_some_and(u8::is_ascii_digit)
            {
                self.position += 1;
            }
            if exponent_start == self.position {
                return Err(HostError::new(
                    "json_invalid",
                    "json",
                    "invalid exponent",
                    false,
                ));
            }
        }
        Ok(JsonNode::Number(
            self.input[start..self.position].to_owned(),
        ))
    }

    fn parse_array(&mut self, depth: usize) -> HostResult<JsonNode> {
        self.position += 1;
        let mut values = Vec::new();
        self.skip_ws();
        if self.input.as_bytes().get(self.position) == Some(&b']') {
            self.position += 1;
            return Ok(JsonNode::Array(values));
        }
        loop {
            values.push(self.parse_value(depth)?);
            self.skip_ws();
            match self.input.as_bytes().get(self.position).copied() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    return Ok(JsonNode::Array(values));
                }
                _ => {
                    return Err(HostError::new(
                        "json_invalid",
                        "json",
                        "invalid array",
                        false,
                    ))
                }
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> HostResult<JsonNode> {
        self.position += 1;
        let mut values = Vec::new();
        self.skip_ws();
        if self.input.as_bytes().get(self.position) == Some(&b'}') {
            self.position += 1;
            return Ok(JsonNode::Object(values));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.input.as_bytes().get(self.position) != Some(&b':') {
                return Err(HostError::new(
                    "json_invalid",
                    "json",
                    "missing object colon",
                    false,
                ));
            }
            self.position += 1;
            let value = self.parse_value(depth)?;
            values.push((key, value));
            self.skip_ws();
            match self.input.as_bytes().get(self.position).copied() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(JsonNode::Object(values));
                }
                _ => {
                    return Err(HostError::new(
                        "json_invalid",
                        "json",
                        "invalid object",
                        false,
                    ))
                }
            }
        }
    }
}

/// Fixed NOTE4C panel information.
#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct DeviceInfo {
    #[rune(get)]
    pub model: String,
    #[rune(get)]
    pub firmware: String,
    #[rune(get)]
    pub screen_width: u32,
    #[rune(get)]
    pub screen_height: u32,
    #[rune(get)]
    pub color_count: u8,
}

impl Default for DeviceInfo {
    fn default() -> Self {
        Self {
            model: "NOTE4C".to_owned(),
            firmware: "unknown".to_owned(),
            screen_width: CANVAS_WIDTH,
            screen_height: CANVAS_HEIGHT,
            color_count: 4,
        }
    }
}

#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct WifiStatus {
    #[rune(get)]
    pub connected: bool,
    #[rune(get)]
    pub ssid: String,
    #[rune(get)]
    pub ip: Option<String>,
    #[rune(get)]
    pub provisioning: bool,
    #[rune(get)]
    pub mode: String,
    #[rune(get)]
    pub rssi: Option<i32>,
}

impl Default for WifiStatus {
    fn default() -> Self {
        Self {
            connected: false,
            ssid: String::new(),
            ip: None,
            provisioning: false,
            mode: "offline".to_owned(),
            rssi: None,
        }
    }
}

/// Raw battery/charger facts. `cell_mv` and `soc_percent` remain optional
/// until the adapter has enough valid measurements to derive them.
#[derive(Any, Clone, Debug, Default, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct BatteryReading {
    #[rune(get)]
    pub adc_raw: u32,
    #[rune(get)]
    pub adc_pin_mv: u32,
    #[rune(get)]
    pub standby_pin_high: bool,
    #[rune(get)]
    pub charge_pin_low: bool,
    #[rune(get)]
    pub cell_mv: Option<u32>,
    #[rune(get)]
    pub soc_percent: Option<u8>,
}

/// RTC sample. `unix_ms` is optional when the RTC has not been validated.
#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct RtcReading {
    #[rune(get)]
    pub year_00_99: u8,
    #[rune(get)]
    pub month: u8,
    #[rune(get)]
    pub day: u8,
    #[rune(get)]
    pub weekday: u8,
    #[rune(get)]
    pub hour: u8,
    #[rune(get)]
    pub minute: u8,
    #[rune(get)]
    pub second: u8,
    #[rune(get)]
    pub century_bit: bool,
    #[rune(get)]
    pub voltage_low: bool,
    #[rune(get)]
    pub unix_ms: Option<i64>,
    #[rune(get)]
    pub valid: bool,
}

/// Time-zone-adjusted system clock captured at widget invocation start.
#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct LocalTimeReading {
    #[rune(get)]
    pub year: u16,
    #[rune(get)]
    pub month: u8,
    #[rune(get)]
    pub day: u8,
    /// Sunday is 0 and Saturday is 6.
    #[rune(get)]
    pub weekday: u8,
    #[rune(get)]
    pub hour: u8,
    #[rune(get)]
    pub minute: u8,
    #[rune(get)]
    pub second: u8,
    #[rune(get)]
    pub utc_offset_minutes: i16,
    #[rune(get)]
    pub unix_ms: i64,
}

/// Chinese lunar calendar fields and display labels for one Gregorian date.
#[derive(Any, Clone, Debug, Eq, PartialEq, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct LunarReading {
    #[rune(get)]
    pub year: u16,
    #[rune(get)]
    pub month: u8,
    #[rune(get)]
    pub day: u8,
    #[rune(get)]
    pub leap_month: bool,
    /// Two-character sexagenary year, for example `丙午`.
    #[rune(get)]
    pub year_name: String,
    /// Lunar month including `月`, and `闰` when applicable.
    #[rune(get)]
    pub month_name: String,
    #[rune(get)]
    pub day_name: String,
}

impl Default for RtcReading {
    fn default() -> Self {
        Self {
            year_00_99: 0,
            month: 0,
            day: 0,
            weekday: 0,
            hour: 0,
            minute: 0,
            second: 0,
            century_bit: false,
            voltage_low: true,
            unix_ms: None,
            valid: false,
        }
    }
}

/// Owned canvas command. It is independent of panelet-ui so this ABI crate can
/// be tested and embedded without coupling the two crates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanvasCommandOwned {
    Clear {
        color: u8,
    },
    Pixel {
        x: i32,
        y: i32,
        color: u8,
    },
    Line {
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        color: u8,
    },
    Rect {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        color: u8,
    },
    FillRect {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        color: u8,
    },
    Text {
        x: i32,
        y: i32,
        text: String,
        color: u8,
        scale: u8,
    },
    TextSized {
        x: i32,
        y: i32,
        text: String,
        color: u8,
        height: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanvasFrame {
    pub commands: Vec<CanvasCommandOwned>,
    pub text_bytes: usize,
}

fn geometry(value: Value, expected: usize) -> HostResult<Vec<i64>> {
    let vector = value
        .downcast::<rune::runtime::Vec>()
        .map_err(|_| HostError::invalid_argument("canvas geometry must be an integer array"))?;
    if vector.len() != expected {
        return Err(HostError::invalid_argument(format!(
            "canvas geometry needs {expected} integers"
        )));
    }
    vector
        .into_iter()
        .map(|value| {
            value
                .as_signed()
                .map_err(|_| HostError::invalid_argument("canvas geometry must be integers"))
        })
        .collect()
}

#[derive(Any, Clone)]
#[rune(item = ::note4c)]
pub struct Canvas {
    invocation: InternalArc<Invocation>,
}

#[derive(Default)]
struct Drawing {
    broker: Option<crate::display::DisplayBroker>,
    epoch: u64,
    allowed: bool,
    frame: Option<crate::display::WorkingFrame>,
    commands: Vec<CanvasCommandOwned>,
    text_bytes: usize,
}

impl Canvas {
    fn push(&mut self, command: CanvasCommandOwned, text_bytes: usize) -> HostResult<()> {
        self.invocation.check_cancelled()?;
        let mut drawing = self
            .invocation
            .drawing
            .lock()
            .map_err(|_| HostError::cancelled())?;
        if drawing.commands.len() >= MAX_CANVAS_COMMANDS {
            return Err(HostError::quota("canvas command limit reached"));
        }
        if drawing.text_bytes + text_bytes > MAX_CANVAS_TEXT_BYTES {
            return Err(HostError::quota("canvas text byte limit reached"));
        }
        let frame = drawing.frame.as_mut().ok_or_else(HostError::cancelled)?;
        rasterize_command(frame.pixels_mut(), &command)?;
        drawing.commands.push(command);
        drawing.text_bytes += text_bytes;
        Ok(())
    }

    #[rune::function(instance)]
    pub fn command_count(&self) -> usize {
        self.invocation
            .drawing
            .lock()
            .map(|d| d.commands.len())
            .unwrap_or(0)
    }
    #[rune::function(instance)]
    pub fn text_bytes(&self) -> usize {
        self.invocation
            .drawing
            .lock()
            .map(|d| d.text_bytes)
            .unwrap_or(0)
    }

    #[rune::function(instance)]
    pub fn clear(&mut self, color: i64) -> HostResult<()> {
        self.push(
            CanvasCommandOwned::Clear {
                color: validate_color(color)?,
            },
            0,
        )
    }

    #[rune::function(instance)]
    pub fn pixel(&mut self, point: Value, color: i64) -> HostResult<()> {
        let point = geometry(point, 2)?;
        let x = point[0];
        let y = point[1];
        let (x, y) = validate_coordinate(x, y)?;
        self.push(
            CanvasCommandOwned::Pixel {
                x,
                y,
                color: validate_color(color)?,
            },
            0,
        )
    }

    #[rune::function(instance)]
    pub fn line(&mut self, points: Value, color: i64) -> HostResult<()> {
        let points = geometry(points, 4)?;
        let x0 = points[0];
        let y0 = points[1];
        let x1 = points[2];
        let y1 = points[3];
        let (x0, y0) = validate_coordinate(x0, y0)?;
        let (x1, y1) = validate_coordinate(x1, y1)?;
        self.push(
            CanvasCommandOwned::Line {
                x0,
                y0,
                x1,
                y1,
                color: validate_color(color)?,
            },
            0,
        )
    }

    #[rune::function(instance)]
    pub fn rect(&mut self, rectangle: Value, color: i64) -> HostResult<()> {
        let rectangle = geometry(rectangle, 4)?;
        let Some((x, y, width, height)) =
            validate_rect(rectangle[0], rectangle[1], rectangle[2], rectangle[3])?
        else {
            return Ok(());
        };
        self.push(
            CanvasCommandOwned::Rect {
                x,
                y,
                width,
                height,
                color: validate_color(color)?,
            },
            0,
        )
    }

    #[rune::function(instance)]
    pub fn fill_rect(&mut self, rectangle: Value, color: i64) -> HostResult<()> {
        let rectangle = geometry(rectangle, 4)?;
        let Some((x, y, width, height)) =
            validate_rect(rectangle[0], rectangle[1], rectangle[2], rectangle[3])?
        else {
            return Ok(());
        };
        self.push(
            CanvasCommandOwned::FillRect {
                x,
                y,
                width,
                height,
                color: validate_color(color)?,
            },
            0,
        )
    }

    #[rune::function(instance)]
    pub fn text(&mut self, point: Value, text: &str, color: i64, scale: i64) -> HostResult<()> {
        let point = geometry(point, 2)?;
        let (x, y) = validate_coordinate(point[0], point[1])?;
        if text.is_empty() || text.len() > MAX_CANVAS_TEXT_BYTES {
            return Err(HostError::invalid_argument(
                "canvas text is empty or too long",
            ));
        }
        if !(1..=4).contains(&scale) {
            return Err(HostError::invalid_argument(
                "canvas text scale must be 1..=4",
            ));
        }
        let color = validate_color(color)?;
        self.push(
            CanvasCommandOwned::Text {
                x,
                y,
                text: text.to_owned(),
                color,
                scale: scale as u8,
            },
            text.len(),
        )
    }
    #[rune::function(instance)]
    pub fn text_sized(
        &mut self,
        point: Value,
        text: &str,
        color: i64,
        height: i64,
    ) -> HostResult<()> {
        let point = geometry(point, 2)?;
        let (x, y) = validate_coordinate(point[0], point[1])?;
        if text.is_empty() || text.len() > MAX_CANVAS_TEXT_BYTES {
            return Err(HostError::invalid_argument(
                "canvas text is empty or too long",
            ));
        }
        if !(16..=32).contains(&height) || height % 2 != 0 {
            return Err(HostError::invalid_argument(
                "canvas text height must be an even value in 16..=32",
            ));
        }
        let color = validate_color(color)?;
        self.push(
            CanvasCommandOwned::TextSized {
                x,
                y,
                text: text.to_owned(),
                color,
                height: height as u8,
            },
            text.len(),
        )
    }
}

#[derive(Any, Clone, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct Std {
    #[rune(get)]
    pub config: ConfigApi,
    #[rune(get)]
    pub time: TimeApi,
    #[rune(get)]
    pub http: HttpApi,
    #[rune(get)]
    pub screen: ScreenApi,
    #[rune(get)]
    pub runtime: RuntimeApi,
    #[rune(get)]
    pub storage: StorageApi,
    #[rune(get)]
    pub resource: ResourceApi,
    #[rune(get)]
    pub fs: FsApi,
    #[rune(get)]
    pub log: LogApi,
    #[rune(get)]
    pub json: JsonApi,
    #[rune(get)]
    pub power: PowerApi,
    #[rune(get)]
    pub wifi: WifiApi,
    #[rune(get)]
    pub device: DeviceApi,
}

impl Std {
    /// Invalidate retained handles when an invocation ends or is cancelled.
    pub fn revoke(&self) {
        if let Ok(mut state) = self.runtime.invocation.state.lock() {
            state.revoked = true;
        }
        if let Ok(mut drawing) = self.runtime.invocation.drawing.lock() {
            drawing.frame = None;
            drawing.commands.clear();
        }
    }
    pub fn new(io: Box<dyn HostIo>, config: StdConfig) -> Self {
        Self::from_arc(Arc::from(io), config)
    }

    pub fn from_arc(io: Arc<dyn HostIo>, config: StdConfig) -> Self {
        let invocation = InternalArc::new(Invocation {
            io,
            config,
            state: InternalMutex::new(InvocationState::default()),
            started_at: Instant::now(),
            drawing: InternalMutex::new(Drawing::default()),
        });
        Self {
            config: ConfigApi::from_invocation(invocation.clone()),
            time: TimeApi::from_invocation(invocation.clone()),
            http: HttpApi::from_invocation(invocation.clone()),
            screen: ScreenApi::from_invocation(invocation.clone()),
            runtime: RuntimeApi::from_invocation(invocation.clone()),
            storage: StorageApi::from_invocation(invocation.clone()),
            resource: ResourceApi::from_invocation(invocation.clone()),
            fs: FsApi::from_invocation(invocation.clone()),
            log: LogApi::from_invocation(invocation.clone()),
            json: JsonApi,
            power: PowerApi::from_invocation(invocation.clone()),
            wifi: WifiApi::from_invocation(invocation.clone()),
            device: DeviceApi::from_invocation(invocation),
        }
    }

    /// Convenience constructor for tests and trusted built-ins. Uploaded
    /// widgets should pass an explicit `StdConfig` from their manifest.
    pub fn unrestricted_for_tests(io: Box<dyn HostIo>) -> Self {
        Self::new(io, StdConfig::unrestricted_for_tests())
    }
}

macro_rules! facade {
    ($name:ident) => {
        #[derive(Any, Clone, rune::alloc::clone::TryClone)]
        #[rune(item = ::note4c)]
        pub struct $name {
            invocation: InternalArc<Invocation>,
        }

        impl $name {
            fn from_invocation(invocation: InternalArc<Invocation>) -> Self {
                Self { invocation }
            }
        }
    };
}

facade!(ConfigApi);
facade!(TimeApi);
facade!(HttpApi);
facade!(ScreenApi);
facade!(RuntimeApi);
facade!(StorageApi);
facade!(ResourceApi);
facade!(FsPrivateApi);
facade!(FsSharedApi);
facade!(LogApi);

#[derive(Any, Clone, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct JsonApi;

#[derive(Any, Clone, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct FsApi {
    #[rune(get)]
    pub private: FsPrivateApi,
    #[rune(get)]
    pub shared: FsSharedApi,
}

impl FsApi {
    fn from_invocation(invocation: InternalArc<Invocation>) -> Self {
        Self {
            private: FsPrivateApi::from_invocation(invocation.clone()),
            shared: FsSharedApi::from_invocation(invocation),
        }
    }
}

#[derive(Any, Clone, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct PowerApi {
    invocation: InternalArc<Invocation>,
}

#[derive(Any, Clone, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct WifiApi {
    invocation: InternalArc<Invocation>,
}

#[derive(Any, Clone, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct DeviceApi {
    invocation: InternalArc<Invocation>,
}

impl DeviceApi {
    fn from_invocation(invocation: InternalArc<Invocation>) -> Self {
        Self { invocation }
    }
}

impl PowerApi {
    fn from_invocation(invocation: InternalArc<Invocation>) -> Self {
        Self { invocation }
    }
}

impl WifiApi {
    fn from_invocation(invocation: InternalArc<Invocation>) -> Self {
        Self { invocation }
    }
}

fn config_get(this: &ConfigApi) -> HostResult<JsonValue> {
    this.invocation.check_cancelled()?;
    this.invocation.start_unprivileged()?;
    if this.invocation.config.widget_config.len() > 2048 {
        return Err(HostError::quota("configuration exceeds 2048 bytes"));
    }
    JsonValue::parse(&this.invocation.config.widget_config)
}

fn runtime_fail(_this: &RuntimeApi, message: &str) -> HostResult<Value> {
    Err(HostError::new(
        "widget_rejected",
        "runtime",
        message.chars().take(160).collect::<String>(),
        false,
    ))
}

fn time_uptime(this: &TimeApi) -> HostResult<u64> {
    if !this.invocation.allows(CAP_TIME_READ) {
        return Err(HostError::capability_denied(CAP_TIME_READ));
    }
    Ok(this.invocation.config.uptime_ms)
}

fn time_unix(this: &TimeApi) -> HostResult<Option<i64>> {
    if !this.invocation.allows(CAP_TIME_READ) {
        return Err(HostError::capability_denied(CAP_TIME_READ));
    }
    Ok(this.invocation.config.unix_ms)
}

fn time_local(this: &TimeApi) -> HostResult<Option<LocalTimeReading>> {
    if !this.invocation.allows(CAP_TIME_READ) {
        return Err(HostError::capability_denied(CAP_TIME_READ));
    }
    Ok(this.invocation.config.local_time.clone())
}

fn time_lunar(this: &TimeApi, year: i64, month: i64, day: i64) -> HostResult<LunarReading> {
    if !this.invocation.allows(CAP_TIME_READ) {
        return Err(HostError::capability_denied(CAP_TIME_READ));
    }
    let year = u16::try_from(year)
        .map_err(|_| HostError::invalid_argument("lunar year is outside 2000..=2099"))?;
    let month = u8::try_from(month)
        .map_err(|_| HostError::invalid_argument("lunar source month is invalid"))?;
    let day = u8::try_from(day)
        .map_err(|_| HostError::invalid_argument("lunar source day is invalid"))?;
    lunar_reading(year, month, day)
}

fn lunar_reading(year: u16, month: u8, day: u8) -> HostResult<LunarReading> {
    let lunar = solar_to_lunar(year, month, day).map_err(|error| {
        HostError::invalid_argument(format!(
            "Gregorian date cannot be converted to lunar: {error:?}"
        ))
    })?;
    const STEMS: [&str; 10] = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"];
    const BRANCHES: [&str; 12] = [
        "子", "丑", "寅", "卯", "辰", "巳", "午", "未", "申", "酉", "戌", "亥",
    ];
    const MONTHS: [&str; 12] = [
        "正月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "冬月",
        "腊月",
    ];
    const DAYS: [&str; 30] = [
        "初一", "初二", "初三", "初四", "初五", "初六", "初七", "初八", "初九", "初十", "十一",
        "十二", "十三", "十四", "十五", "十六", "十七", "十八", "十九", "二十", "廿一", "廿二",
        "廿三", "廿四", "廿五", "廿六", "廿七", "廿八", "廿九", "三十",
    ];

    let year_offset = usize::from(lunar.year - 4);
    let mut year_name = String::from(STEMS[year_offset % STEMS.len()]);
    year_name.push_str(BRANCHES[year_offset % BRANCHES.len()]);
    let month_name = if lunar.is_leap_month {
        format!("闰{}", MONTHS[usize::from(lunar.month - 1)])
    } else {
        MONTHS[usize::from(lunar.month - 1)].to_owned()
    };

    Ok(LunarReading {
        year: lunar.year,
        month: lunar.month,
        day: lunar.day,
        leap_month: lunar.is_leap_month,
        year_name,
        month_name,
        day_name: DAYS[usize::from(lunar.day - 1)].to_owned(),
    })
}

fn time_rtc(this: TimeApi) -> impl CoreFuture<Output = HostResult<RtcReading>> {
    let invocation = this.invocation.clone();
    async move {
        invocation.check_cancelled()?;
        invocation.start(CAP_TIME_READ)?;
        invocation.io.rtc().await
    }
}

fn http_request(this: &HttpApi, method: &str, url: &str) -> HostResult<HttpRequest> {
    if !this.invocation.allows(CAP_HTTP_FETCH) {
        return Err(HostError::capability_denied(CAP_HTTP_FETCH));
    }
    let request = HttpRequest::new(method, url)?;
    this.invocation.validate_url(&request)?;
    Ok(request)
}

fn http_get(this: HttpApi, url: String) -> impl CoreFuture<Output = HostResult<HttpResponse>> {
    let invocation = this.invocation.clone();
    async move { http_fetch_impl(invocation, HttpRequest::get(url)?).await }
}

fn http_get_text(this: HttpApi, url: String) -> impl CoreFuture<Output = HostResult<String>> {
    let invocation = this.invocation.clone();
    async move {
        let response = http_fetch_impl(invocation, HttpRequest::get(url)?).await?;
        Ok(response.body)
    }
}

fn http_fetch(
    this: HttpApi,
    request: HttpRequest,
) -> impl CoreFuture<Output = HostResult<HttpResponse>> {
    let invocation = this.invocation.clone();
    async move { http_fetch_impl(invocation, request).await }
}

async fn http_fetch_impl(
    invocation: InternalArc<Invocation>,
    request: HttpRequest,
) -> HostResult<HttpResponse> {
    invocation.check_cancelled()?;
    invocation.start(CAP_HTTP_FETCH)?;
    invocation.validate_url(&request)?;
    if !request.tls_verify && !invocation.allows(CAP_HTTP_INSECURE_TLS) {
        return Err(HostError::capability_denied(CAP_HTTP_INSECURE_TLS));
    }
    if request
        .body
        .as_ref()
        .is_some_and(|body| body.len() > HTTP_BODY_MAX_BYTES)
    {
        return Err(HostError::new(
            "body_too_large",
            "http",
            format!("request body exceeds {HTTP_BODY_MAX_BYTES} bytes"),
            false,
        ));
    }
    let response = invocation.io.http_fetch(request.clone()).await?;
    if response.body.len() > request.max_body_bytes.min(HTTP_BODY_MAX_BYTES) {
        return Err(HostError::new(
            "response_too_large",
            "http",
            format!("response exceeds {} bytes", request.max_body_bytes),
            false,
        ));
    }
    if response.headers.entries.len() > HTTP_REQUEST_HEADERS_MAX
        || response
            .headers
            .entries
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
            > HTTP_HEADERS_MAX_BYTES
    {
        return Err(HostError::quota("HTTP response header limit reached"));
    }
    Ok(response)
}

fn storage_has(this: StorageApi, key: String) -> impl CoreFuture<Output = HostResult<bool>> {
    let invocation = this.invocation.clone();
    async move {
        validate_storage_key(&key)?;
        invocation.check_cancelled()?;
        invocation.start(CAP_STORAGE_KV)?;
        invocation.io.storage_has(key).await
    }
}

fn storage_get(
    this: StorageApi,
    key: String,
) -> impl CoreFuture<Output = HostResult<Option<String>>> {
    let invocation = this.invocation.clone();
    async move {
        validate_storage_key(&key)?;
        invocation.check_cancelled()?;
        invocation.start(CAP_STORAGE_KV)?;
        let value = invocation.io.storage_get(key).await?;
        if value
            .as_ref()
            .is_some_and(|value| value.len() > KV_VALUE_MAX_BYTES)
        {
            return Err(HostError::quota("stored value exceeds byte limit"));
        }
        Ok(value)
    }
}

fn storage_set(
    this: StorageApi,
    key: String,
    value: String,
) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        validate_storage_key(&key)?;
        if value.len() > KV_VALUE_MAX_BYTES {
            return Err(HostError::quota("stored value exceeds byte limit"));
        }
        invocation.check_cancelled()?;
        invocation.start(CAP_STORAGE_KV)?;
        invocation.io.storage_set(key, value).await
    }
}

fn storage_delete(this: StorageApi, key: String) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        validate_storage_key(&key)?;
        invocation.check_cancelled()?;
        invocation.start(CAP_STORAGE_KV)?;
        invocation.io.storage_delete(key).await
    }
}

fn validate_storage_key(key: &str) -> HostResult<()> {
    if valid_key(key) {
        Ok(())
    } else {
        Err(HostError::invalid_argument(format!(
            "storage key must be 1..={KV_KEY_MAX_BYTES} ASCII key bytes"
        )))
    }
}

fn log_write(
    this: LogApi,
    level: String,
    message: String,
) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        if !matches!(level.as_str(), "debug" | "info" | "warn" | "error") {
            return Err(HostError::invalid_argument("log level is invalid"));
        }
        if message.len() > LOG_MESSAGE_MAX_BYTES {
            return Err(HostError::quota("log message exceeds byte limit"));
        }
        invocation.check_cancelled()?;
        invocation.start(CAP_LOG_WRITE)?;
        invocation.io.log(level, message).await
    }
}

fn log_sugar(
    this: LogApi,
    level: &'static str,
    message: String,
) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        if message.len() > LOG_MESSAGE_MAX_BYTES {
            return Err(HostError::quota("log message exceeds byte limit"));
        }
        invocation.check_cancelled()?;
        invocation.start(CAP_LOG_WRITE)?;
        invocation.io.log(level.to_owned(), message).await
    }
}

fn log_debug(this: LogApi, message: String) -> impl CoreFuture<Output = HostResult<()>> {
    log_sugar(this, "debug", message)
}

fn log_info(this: LogApi, message: String) -> impl CoreFuture<Output = HostResult<()>> {
    log_sugar(this, "info", message)
}

fn log_warn(this: LogApi, message: String) -> impl CoreFuture<Output = HostResult<()>> {
    log_sugar(this, "warn", message)
}

fn log_error(this: LogApi, message: String) -> impl CoreFuture<Output = HostResult<()>> {
    log_sugar(this, "error", message)
}

fn runtime_abi_version(this: &RuntimeApi) -> u64 {
    let _ = this;
    u64::from(ABI_VERSION)
}

fn runtime_capability(this: &RuntimeApi, name: String) -> bool {
    this.invocation.allows(&name) && all_capabilities().contains(&name.as_str())
}

fn runtime_remaining_ms(this: &RuntimeApi) -> u64 {
    this.invocation.remaining_ms()
}

fn runtime_cancelled(this: &RuntimeApi) -> bool {
    this.invocation.io.cancelled() || this.invocation.remaining_ms() == 0
}

fn runtime_limits(this: &RuntimeApi) -> RuntimeLimits {
    let _ = this;
    RuntimeLimits::default()
}

/// Construct the successful entrypoint result without relying on Rune's
/// default prelude, which is intentionally not installed in the small MVP
/// context.
fn runtime_ok(this: &RuntimeApi, value: Value) -> HostResult<Value> {
    let _ = this;
    Ok(value)
}

#[derive(Any, Clone, Debug, Eq, PartialEq)]
#[rune(item = ::note4c)]
pub struct RuntimeLimits {
    #[rune(get)]
    pub max_started_calls: u64,
    #[rune(get)]
    pub max_wait_all: u64,
    #[rune(get)]
    pub max_canvas_commands: u64,
    #[rune(get)]
    pub max_canvas_text_bytes: u64,
    #[rune(get)]
    pub max_http_body_bytes: u64,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_started_calls: MAX_STARTED_CALLS as u64,
            max_wait_all: MAX_WAIT_ALL as u64,
            max_canvas_commands: MAX_CANVAS_COMMANDS as u64,
            max_canvas_text_bytes: MAX_CANVAS_TEXT_BYTES as u64,
            max_http_body_bytes: HTTP_BODY_MAX_BYTES as u64,
        }
    }
}

fn resource_exists(this: ResourceApi, path: String) -> impl CoreFuture<Output = HostResult<bool>> {
    let invocation = this.invocation.clone();
    async move {
        validate_resource_path(&path)?;
        invocation.check_cancelled()?;
        invocation.start_unprivileged()?;
        let resource = invocation.io.resource_get(path).await?;
        Ok(resource.is_some())
    }
}

fn resource_read_text(
    this: ResourceApi,
    path: String,
) -> impl CoreFuture<Output = HostResult<Option<String>>> {
    let invocation = this.invocation.clone();
    async move {
        validate_resource_path(&path)?;
        invocation.check_cancelled()?;
        invocation.start_unprivileged()?;
        let path_bytes = path.len();
        let resource = invocation.io.resource_get(path).await?;
        if let Some(value) = resource.as_ref() {
            if value.len() > RESOURCE_ITEM_MAX_BYTES {
                return Err(HostError::quota("resource exceeds per-item byte limit"));
            }
            let bytes = path_bytes
                .checked_add(value.len())
                .ok_or_else(|| HostError::quota("resource byte limit reached"))?;
            invocation.reserve_resource_bytes(bytes)?;
        }
        Ok(resource)
    }
}

fn resource_list(this: ResourceApi) -> impl CoreFuture<Output = HostResult<Vec<String>>> {
    let invocation = this.invocation.clone();
    async move {
        invocation.check_cancelled()?;
        invocation.start_unprivileged()?;
        let resources = invocation.io.resource_list().await?;
        let path_bytes = resources.iter().try_fold(0usize, |total, path| {
            validate_resource_path(path)?;
            total
                .checked_add(path.len())
                .ok_or_else(|| HostError::quota("resource list byte limit reached"))
        })?;
        if resources.len() > RESOURCE_COUNT_MAX || path_bytes > RESOURCE_TOTAL_MAX_BYTES {
            return Err(HostError::quota("resource list is invalid or too large"));
        }
        Ok(resources)
    }
}

fn fs_private_exists(
    this: FsPrivateApi,
    path: String,
) -> impl CoreFuture<Output = HostResult<bool>> {
    let invocation = this.invocation.clone();
    async move {
        let path = normalize_fs_path(&path)?;
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_PRIVATE)?;
        invocation.io.fs_private_exists(path).await
    }
}

fn fs_private_read_text(
    this: FsPrivateApi,
    path: String,
) -> impl CoreFuture<Output = HostResult<Option<String>>> {
    let invocation = this.invocation.clone();
    async move {
        let path = normalize_fs_path(&path)?;
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_PRIVATE)?;
        let value = invocation.io.fs_private_read_text(path).await?;
        if let Some(value) = value.as_ref() {
            if value.len() > FS_SINGLE_MAX_BYTES {
                return Err(HostError::quota(
                    "filesystem read exceeds per-call byte limit",
                ));
            }
            invocation.reserve_fs_bytes(value.len())?;
        }
        Ok(value)
    }
}

fn fs_private_write_text(
    this: FsPrivateApi,
    path: String,
    text: String,
) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        let path = normalize_fs_path(&path)?;
        if text.len() > FS_SINGLE_MAX_BYTES {
            return Err(HostError::quota(
                "filesystem write exceeds per-call byte limit",
            ));
        }
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_PRIVATE)?;
        invocation.reserve_fs_bytes(text.len())?;
        invocation.io.fs_private_write_text(path, text).await
    }
}

fn fs_private_delete(this: FsPrivateApi, path: String) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        let path = normalize_fs_path(&path)?;
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_PRIVATE)?;
        invocation.io.fs_private_delete(path).await
    }
}

fn fs_private_list(
    this: FsPrivateApi,
    prefix: String,
) -> impl CoreFuture<Output = HostResult<Vec<String>>> {
    let invocation = this.invocation.clone();
    async move {
        let prefix = normalize_fs_prefix(&prefix)?;
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_PRIVATE)?;
        let paths = invocation.io.fs_private_list(prefix.clone()).await?;
        let paths = validate_fs_list(&prefix, paths)?;
        let bytes = paths.iter().try_fold(0usize, |total, path| {
            total
                .checked_add(path.len())
                .ok_or_else(|| HostError::quota("filesystem list byte limit reached"))
        })?;
        invocation.reserve_fs_bytes(bytes)?;
        Ok(paths)
    }
}

fn fs_shared_exists(this: FsSharedApi, path: String) -> impl CoreFuture<Output = HostResult<bool>> {
    let invocation = this.invocation.clone();
    async move {
        let path = normalize_fs_path(&path)?;
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_SHARED_READ)?;
        invocation.io.fs_shared_exists(path).await
    }
}

fn fs_shared_read_text(
    this: FsSharedApi,
    path: String,
) -> impl CoreFuture<Output = HostResult<Option<String>>> {
    let invocation = this.invocation.clone();
    async move {
        let path = normalize_fs_path(&path)?;
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_SHARED_READ)?;
        let value = invocation.io.fs_shared_read_text(path).await?;
        if let Some(value) = value.as_ref() {
            if value.len() > FS_SINGLE_MAX_BYTES {
                return Err(HostError::quota(
                    "filesystem read exceeds per-call byte limit",
                ));
            }
            invocation.reserve_fs_bytes(value.len())?;
        }
        Ok(value)
    }
}

fn fs_shared_write_text(
    this: FsSharedApi,
    path: String,
    text: String,
) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        let path = normalize_fs_path(&path)?;
        if text.len() > FS_SINGLE_MAX_BYTES {
            return Err(HostError::quota(
                "filesystem write exceeds per-call byte limit",
            ));
        }
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_SHARED_WRITE)?;
        invocation.reserve_fs_bytes(text.len())?;
        invocation.io.fs_shared_write_text(path, text).await
    }
}

fn fs_shared_delete(this: FsSharedApi, path: String) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        let path = normalize_fs_path(&path)?;
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_SHARED_WRITE)?;
        invocation.io.fs_shared_delete(path).await
    }
}

fn fs_shared_list(
    this: FsSharedApi,
    prefix: String,
) -> impl CoreFuture<Output = HostResult<Vec<String>>> {
    let invocation = this.invocation.clone();
    async move {
        let prefix = normalize_fs_prefix(&prefix)?;
        invocation.check_cancelled()?;
        invocation.start_fs(CAP_FS_SHARED_READ)?;
        let paths = invocation.io.fs_shared_list(prefix.clone()).await?;
        let paths = validate_fs_list(&prefix, paths)?;
        let bytes = paths.iter().try_fold(0usize, |total, path| {
            total
                .checked_add(path.len())
                .ok_or_else(|| HostError::quota("filesystem list byte limit reached"))
        })?;
        invocation.reserve_fs_bytes(bytes)?;
        Ok(paths)
    }
}

fn runtime_sleep(this: RuntimeApi, duration_ms: i64) -> impl CoreFuture<Output = HostResult<()>> {
    let invocation = this.invocation.clone();
    async move {
        if duration_ms < 0 || duration_ms as u64 > MAX_SLEEP_MS {
            return Err(HostError::invalid_argument(format!(
                "sleep duration must be 0..={MAX_SLEEP_MS} ms"
            )));
        }
        invocation.check_cancelled()?;
        invocation.start_unprivileged()?;
        invocation.io.sleep_ms(duration_ms as u64).await
    }
}

/// Poll all children fairly, preserving input order. A child host error is a
/// normal `Result::Err` value and therefore does not abort siblings; a VM-level
/// error still aborts the aggregate because no valid Rune value exists to put
/// in the result array.
struct JoinAll {
    invocation: InternalArc<Invocation>,
    futures: Vec<Option<Pin<Box<RuneFuture>>>>,
    values: Vec<Option<VmResult<Value>>>,
}

impl JoinAll {
    fn new(invocation: InternalArc<Invocation>, futures: Vec<RuneFuture>) -> Self {
        let values = (0..futures.len()).map(|_| None).collect();
        Self {
            invocation,
            futures: futures
                .into_iter()
                .map(|future| Some(Box::pin(future)))
                .collect(),
            values,
        }
    }
}

impl CoreFuture for JoinAll {
    type Output = VmResult<HostResult<rune::runtime::Vec>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let mut pending = false;
        for index in 0..self.futures.len() {
            // A child can run Rune code that resolves os against a newer
            // binding. Reject the retained aggregate before polling any child.
            if let Err(error) = self.invocation.check_cancelled() {
                return Poll::Ready(VmResult::Ok(Err(error)));
            }
            let result = {
                let slot = &mut self.futures[index];
                let Some(future) = slot.as_mut() else {
                    continue;
                };
                match future.as_mut().poll(cx) {
                    Poll::Pending => {
                        pending = true;
                        None
                    }
                    Poll::Ready(result) => {
                        *slot = None;
                        Some(result)
                    }
                }
            };
            if let Some(result) = result {
                self.values[index] = Some(result);
            }
        }
        if pending {
            return Poll::Pending;
        }

        let mut values = rune::runtime::Vec::new();
        for value in self.values.iter_mut() {
            let Some(value) = value.take() else {
                return Poll::Ready(VmResult::Ok(Err(HostError::new(
                    "wait_all_incomplete",
                    "runtime",
                    "wait_all lost a child result",
                    false,
                ))));
            };
            match value {
                VmResult::Ok(value) => {
                    if let VmResult::Err(error) = values.push_value(value) {
                        return Poll::Ready(VmResult::Err(error));
                    }
                }
                VmResult::Err(error) => return Poll::Ready(VmResult::Err(error)),
            }
        }
        Poll::Ready(VmResult::Ok(Ok(values)))
    }
}

async fn runtime_wait_all(
    this: RuntimeApi,
    value: Value,
) -> VmResult<HostResult<rune::runtime::Vec>> {
    let invocation = this.invocation;
    if let Err(error) = invocation.check_cancelled() {
        return VmResult::Ok(Err(error));
    }
    let vector = match value.downcast::<rune::runtime::Vec>() {
        Ok(vector) => vector,
        Err(error) => return VmResult::err(error),
    };
    if vector.len() > MAX_WAIT_ALL {
        return VmResult::Ok(Err(HostError::quota(
            "wait_all accepts at most eight futures",
        )));
    }
    let mut futures = Vec::with_capacity(vector.len());
    for value in vector {
        match value.into_future() {
            Ok(future) => futures.push(future),
            Err(error) => return VmResult::err(error),
        }
    }
    JoinAll::new(invocation, futures).await
}

struct TimeoutFuture {
    invocation: InternalArc<Invocation>,
    future: Pin<Box<RuneFuture>>,
    timer: Option<HostFuture<()>>,
    timeout_ms: u64,
}

impl CoreFuture for TimeoutFuture {
    type Output = VmResult<Value>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        if let Err(error) = this.invocation.check_cancelled() {
            return Poll::Ready(host_error_value(error));
        }
        if let Poll::Ready(result) = this.future.as_mut().poll(cx) {
            return match result {
                VmResult::Ok(value) => Poll::Ready(VmResult::Ok(value)),
                VmResult::Err(error) => Poll::Ready(VmResult::Err(error)),
            };
        }

        if this.timer.is_none() {
            if let Err(error) = this.invocation.start_unprivileged() {
                return Poll::Ready(host_error_value(error));
            }
            this.timer = Some(this.invocation.io.sleep_ms(this.timeout_ms));
        }
        match this
            .timer
            .as_mut()
            .expect("timeout timer initialized")
            .as_mut()
            .poll(cx)
        {
            Poll::Ready(Ok(())) => Poll::Ready(host_error_value(HostError::timeout(
                "operation exceeded timeout",
            ))),
            Poll::Ready(Err(error)) => Poll::Ready(host_error_value(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn host_error_value(error: HostError) -> VmResult<Value> {
    match rune::to_value(Err::<Value, HostError>(error)) {
        Ok(value) => VmResult::Ok(value),
        Err(error) => VmResult::err(error),
    }
}

fn runtime_timeout(
    this: RuntimeApi,
    value: Value,
    timeout_ms: i64,
) -> impl CoreFuture<Output = VmResult<Value>> {
    let invocation = this.invocation.clone();
    async move {
        if timeout_ms <= 0 || timeout_ms as u64 > MAX_SLEEP_MS {
            return host_error_value(HostError::invalid_argument("timeout is out of bounds"));
        }
        let future = match value.into_future() {
            Ok(future) => future,
            Err(error) => return VmResult::err(error),
        };
        TimeoutFuture {
            invocation,
            future: Box::pin(future),
            timer: None,
            timeout_ms: timeout_ms as u64,
        }
        .await
    }
}

fn device_info(this: &DeviceApi) -> HostResult<DeviceInfo> {
    device_read_allowed(this)?;
    Ok(this.invocation.config.device.clone())
}

fn device_model(this: &DeviceApi) -> HostResult<String> {
    device_read_allowed(this)?;
    Ok(this.invocation.config.device.model.clone())
}

fn device_firmware(this: &DeviceApi) -> HostResult<String> {
    device_read_allowed(this)?;
    Ok(this.invocation.config.device.firmware.clone())
}

fn device_screen_width(this: &DeviceApi) -> HostResult<u32> {
    device_read_allowed(this)?;
    Ok(this.invocation.config.device.screen_width)
}

fn device_screen_height(this: &DeviceApi) -> HostResult<u32> {
    device_read_allowed(this)?;
    Ok(this.invocation.config.device.screen_height)
}

fn device_color_count(this: &DeviceApi) -> HostResult<u8> {
    device_read_allowed(this)?;
    Ok(this.invocation.config.device.color_count)
}

fn device_read_allowed(this: &DeviceApi) -> HostResult<()> {
    if this.invocation.allows(CAP_DEVICE_READ) {
        Ok(())
    } else {
        Err(HostError::capability_denied(CAP_DEVICE_READ))
    }
}

fn power_battery(this: PowerApi) -> impl CoreFuture<Output = HostResult<BatteryReading>> {
    let invocation = this.invocation.clone();
    async move {
        invocation.check_cancelled()?;
        invocation.start(CAP_POWER_READ)?;
        invocation.io.battery().await
    }
}

fn wifi_status(this: WifiApi) -> impl CoreFuture<Output = HostResult<WifiStatus>> {
    let invocation = this.invocation.clone();
    async move {
        invocation.check_cancelled()?;
        invocation.start(CAP_WIFI_READ)?;
        invocation.io.wifi_status().await
    }
}

/// Clip untrusted feed text without splitting UTF-8 or producing unsupported
/// glyphs. Ellipsis width is reserved only when actual clipping is necessary.
fn screen_fit_text(_this: &ScreenApi, text: &str, width: i64, scale: i64) -> HostResult<String> {
    fit_text_layout(text, width, scale, false)
}

fn screen_fit_text_sized(
    _this: &ScreenApi,
    text: &str,
    width: i64,
    height: i64,
) -> HostResult<String> {
    fit_text_layout(text, width, height, true)
}

fn fit_text_layout(text: &str, width: i64, scale: i64, sized: bool) -> HostResult<String> {
    if !(1..=400).contains(&width)
        || (if sized {
            !(16..=32).contains(&scale) || scale % 2 != 0
        } else {
            !(1..=4).contains(&scale)
        })
        || text.len() > 1024
    {
        return Err(HostError::new(
            "invalid_argument",
            "screen",
            "invalid text layout bounds",
            false,
        ));
    }
    let advance = |character| {
        if sized {
            panelet_ui::glyph16_advance(character).map(|value| value as i64 * scale / 16)
        } else {
            panelet_ui::glyph_advance(character).map(|value| value as i64 * scale)
        }
    };
    let ellipsis = advance('.').unwrap_or(6) * 3;
    let mut output = String::new();
    let mut used = 0;
    let mut clipped = false;
    for original in text.chars() {
        let original = if sized {
            original
        } else {
            original.to_ascii_uppercase()
        };
        let character = if original.is_control() {
            ' '
        } else if advance(original).is_some() {
            original
        } else {
            '?'
        };
        let char_width = advance(character).unwrap_or(6);
        if used + char_width > width {
            clipped = true;
            break;
        }
        output.push(character);
        used += char_width;
    }
    if clipped && width >= ellipsis {
        while used + ellipsis > width {
            if let Some(character) = output.pop() {
                used -= advance(character).unwrap_or(6);
            } else {
                break;
            }
        }
        output.push_str("...");
    }
    Ok(output)
}

fn register_module() -> Result<Module, ContextError> {
    let mut module = Module::with_crate("note4c")?;

    module.ty::<HostError>()?;
    module.ty::<HttpHeader>()?;
    module.ty::<HttpHeaders>()?;
    module.ty::<HttpRequest>()?;
    module.ty::<HttpResponse>()?;
    module.ty::<JsonValue>()?;
    module.ty::<Canvas>()?;
    module.ty::<SubmissionReceipt>()?;
    module.function_meta(canvas_flush)?;
    module.ty::<DeviceInfo>()?;
    module.ty::<BatteryReading>()?;
    module.ty::<RtcReading>()?;
    module.ty::<LocalTimeReading>()?;
    module.ty::<LunarReading>()?;
    module.ty::<WifiStatus>()?;
    module.ty::<RuntimeLimits>()?;

    module.function_meta(JsonValue::at)?;

    module.function_meta(HttpRequest::add_header)?;
    module.function_meta(HttpRequest::set_basic_auth)?;
    module.function_meta(HttpRequest::set_body)?;
    module.function_meta(HttpRequest::set_timeout)?;
    module.function_meta(HttpRequest::set_tls_verify)?;
    module.function_meta(HttpRequest::set_max_redirects)?;
    module.function_meta(HttpHeaders::len)?;
    module.function_meta(HttpHeaders::get)?;
    module.function_meta(HttpResponse::is_success)?;
    module.function_meta(HttpResponse::text)?;
    module.function_meta(HttpResponse::json)?;
    module.function_meta(JsonValue::kind)?;
    module.function_meta(JsonValue::get)?;
    module.function_meta(JsonValue::string)?;
    module.function_meta(JsonValue::integer)?;
    module.function_meta(JsonValue::boolean)?;
    module.function_meta(JsonValue::len)?;
    module.function_meta(JsonValue::stringify)?;
    module.function_meta(Canvas::command_count)?;
    module.function_meta(Canvas::text_bytes)?;
    module.function_meta(Canvas::clear)?;
    module.function_meta(Canvas::pixel)?;
    module.function_meta(Canvas::line)?;
    module.function_meta(Canvas::rect)?;
    module.function_meta(Canvas::fill_rect)?;
    module.function_meta(Canvas::text)?;
    module.function_meta(Canvas::text_sized)?;

    Ok(module)
}

/// Construct the v2 context used by uploaded widgets and host tests.
pub fn mvp_context() -> Result<Context, ContextError> {
    {
        let display = crate::display::DisplayBroker::new();
        context_with_os(&OsBindings::new(display.clone(), display.epoch()))
    }
}

pub fn context_with_os(bindings: &OsBindings) -> Result<Context, ContextError> {
    let mut context = minimal_context()?;
    context.install(rune::modules::i64::module()?)?;
    // Unsigned Rust return values (for example RTC u8 fields and uptime u64)
    // enter Rune as its native u64 value representation. Install the matching
    // protocol module so widget code can format and otherwise operate on them.
    context.install(rune::modules::u64::module()?)?;
    context.install(rune::modules::iter::module()?)?;
    context.install(rune::modules::option::module()?)?;
    context.install(rune::modules::ops::module()?)?;
    context.install(rune::modules::string::module()?)?;
    context.install(rune::modules::result::module()?)?;
    context.install(rune::modules::future::module()?)?;
    context.install(rune::modules::object::module()?)?;
    context.install(register_module()?)?;
    for module in os::modules(bindings.clone())? {
        context.install(module)?;
    }
    context.install(crate::lifecycle::module()?)?;
    context.install(crate::lifecycle::input_module()?)?;
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Source, Sources, Vm};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeIo {
        calls: Arc<AtomicUsize>,
        logs: Arc<Mutex<Vec<(String, String)>>>,
        display: Arc<Mutex<Vec<CanvasFrame>>>,
        kv: Arc<Mutex<BTreeMap<String, String>>>,
        resources: Arc<Mutex<BTreeMap<String, String>>>,
        private_files: Arc<Mutex<BTreeMap<String, String>>>,
        shared_files: Arc<Mutex<BTreeMap<String, String>>>,
        http_results: Arc<Mutex<std::collections::VecDeque<HostResult<HttpResponse>>>>,
        http_requests: Arc<Mutex<Vec<HttpRequest>>>,
    }

    impl FakeIo {
        fn count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn list_files(files: &BTreeMap<String, String>, prefix: &str) -> Vec<String> {
            files
                .keys()
                .filter(|path| prefix.is_empty() || path.as_str().starts_with(prefix))
                .cloned()
                .collect()
        }
    }

    impl HostIo for FakeIo {
        fn http_fetch(&self, request: HttpRequest) -> HostFuture<HttpResponse> {
            let calls = self.calls.clone();
            let results = self.http_results.clone();
            let requests = self.http_requests.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                requests.lock().unwrap().push(request.clone());
                results
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| Ok(HttpResponse::new(200, format!("body:{}", request.url))))
            })
        }

        fn on_submission(&self, frame: CanvasFrame) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.display.lock().unwrap().push(frame);
        }

        fn storage_has(&self, key: String) -> HostFuture<bool> {
            let calls = self.calls.clone();
            let kv = self.kv.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(kv.lock().unwrap().contains_key(&key))
            })
        }

        fn storage_get(&self, key: String) -> HostFuture<Option<String>> {
            let calls = self.calls.clone();
            let kv = self.kv.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(kv.lock().unwrap().get(&key).cloned())
            })
        }

        fn storage_set(&self, key: String, value: String) -> HostFuture<()> {
            let calls = self.calls.clone();
            let kv = self.kv.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                kv.lock().unwrap().insert(key, value);
                Ok(())
            })
        }

        fn storage_delete(&self, key: String) -> HostFuture<()> {
            let calls = self.calls.clone();
            let kv = self.kv.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                kv.lock().unwrap().remove(&key);
                Ok(())
            })
        }

        fn resource_get(&self, path: String) -> HostFuture<Option<String>> {
            let calls = self.calls.clone();
            let resources = self.resources.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(resources.lock().unwrap().get(&path).cloned())
            })
        }

        fn resource_list(&self) -> HostFuture<Vec<String>> {
            let calls = self.calls.clone();
            let resources = self.resources.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(resources.lock().unwrap().keys().cloned().collect())
            })
        }

        fn fs_private_exists(&self, path: String) -> HostFuture<bool> {
            let calls = self.calls.clone();
            let files = self.private_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(files.lock().unwrap().contains_key(&path))
            })
        }

        fn fs_private_read_text(&self, path: String) -> HostFuture<Option<String>> {
            let calls = self.calls.clone();
            let files = self.private_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(files.lock().unwrap().get(&path).cloned())
            })
        }

        fn fs_private_write_text(&self, path: String, text: String) -> HostFuture<()> {
            let calls = self.calls.clone();
            let files = self.private_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                files.lock().unwrap().insert(path, text);
                Ok(())
            })
        }

        fn fs_private_delete(&self, path: String) -> HostFuture<()> {
            let calls = self.calls.clone();
            let files = self.private_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                files.lock().unwrap().remove(&path);
                Ok(())
            })
        }

        fn fs_private_list(&self, prefix: String) -> HostFuture<Vec<String>> {
            let calls = self.calls.clone();
            let files = self.private_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Self::list_files(&files.lock().unwrap(), &prefix))
            })
        }

        fn fs_shared_exists(&self, path: String) -> HostFuture<bool> {
            let calls = self.calls.clone();
            let files = self.shared_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(files.lock().unwrap().contains_key(&path))
            })
        }

        fn fs_shared_read_text(&self, path: String) -> HostFuture<Option<String>> {
            let calls = self.calls.clone();
            let files = self.shared_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(files.lock().unwrap().get(&path).cloned())
            })
        }

        fn fs_shared_write_text(&self, path: String, text: String) -> HostFuture<()> {
            let calls = self.calls.clone();
            let files = self.shared_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                files.lock().unwrap().insert(path, text);
                Ok(())
            })
        }

        fn fs_shared_delete(&self, path: String) -> HostFuture<()> {
            let calls = self.calls.clone();
            let files = self.shared_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                files.lock().unwrap().remove(&path);
                Ok(())
            })
        }

        fn fs_shared_list(&self, prefix: String) -> HostFuture<Vec<String>> {
            let calls = self.calls.clone();
            let files = self.shared_files.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Self::list_files(&files.lock().unwrap(), &prefix))
            })
        }

        fn log(&self, level: String, message: String) -> HostFuture<()> {
            let calls = self.calls.clone();
            let logs = self.logs.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                logs.lock().unwrap().push((level, message));
                Ok(())
            })
        }

        fn sleep_ms(&self, _duration_ms: u64) -> HostFuture<()> {
            let calls = self.calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }

        fn battery(&self) -> HostFuture<BatteryReading> {
            let calls = self.calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(BatteryReading {
                    adc_raw: 123,
                    adc_pin_mv: 1800,
                    standby_pin_high: true,
                    charge_pin_low: false,
                    cell_mv: Some(3800),
                    soc_percent: Some(80),
                })
            })
        }

        fn rtc(&self) -> HostFuture<RtcReading> {
            let calls = self.calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(RtcReading {
                    year_00_99: 24,
                    month: 9,
                    day: 3,
                    // The example calendar documents and tests the raw
                    // PCF8563 convention Sunday=0; 2024-09-03 was Tuesday.
                    weekday: 2,
                    hour: 12,
                    minute: 34,
                    second: 56,
                    century_bit: false,
                    voltage_low: false,
                    unix_ms: Some(1_700_000_000_000),
                    valid: true,
                })
            })
        }

        fn wifi_status(&self) -> HostFuture<WifiStatus> {
            let calls = self.calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(WifiStatus {
                    connected: true,
                    ssid: "test".to_owned(),
                    ip: Some("192.168.1.2".to_owned()),
                    provisioning: false,
                    mode: "station".to_owned(),
                    rssi: Some(-42),
                })
            })
        }
    }

    fn test_config() -> StdConfig {
        StdConfig {
            widget_config: "{}".to_owned(),
            uptime_ms: 1234,
            unix_ms: Some(1_700_000_000_000),
            local_time: Some(LocalTimeReading {
                year: 2026,
                month: 9,
                day: 5,
                weekday: 6,
                hour: 20,
                minute: 34,
                second: 56,
                utc_offset_minutes: 480,
                unix_ms: 1_788_560_096_000,
            }),
            capabilities: all_capabilities().into_iter().map(str::to_owned).collect(),
            host_permissions: vec!["example.test".to_owned(), "192.168.1.2".to_owned()],
            device: DeviceInfo::default(),
            wall_time_ms: 120_000,
        }
    }

    fn run(source: &str, host: Std) -> rune::runtime::Value {
        let mut sources = Sources::new();
        sources.insert(Source::memory(source).unwrap()).unwrap();
        let display = crate::display::DisplayBroker::new();
        let bindings = OsBindings::new(display.clone(), display.epoch());
        let context = context_with_os(&bindings).unwrap();
        let mut diagnostics = rune::Diagnostics::new();
        let unit = match rune::prepare(&mut sources)
            .with_context(&context)
            .with_diagnostics(&mut diagnostics)
            .build()
        {
            Ok(unit) => unit,
            Err(error) => panic!("Rune build failed: {error:?}; diagnostics: {diagnostics:?}"),
        };
        let runtime = Arc::new(context.runtime().unwrap());
        let mut vm = Vm::new(runtime, Arc::new(unit));
        let call = async {
            if source.contains("#[widget::component]") {
                let mut session = crate::lifecycle::ComponentSession::new(
                    vm,
                    bindings.clone(),
                    64 * 1024,
                    100_000,
                )
                .unwrap();
                rune::to_value(
                    session
                        .dispatch(host, crate::lifecycle::ComponentEvent::Mount)
                        .await,
                )
                .unwrap()
            } else {
                bindings.activate(host, true).unwrap();
                let value = rune::runtime::budget::with(
                    100_000,
                    rune::alloc::limit::with(64 * 1024, vm.async_call(["main"], ())),
                )
                .await
                .unwrap();
                bindings.revoke();
                value
            }
        };
        let mut call = std::pin::pin!(call);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            match call.as_mut().poll(&mut cx) {
                Poll::Ready(result) => break result,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn compile(source: &str) {
        let mut sources = Sources::new();
        sources.insert(Source::memory(source).unwrap()).unwrap();
        let context = mvp_context().unwrap();
        let mut diagnostics = rune::Diagnostics::new();
        if let Err(error) = rune::prepare(&mut sources)
            .with_context(&context)
            .with_diagnostics(&mut diagnostics)
            .build()
        {
            panic!("example widget did not compile: {error}; diagnostics: {diagnostics:?}");
        }
    }

    fn compile_with_limit(source: &str, limit_bytes: usize) {
        let mut sources = Sources::new();
        sources.insert(Source::memory(source).unwrap()).unwrap();
        let context = mvp_context().unwrap();
        let mut diagnostics = rune::Diagnostics::new();
        if let Err(error) = rune::alloc::limit::with(limit_bytes, || {
            rune::prepare(&mut sources)
                .with_context(&context)
                .with_diagnostics(&mut diagnostics)
                .build()
        })
        .call()
        {
            panic!(
                "example widget exceeded the {limit_bytes}-byte compile budget or did not compile: \
                 {error}; diagnostics: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn example_widget_sources_compile_with_the_device_context() {
        compile(include_str!("../../../examples/widgets/todo/main.rn"));
        compile(include_str!("../../../examples/widgets/calendar/main.rn"));
        compile(include_str!(
            "../../../examples/widgets/photo-album/main.rn"
        ));
    }

    #[test]
    fn example_widget_sources_fit_the_device_compile_budget() {
        compile_with_limit(
            include_str!("../../../examples/widgets/todo/main.rn"),
            1024 * 1024,
        );
        const DEVICE_COMPILE_MEMORY_LIMIT: usize = 1024 * 1024;

        compile_with_limit(
            include_str!("../../../examples/widgets/calendar/main.rn"),
            DEVICE_COMPILE_MEMORY_LIMIT,
        );
        compile_with_limit(
            include_str!("../../../examples/widgets/photo-album/main.rn"),
            DEVICE_COMPILE_MEMORY_LIMIT,
        );
    }

    fn successful_entry(value: Value) -> Value {
        rune::from_value::<Result<Value, HostError>>(value)
            .expect("example entry must return Result<Value, HostError>")
            .expect("example entry returned a host error")
    }

    #[test]
    fn todo_basic_auth_is_utf8_bounded_and_https_only() {
        let io = Arc::new(FakeIo::default());
        io.http_results
            .lock()
            .unwrap()
            .push_back(Ok(HttpResponse::new(200, r#"{"version":1,"items":[]}"#)));
        let mut config = test_config();
        config.widget_config=r#"{"url":"https://example.test/todo","auth_type":"basic","username":"Aladdin","password":"open sesame"}"#.into();
        successful_entry(run(
            include_str!("../../../examples/widgets/todo/main.rn"),
            Std::from_arc(io.clone(), config),
        ));
        let requests = io.http_requests.lock().unwrap();
        assert_eq!(
            requests[0]
                .headers
                .entries
                .iter()
                .find(|h| h.name == "Authorization")
                .map(|h| h.value.as_str()),
            Some("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==")
        );
        drop(requests);
        for (url, user, password) in [
            ("http://example.test/todo", "u", "p"),
            ("https://example.test/todo", "a:b", "secret"),
            ("https://example.test/todo", "u", "secret\n"),
        ] {
            let mut request = HttpRequest::new("GET", url).unwrap();
            let error = request.apply_basic_auth(user, password).unwrap_err();
            assert!(!error.message.contains("secret"));
            assert!(request
                .headers
                .entries
                .iter()
                .all(|h| h.name != "Authorization"));
        }
        let mut request = HttpRequest::new("GET", "https://example.test/todo").unwrap();
        request.apply_basic_auth("u", "密").unwrap();
        assert_eq!(
            request
                .headers
                .entries
                .iter()
                .find(|h| h.name == "Authorization")
                .map(|h| h.value.as_str()),
            Some("Basic dTrlr4Y=")
        );
        request.apply_basic_auth("u", "p").unwrap();
        assert_eq!(request.headers.entries.len(), 1);
    }

    #[test]
    fn todo_pulls_configured_snapshot_and_renders_only_open_items() {
        let io = Arc::new(FakeIo::default());
        io.http_results.lock().unwrap().push_back(Ok(HttpResponse::new(200,
            r#"{"version":1,"items":[{"text":"买牛奶","detail":"今天 18:00","important":true},{"text":"已完成","completed":true},{"text":"Buy bread"}]}"#)));
        let mut config = test_config();
        config.widget_config =
            r#"{"url":"https://example.test/todo","bearer_token":"TEST_ONLY","title":"今日待办"}"#
                .into();
        successful_entry(run(
            include_str!("../../../examples/widgets/todo/main.rn"),
            Std::from_arc(io.clone(), config),
        ));
        let requests = io.http_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url, "https://example.test/todo");
        assert_eq!(requests[0].max_redirects, 0);
        assert!(requests[0]
            .headers
            .entries
            .iter()
            .any(|h| h.name == "Authorization" && h.value == "Bearer TEST_ONLY"));
        let frames = io.display.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert!(frame_has_text(&frames[0], "买牛奶"));
        assert!(frame_has_text(&frames[0], "Buy bread"));
        assert!(!frame_has_text(&frames[0], "已完成"));
        assert!(frame_has_text(&frames[0], "2 / 2 TODO"));
        assert!(frames[0].commands.iter().any(
            |c| matches!(c, CanvasCommandOwned::TextSized { text, color:3, .. } if text == "买牛奶")
        ));
    }

    #[test]
    fn todo_invalid_snapshots_and_http_errors_never_submit_a_frame() {
        for (status, body) in [
            (500, "server error"),
            (304, ""),
            (200, "invalid"),
            (200, "{}"),
            (200, r#"{"version":2,"items":[]}"#),
            (200, r#"{"version":1,"items":{}}"#),
            (
                200,
                r#"{"version":1,"items":[{"text":"OK"},{"completed":false}]}"#,
            ),
            (
                200,
                r#"{"version":1,"items":[{"text":"OK","completed":"false"}]}"#,
            ),
        ] {
            let io = Arc::new(FakeIo::default());
            io.http_results
                .lock()
                .unwrap()
                .push_back(Ok(HttpResponse::new(status, body)));
            let mut config = test_config();
            config.widget_config = r#"{"url":"https://example.test/todo"}"#.into();
            let value = run(
                include_str!("../../../examples/widgets/todo/main.rn"),
                Std::from_arc(io.clone(), config),
            );
            assert!(
                rune::from_value::<Result<Value, HostError>>(value)
                    .unwrap()
                    .is_err(),
                "{body}"
            );
            assert!(io.display.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn todo_bounds_rows_and_layout_and_rejects_oversized_item_lists() {
        for count in [64, 65] {
            let io = Arc::new(FakeIo::default());
            let text = "今天准备提交项目进度报告并且买牛奶".repeat(3);
            let body = format!(
                r#"{{"version":1,"items":[{}]}}"#,
                vec![format!(r#"{{"text":"{text}","detail":"今天 18:00"}}"#); count].join(",")
            );
            io.http_results
                .lock()
                .unwrap()
                .push_back(Ok(HttpResponse::new(200, body)));
            let mut config = test_config();
            config.widget_config = r#"{"url":"https://example.test/todo"}"#.into();
            let result = rune::from_value::<Result<Value, HostError>>(run(
                include_str!("../../../examples/widgets/todo/main.rn"),
                Std::from_arc(io.clone(), config),
            ))
            .unwrap();
            if count == 65 {
                assert!(result.is_err());
                assert!(io.display.lock().unwrap().is_empty());
            } else {
                result.unwrap();
                let frames = io.display.lock().unwrap();
                assert_eq!(frames.len(), 1);
                assert!(frames[0].text_bytes <= MAX_CANVAS_TEXT_BYTES);
                assert!(frame_has_text(&frames[0], "6 / 64 TODO"));
                assert_eq!(
                    frames[0]
                        .commands
                        .iter()
                        .filter(|c| matches!(c, CanvasCommandOwned::Rect { .. }))
                        .count(),
                    6
                );
            }
        }
    }

    #[test]
    fn todo_rendered_frame_preserves_pixels_when_json_formatting_changes() {
        use ssd2683::{Framebuffer, PanelColor, FRAMEBUFFER_LEN, HEIGHT, WIDTH};
        let io = Arc::new(FakeIo::default());
        let body = include_str!("../../../examples/widgets/todo/feed.example.json");
        for feed in [body.to_owned(), format!("  {body}  ")] {
            io.http_results
                .lock()
                .unwrap()
                .push_back(Ok(HttpResponse::new(200, feed)));
            let mut config = test_config();
            config.widget_config = r#"{"url":"https://example.test/todo"}"#.into();
            successful_entry(run(
                include_str!("../../../examples/widgets/todo/main.rn"),
                Std::from_arc(io.clone(), config),
            ));
        }
        let frames = io.display.lock().unwrap();
        let mut a = [0; FRAMEBUFFER_LEN];
        let mut b = [0; FRAMEBUFFER_LEN];
        panelet_ui::render_canvas(
            &mut Framebuffer::new(&mut a, PanelColor::White),
            &mapped_canvas(&frames[0]),
        )
        .unwrap();
        let mut rendered = Framebuffer::new(&mut b, PanelColor::White);
        panelet_ui::render_canvas(&mut rendered, &mapped_canvas(&frames[1])).unwrap();
        assert_eq!(a.as_slice(), rendered.as_bytes());
        assert_eq!(rendered.pixel(13, 56).unwrap(), PanelColor::Red);
        for (x, y) in [(0, 0), (399, 0), (0, 299), (399, 299)] {
            assert_eq!(rendered.pixel(x, y).unwrap(), PanelColor::White);
        }
        if let Ok(path) = std::env::var("NOTE4C_TODO_PREVIEW") {
            let mut ppm = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    ppm.extend_from_slice(match rendered.pixel(x, y).unwrap() {
                        PanelColor::Black => &[0, 0, 0],
                        PanelColor::White => &[255, 255, 255],
                        PanelColor::Yellow => &[255, 215, 0],
                        PanelColor::Red => &[220, 0, 0],
                    });
                }
            }
            std::fs::write(path, ppm).unwrap();
        }
    }

    #[test]
    fn todo_empty_feed_clears_old_tasks_and_credentials_require_https() {
        let io = Arc::new(FakeIo::default());
        let mut config = test_config();
        config.widget_config =
            r#"{"url":"http://example.test/todo","bearer_token":"TEST_ONLY"}"#.into();
        let value = run(
            include_str!("../../../examples/widgets/todo/main.rn"),
            Std::from_arc(io.clone(), config),
        );
        assert!(rune::from_value::<Result<Value, HostError>>(value)
            .unwrap()
            .is_err());
        assert!(io.http_requests.lock().unwrap().is_empty());
        io.http_results
            .lock()
            .unwrap()
            .push_back(Ok(HttpResponse::new(200, r#"{"version":1,"items":[]}"#)));
        let mut config = test_config();
        config.widget_config = r#"{"url":"https://example.test/todo"}"#.into();
        successful_entry(run(
            include_str!("../../../examples/widgets/todo/main.rn"),
            Std::from_arc(io.clone(), config),
        ));
        assert!(frame_has_text(&io.display.lock().unwrap()[0], "暂无待办"));
    }

    fn frame_has_text(frame: &CanvasFrame, expected: &str) -> bool {
        frame.commands.iter().any(
            |command| matches!(command, CanvasCommandOwned::Text { text, .. } | CanvasCommandOwned::TextSized {text,..} if text == expected),
        )
    }

    #[test]
    fn example_widget_mounts_execute_against_the_host_abi() {
        let calendar_io = Arc::new(FakeIo::default());
        successful_entry(run(
            include_str!("../../../examples/widgets/calendar/main.rn"),
            Std::from_arc(calendar_io.clone(), test_config()),
        ));
        let calendar_frames = calendar_io.display.lock().unwrap();
        assert_eq!(calendar_frames.len(), 1);
        assert!(calendar_frames[0].commands.len() <= MAX_CANVAS_COMMANDS);
        assert!(calendar_frames[0].text_bytes <= MAX_CANVAS_TEXT_BYTES);
        for expected in ["2026-09-05", "星期六", "丙午年", "农历七月廿四", "1", "30"] {
            assert!(frame_has_text(&calendar_frames[0], expected));
        }
        drop(calendar_frames);

        let album_io = Arc::new(FakeIo::default());
        for (path, text) in [
            (
                "album.json",
                include_str!("../../../examples/widgets/photo-album/resources/album.json"),
            ),
            (
                "captions/morning.txt",
                include_str!(
                    "../../../examples/widgets/photo-album/resources/captions/morning.txt"
                ),
            ),
            (
                "captions/market.txt",
                include_str!("../../../examples/widgets/photo-album/resources/captions/market.txt"),
            ),
            (
                "captions/harbor.txt",
                include_str!("../../../examples/widgets/photo-album/resources/captions/harbor.txt"),
            ),
        ] {
            album_io
                .resources
                .lock()
                .unwrap()
                .insert(path.into(), text.into());
        }
        let album_source = include_str!("../../../examples/widgets/photo-album/main.rn");
        for expected_frames in 1..=3 {
            successful_entry(run(
                album_source,
                Std::from_arc(album_io.clone(), test_config()),
            ));
            assert_eq!(album_io.display.lock().unwrap().len(), expected_frames);
        }
        let album_frames = album_io.display.lock().unwrap();
        for (frame, expected) in
            album_frames
                .iter()
                .zip(["MORNING RIDGE", "MARKET STREET", "NIGHT HARBOR"])
        {
            assert!(frame_has_text(frame, expected));
            assert!(frame.commands.len() <= MAX_CANVAS_COMMANDS);
            assert!(frame.text_bytes <= MAX_CANVAS_TEXT_BYTES);
        }
        drop(album_frames);
        assert_eq!(
            album_io
                .kv
                .lock()
                .unwrap()
                .get("album.current")
                .map(String::as_str),
            Some("morning")
        );
    }

    fn mapped_canvas(frame: &CanvasFrame) -> Vec<panelet_ui::CanvasCommand<'_>> {
        use panelet_ui::CanvasCommand;
        frame
            .commands
            .iter()
            .map(|command| match command {
                CanvasCommandOwned::Clear { color } => CanvasCommand::Clear {
                    color: test_panel_color(*color),
                },
                CanvasCommandOwned::Pixel { x, y, color } => CanvasCommand::Pixel {
                    x: *x,
                    y: *y,
                    color: test_panel_color(*color),
                },
                CanvasCommandOwned::Line {
                    x0,
                    y0,
                    x1,
                    y1,
                    color,
                } => CanvasCommand::Line {
                    x0: *x0,
                    y0: *y0,
                    x1: *x1,
                    y1: *y1,
                    color: test_panel_color(*color),
                },
                CanvasCommandOwned::Rect {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => CanvasCommand::Rect {
                    x: *x,
                    y: *y,
                    width: *width,
                    height: *height,
                    color: test_panel_color(*color),
                },
                CanvasCommandOwned::FillRect {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => CanvasCommand::FillRect {
                    x: *x,
                    y: *y,
                    width: *width,
                    height: *height,
                    color: test_panel_color(*color),
                },
                CanvasCommandOwned::Text {
                    x,
                    y,
                    text,
                    color,
                    scale,
                } => CanvasCommand::Text {
                    x: *x,
                    y: *y,
                    text,
                    color: test_panel_color(*color),
                    scale: *scale,
                },
                CanvasCommandOwned::TextSized {
                    x,
                    y,
                    text,
                    color,
                    height,
                } => CanvasCommand::TextSized {
                    x: *x,
                    y: *y,
                    text,
                    color: test_panel_color(*color),
                    height: *height,
                },
            })
            .collect()
    }

    #[test]
    fn selected_calendar_frame_renders_with_safe_corners_and_one_today_fill() {
        use panelet_ui::render_canvas;
        use ssd2683::{Framebuffer, PanelColor, FRAMEBUFFER_LEN, HEIGHT, WIDTH};

        let io = Arc::new(FakeIo::default());
        successful_entry(run(
            include_str!("../../../examples/widgets/calendar/main.rn"),
            Std::from_arc(io.clone(), test_config()),
        ));
        let frame = io.display.lock().unwrap()[0].clone();
        let commands = mapped_canvas(&frame);
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut rendered = Framebuffer::new(&mut storage, PanelColor::White);
        render_canvas(&mut rendered, &commands).unwrap();

        for point in [(0, 0), (399, 0), (0, 299), (399, 299)] {
            assert_eq!(rendered.pixel(point.0, point.1).unwrap(), PanelColor::White);
        }
        assert_eq!(rendered.pixel(338, 80).unwrap(), PanelColor::Yellow);
        assert_eq!(rendered.pixel(300, 80).unwrap(), PanelColor::White);

        if let Ok(path) = std::env::var("NOTE4C_CALENDAR_PREVIEW") {
            let mut ppm = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    ppm.extend_from_slice(match rendered.pixel(x, y).unwrap() {
                        PanelColor::Black => &[0, 0, 0],
                        PanelColor::White => &[255, 255, 255],
                        PanelColor::Yellow => &[255, 215, 0],
                        PanelColor::Red => &[220, 0, 0],
                    });
                }
            }
            std::fs::write(path, ppm).unwrap();
        }
    }

    fn test_panel_color(color: u8) -> ssd2683::PanelColor {
        match color {
            0 => ssd2683::PanelColor::Black,
            1 => ssd2683::PanelColor::White,
            2 => ssd2683::PanelColor::Yellow,
            _ => ssd2683::PanelColor::Red,
        }
    }

    #[test]
    fn lunar_time_api_produces_display_ready_labels() {
        let reading = lunar_reading(2026, 9, 5).unwrap();
        assert_eq!(reading.year_name, "丙午");
        assert_eq!(reading.month_name, "七月");
        assert_eq!(reading.day_name, "廿四");
        assert!(!reading.leap_month);

        let leap = lunar_reading(2017, 7, 28).unwrap();
        assert_eq!(leap.month_name, "闰六月");
        assert_eq!(leap.day_name, "初六");
        assert!(leap.leap_month);
    }

    #[test]
    fn cold_future_has_no_side_effect_until_awaited() {
        let io = Arc::new(FakeIo::default());
        let host = Std::from_arc(io.clone(), test_config());
        let value = run(
            r#"
                pub async fn main() {
                    let future = os::http::get("https://example.test/data");
                    7
                }
            "#,
            host,
        );
        assert_eq!(rune::from_value::<i64>(value).unwrap(), 7);
        assert_eq!(io.count(), 0);
    }

    #[test]
    fn filesystem_futures_are_cold_and_canonicalize_paths() {
        let io = Arc::new(FakeIo::default());
        let host = Std::from_arc(io.clone(), test_config());
        let value = run(
            r#"
                pub async fn main() {
                    let pending = os::fs::private::write_text("notes/./today.txt", "hello");
                    os::runtime::ok(7)
                }
            "#,
            host,
        );
        assert_eq!(rune::from_value::<i64>(successful_entry(value)).unwrap(), 7);
        assert_eq!(io.count(), 0);
        assert!(io.private_files.lock().unwrap().is_empty());

        let value = run(
            r#"
                pub async fn main() {
                    os::fs::private::write_text("notes/./today.txt", "hello").await?;
                    let exists = os::fs::private::exists("notes/today.txt").await?;
                    let text = os::fs::private::read_text("notes/./today.txt").await?;
                    let paths = os::fs::private::list("notes").await?;
                    if !exists || text.unwrap() != "hello" || paths.len() != 1 { return os::runtime::ok(0); }
                    os::runtime::ok(1)
                }
            "#,
            Std::from_arc(io.clone(), test_config()),
        );
        assert_eq!(rune::from_value::<i64>(successful_entry(value)).unwrap(), 1);
        assert_eq!(io.count(), 4);
        assert_eq!(
            io.private_files
                .lock()
                .unwrap()
                .get("notes/today.txt")
                .map(String::as_str),
            Some("hello")
        );
    }

    #[test]
    fn filesystem_namespaces_and_shared_write_capability_are_separate() {
        let io = Arc::new(FakeIo::default());
        io.shared_files
            .lock()
            .unwrap()
            .insert("shared/readme.txt".to_owned(), "public".to_owned());
        let mut config = test_config();
        config.capabilities = vec![CAP_FS_PRIVATE.to_owned(), CAP_FS_SHARED_READ.to_owned()];
        let value = run(
            r#"
                pub async fn main() {
                    let denied = os::fs::shared::write_text("shared/out.txt", "nope").await;
                    let exists = os::fs::shared::exists("./shared/readme.txt").await?;
                    let text = os::fs::shared::read_text("shared/readme.txt").await?;
                    let paths = os::fs::shared::list("shared").await?;
                    if denied.is_ok() || !exists || text.unwrap() != "public" || paths.len() != 1 {
                        return os::runtime::ok(0);
                    }
                    os::runtime::ok(1)
                }
            "#,
            Std::from_arc(io.clone(), config),
        );
        assert_eq!(rune::from_value::<i64>(successful_entry(value)).unwrap(), 1);
        assert_eq!(io.count(), 3);
        assert!(!io
            .shared_files
            .lock()
            .unwrap()
            .contains_key("shared/out.txt"));
    }

    #[test]
    fn filesystem_paths_and_budgets_are_strict() {
        assert_eq!(
            normalize_fs_path("notes/./today.txt").unwrap(),
            "notes/today.txt"
        );
        assert_eq!(normalize_fs_prefix("").unwrap(), "");
        for invalid in [
            "",
            "/absolute",
            "\\absolute",
            "notes//today.txt",
            "notes/../today.txt",
            "notes/",
            "notes\\today.txt",
            "drive:C/today.txt",
            "notes\0today.txt",
        ] {
            assert!(normalize_fs_path(invalid).is_err(), "accepted {invalid:?}");
        }
        assert!(normalize_fs_prefix("notes/").is_err());
        assert!(validate_fs_list("notes", vec!["other/file.txt".to_owned()]).is_err());

        let io = Arc::new(FakeIo::default());
        let invocation = Invocation {
            io,
            config: test_config(),
            state: InternalMutex::new(InvocationState::default()),
            started_at: Instant::now(),
            drawing: InternalMutex::new(Drawing::default()),
        };
        invocation.reserve_fs_bytes(FS_TOTAL_MAX_BYTES).unwrap();
        assert!(invocation.reserve_fs_bytes(1).is_err());
        for _ in 0..FS_MAX_CALLS {
            invocation.start_fs(CAP_FS_PRIVATE).unwrap();
        }
        assert!(invocation.start_fs(CAP_FS_PRIVATE).is_err());
    }

    #[test]
    fn direct_await_and_grouped_wait_all_start_work() {
        let io = Arc::new(FakeIo::default());
        let host = Std::from_arc(io.clone(), test_config());
        let value = run(
            r#"
                pub async fn main() {
                    let one = os::http::get_text("https://example.test/one").await.unwrap();
                    let two = os::http::get_text("https://example.test/two");
                    let all = os::runtime::wait_all([two]).await.unwrap();
                    if one != "body:https://example.test/one" { return 0; }
                    if all[0].is_err() { return 0; }
                    2
                }
            "#,
            host,
        );
        assert_eq!(rune::from_value::<i64>(value).unwrap(), 2);
        assert_eq!(io.count(), 2);
    }

    #[test]
    fn result_error_and_canvas_limits_are_visible() {
        let io = Arc::new(FakeIo::default());
        let mut config = test_config();
        config.capabilities = vec![CAP_SCREEN_PRESENT.to_owned()];
        let host = Std::from_arc(io.clone(), config);
        let value = run(
            r#"
                pub async fn main() {
                    let canvas = os::screen::get_context().unwrap();
                    for i in 0..128 { canvas.pixel([i % 400, 0], 0).unwrap(); }
                    let failed = canvas.pixel([0, 0], 9);
                    if failed.is_ok() { return 0; }
                    let pending = os::log::info("should be denied");
                    let denied = pending.await;
                    if denied.is_err() { 1 } else { 0 }
                }
            "#,
            host,
        );
        assert_eq!(rune::from_value::<i64>(value).unwrap(), 1);
        assert_eq!(io.count(), 0);
    }

    #[test]
    fn entry_result_preserves_structured_host_error_for_job_adapter() {
        let io = Arc::new(FakeIo::default());
        let mut config = test_config();
        config.capabilities.clear();
        let host = Std::from_arc(io.clone(), config);
        let value = run(
            r#"
                pub async fn main() {
                    os::log::info("denied").await
                }
            "#,
            host,
        );
        let returned = rune::from_value::<Result<Value, HostError>>(value).unwrap();
        let error = returned.unwrap_err();
        assert_eq!(error.code, "capability_denied");
        assert_eq!(error.group, "capability");
        assert!(!error.retryable);
        assert_eq!(io.count(), 0);
    }

    #[test]
    fn explicit_success_result_is_available_to_widget_entry() {
        let io = Arc::new(FakeIo::default());
        let host = Std::from_arc(io, test_config());
        let value = run(
            r#"
                pub async fn main() {
                    os::runtime::ok(7)
                }
            "#,
            host,
        );
        let returned = rune::from_value::<Result<Value, HostError>>(value).unwrap();
        assert_eq!(rune::from_value::<i64>(returned.unwrap()).unwrap(), 7);
    }

    #[test]
    fn management_page_style_canvas_entry_compiles_and_runs() {
        let io = Arc::new(FakeIo::default());
        let host = Std::from_arc(io.clone(), test_config());
        let value = run(
            r#"
                pub async fn main() {
                    let canvas = os::screen::get_context()?;
                    canvas.clear(1)?;
                    canvas.text([16, 20], "HELLO NOTE4C", 0, 2)?;
                    let receipt = canvas.flush().await?;
                    os::log::info("SCREEN PRESENT QUEUED").await?;
                    os::runtime::ok(receipt.submission_id)
                }
            "#,
            host,
        );
        let returned = rune::from_value::<Result<Value, HostError>>(value).unwrap();
        assert_eq!(rune::from_value::<u64>(returned.unwrap()).unwrap(), 1);
        assert_eq!(io.count(), 2);
    }

    #[test]
    fn json_subvalues_round_trip_control_escapes_and_surrogate_pairs() {
        let root = JsonValue::parse(r#"{"text":"\b\f\u0000\u001f\uD83D\uDE00"}"#).unwrap();
        let JsonNode::Object(entries) = root.node().unwrap() else {
            panic!("expected object")
        };
        let JsonNode::String(text) = &entries[0].1 else {
            panic!("expected string")
        };
        assert_eq!(text, "\u{0008}\u{000c}\u{0000}\u{001f}😀");
        let encoded = entries[0].1.to_json();
        assert_eq!(encoded, r#""\b\f\u0000\u001f😀""#);
        assert!(JsonValue::parse(&encoded).is_ok());
        assert!(JsonValue::parse(r#""\uDE00""#).is_err());
    }

    #[test]
    fn http_lan_and_json_sugar_are_bounded() {
        let io = Arc::new(FakeIo::default());
        let host = Std::from_arc(io.clone(), test_config());
        let value = run(
            r#"
                pub async fn main() {
                    let request = os::http::request("GET", "http://192.168.1.2/data").unwrap();
                    let response = os::http::fetch(request).await.unwrap();
                    let json = os::json::parse("{\"n\": 42, \"ok\": true}").unwrap();
                    if !response.is_success() { return 0; }
                    if json.get("n").unwrap().unwrap().integer().unwrap().unwrap() != 42 { return 0; }
                    if json.get("ok").unwrap().unwrap().boolean().unwrap().unwrap() != true { return 0; }
                    1
                }
            "#,
            host,
        );
        assert_eq!(rune::from_value::<i64>(value).unwrap(), 1);
        assert_eq!(io.count(), 1);
    }

    #[test]
    fn canvas_frame_is_owned_and_maps_all_commands() {
        let io = Arc::new(FakeIo::default());
        successful_entry(run(
            r#"
            pub async fn main() {
                let canvas = os::screen::get_context()?;
                canvas.clear(os::screen::BLACK)?;
                canvas.pixel([1, 2], os::screen::RED)?;
                canvas.line([1, 2, 3, 4], os::screen::YELLOW)?;
                canvas.rect([1, 2, 10, 11], os::screen::WHITE)?;
                canvas.fill_rect([2, 3, 4, 5], os::screen::BLACK)?;
                canvas.text([0, 0], "X", os::screen::RED, 1)?;
                canvas.flush().await?;
                os::runtime::ok(())
            }
        "#,
            Std::from_arc(io.clone(), test_config()),
        ));
        let frames = io.display.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].commands.len(), 6);
        assert_eq!(frames[0].text_bytes, 1);
        assert!(matches!(
            frames[0].commands[0],
            CanvasCommandOwned::Clear { color: 0 }
        ));
        assert!(matches!(
            frames[0].commands[1],
            CanvasCommandOwned::Pixel {
                x: 1,
                y: 2,
                color: 3
            }
        ));
        assert!(matches!(
            frames[0].commands[2],
            CanvasCommandOwned::Line { color: 2, .. }
        ));
        assert!(matches!(
            frames[0].commands[3],
            CanvasCommandOwned::Rect {
                width: 10,
                height: 11,
                color: 1,
                ..
            }
        ));
        assert!(matches!(
            frames[0].commands[4],
            CanvasCommandOwned::FillRect {
                width: 4,
                height: 5,
                color: 0,
                ..
            }
        ));
        assert!(
            matches!(&frames[0].commands[5], CanvasCommandOwned::Text { text, color: 3, scale: 1, .. } if text == "X")
        );
    }
}

#[path = "os.rs"]
mod os;
pub use os::OsBindings;

#[derive(Any, Clone, Debug, rune::alloc::clone::TryClone)]
#[rune(item = ::note4c)]
pub struct SubmissionReceipt {
    #[rune(get)]
    pub submission_id: u64,
}

#[rune::function(instance, path = flush)]
async fn canvas_flush(this: Ref<Canvas>) -> HostResult<SubmissionReceipt> {
    let invocation = &this.invocation;
    invocation.check_cancelled()?;
    invocation.start(CAP_SCREEN_PRESENT)?;
    let drawing = invocation
        .drawing
        .lock()
        .map_err(|_| HostError::cancelled())?;
    if !drawing.allowed {
        return Err(HostError::cancelled());
    }
    let broker = drawing.broker.as_ref().ok_or_else(HostError::cancelled)?;
    let frame = drawing.frame.as_ref().ok_or_else(HostError::cancelled)?;
    let now = invocation
        .config
        .uptime_ms
        .saturating_add(invocation.started_at.elapsed().as_millis() as u64);
    let submission_id = broker
        .submit(drawing.epoch, frame, now)
        .map_err(|_| HostError::quota("screen submission unavailable"))?;
    #[cfg(test)]
    invocation.io.on_submission(CanvasFrame {
        commands: drawing.commands.clone(),
        text_bytes: drawing.text_bytes,
    });
    invocation.io.screen_submitted();
    Ok(SubmissionReceipt { submission_id })
}

fn rasterize_command(bytes: &mut [u8], command: &CanvasCommandOwned) -> HostResult<()> {
    use panelet_ui::CanvasCommand;
    fn panel_color(color: u8) -> ssd2683::PanelColor {
        match color {
            0 => ssd2683::PanelColor::Black,
            1 => ssd2683::PanelColor::White,
            2 => ssd2683::PanelColor::Yellow,
            _ => ssd2683::PanelColor::Red,
        }
    }
    let mapped = match command {
        CanvasCommandOwned::Clear { color } => CanvasCommand::Clear {
            color: panel_color(*color),
        },
        CanvasCommandOwned::Pixel { x, y, color } => CanvasCommand::Pixel {
            x: *x,
            y: *y,
            color: panel_color(*color),
        },
        CanvasCommandOwned::Line {
            x0,
            y0,
            x1,
            y1,
            color,
        } => CanvasCommand::Line {
            x0: *x0,
            y0: *y0,
            x1: *x1,
            y1: *y1,
            color: panel_color(*color),
        },
        CanvasCommandOwned::Rect {
            x,
            y,
            width,
            height,
            color,
        } => CanvasCommand::Rect {
            x: *x,
            y: *y,
            width: *width,
            height: *height,
            color: panel_color(*color),
        },
        CanvasCommandOwned::FillRect {
            x,
            y,
            width,
            height,
            color,
        } => CanvasCommand::FillRect {
            x: *x,
            y: *y,
            width: *width,
            height: *height,
            color: panel_color(*color),
        },
        CanvasCommandOwned::Text {
            x,
            y,
            text,
            color,
            scale,
        } => CanvasCommand::Text {
            x: *x,
            y: *y,
            text,
            scale: *scale,
            color: panel_color(*color),
        },
        CanvasCommandOwned::TextSized {
            x,
            y,
            text,
            color,
            height,
        } => CanvasCommand::TextSized {
            x: *x,
            y: *y,
            text,
            height: *height,
            color: panel_color(*color),
        },
    };

    let bytes: &mut [u8; 30000] = bytes
        .try_into()
        .map_err(|_| HostError::invalid_argument("frame length"))?;
    let mut frame = ssd2683::Framebuffer::from_bytes(bytes);
    panelet_ui::render_canvas(&mut frame, &[mapped])
        .map_err(|_| HostError::invalid_argument("invalid canvas command"))?;
    Ok(())
}
