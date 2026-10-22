// SPDX-License-Identifier: MPL-2.0

#![forbid(unsafe_code)]

//! Transport-independent management RPC state and typed work queue.
//!
//! JSON is deliberately confined to this adapter crate. Hardware workers only
//! receive [`Command`] values and never see HTTP, BLE, sockets, or JSON.

pub mod wifi_profiles;

use std::collections::VecDeque;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

mod display_cache;
pub use display_cache::DisplayCache;
mod widget_settings;
use widget_settings::PollSchedule;
pub use widget_settings::{WidgetSettings, WIDGET_SETTINGS_MAX};

pub const RPC_BODY_MAX: usize = 64 * 1024;
pub const COMMAND_QUEUE_CAPACITY: usize = 8;
pub const JOB_CAPACITY: usize = 24;
pub const EVENT_CAPACITY: usize = 48;
pub const DISPLAY_TEXT_MAX: usize = 160;
pub const EVENT_PAGE_MAX: usize = 32;
pub const ADMIN_TOKEN_MIN_LEN: usize = 6;
pub const ADMIN_TOKEN_MAX_LEN: usize = 32;
pub const SERIAL_PROVISION_LINE_MAX: usize = 768;
pub const SERIAL_PROVISION_HELLO_PREFIX: &str = "N4C-HELLO/1 ";
pub const SERIAL_PROVISION_REQUEST_PREFIX: &str = "N4C-PROVISION/1 ";
pub const SERIAL_PROVISION_RESULT_PREFIX: &str = "N4C-RESULT/1 ";
pub const NTP_SERVER_MAX_LEN: usize = 128;
pub const UTC_OFFSET_MINUTES_MIN: i16 = -12 * 60;
pub const UTC_OFFSET_MINUTES_MAX: i16 = 14 * 60;
pub const DEFAULT_NTP_SERVER: &str = "pool.ntp.org";
pub const DEFAULT_UTC_OFFSET_MINUTES: i16 = 8 * 60;
pub const DEMO_WIDGET_ID: &str = "demo.hello";
pub const WIDGET_CAPACITY: usize = 4;
pub const WIDGET_SOURCE_MAX: usize = 5 * 1024;
pub const WIDGET_RESOURCE_ITEM_MAX: usize = 1024;
pub const WIDGET_RESOURCE_MAX: usize = 4 * 1024;
pub const WIDGET_RAM_MAX: usize = 20 * 1024;
pub const WIDGET_ABI: &str = "note4c.rune/3";
pub const CAP_TIME_READ: &str = "time.read";
pub const CAP_POWER_READ: &str = "power.read";
pub const CAP_WIFI_READ: &str = "wifi.read";
pub const CAP_SCREEN_PRESENT: &str = "screen.present";
pub const CAP_STORAGE_KV: &str = "storage.kv";
pub const CAP_LOG_WRITE: &str = "log.write";
pub const CAP_DEVICE_READ: &str = "device.read";
pub const CAP_HTTP_FETCH: &str = "http.fetch";
pub const CAP_HTTP_INSECURE_TLS: &str = "http.insecure_tls";
pub const CAP_FS_PRIVATE: &str = "fs.private";
pub const CAP_FS_SHARED_READ: &str = "fs.shared.read";
pub const CAP_FS_SHARED_WRITE: &str = "fs.shared.write";
pub const WIDGET_CAPABILITIES: &[&str] = &[
    CAP_TIME_READ,
    CAP_POWER_READ,
    CAP_WIFI_READ,
    CAP_SCREEN_PRESENT,
    CAP_STORAGE_KV,
    CAP_LOG_WRITE,
    CAP_DEVICE_READ,
    CAP_HTTP_FETCH,
    CAP_HTTP_INSECURE_TLS,
    CAP_FS_PRIVATE,
    CAP_FS_SHARED_READ,
    CAP_FS_SHARED_WRITE,
];
pub const HOST_PERMISSION_CAPACITY: usize = 8;
pub const HOST_PATTERN_MAX: usize = 512;
pub const HTTP_URL_MAX: usize = 512;
pub const KV_CLEANUP_QUEUE_CAPACITY: usize = 8;
const WIDGET_PACKAGE_ACCOUNTING_OVERHEAD: usize = 128;
const WIDGET_STRING_ACCOUNTING_OVERHEAD: usize = 32;
const READABLE_CREDENTIAL_ALPHABET: &[u8; 26] = b"ACEFGHJKMNPQRTUVWXYZ234679";
const READABLE_REJECTION_LIMIT: u8 = 234;

pub const TEST_IDS: &[&str] = &[
    "system_info",
    "buttons_led",
    "epd_quick",
    "epd_partial",
    "i2c_scan",
    "rtc_read",
    "battery",
    "status_screen",
];

pub fn normalize_display_text(text: &str) -> Result<String, &'static str> {
    let text = text.to_ascii_uppercase();
    if text.is_empty()
        || text.len() > DISPLAY_TEXT_MAX
        || !text.chars().all(|ch| {
            ch == '\n'
                || ch == ' '
                || ch == '-'
                || ch == '_'
                || ch == '/'
                || ch == '?'
                || ch == '.'
                || ch == ':'
                || ch.is_ascii_uppercase()
                || ch.is_ascii_digit()
        })
    {
        return Err("text contains unsupported characters or is too long");
    }
    Ok(text)
}

/// Derive the stable development SoftAP password from the factory identity.
///
/// The identity (normally the factory eFuse MAC) is not secret. This function
/// provides a repeatable per-device bootstrap value, not production-grade
/// authentication.
#[must_use]
pub fn derive_setup_ap_password(factory_identity: &[u8]) -> String {
    let raw = derive_readable_credential(factory_identity, b"SETUP-AP-PASSWORD", 12);
    format!("{}-{}-{}", &raw[..4], &raw[4..8], &raw[8..12])
}

/// Derive the independent eight-character development admin bootstrap token.
#[must_use]
pub fn derive_admin_bootstrap_token(factory_identity: &[u8]) -> String {
    derive_readable_credential(factory_identity, b"ADMIN-BOOTSTRAP", 8)
}

fn derive_readable_credential(identity: &[u8], domain: &[u8], length: usize) -> String {
    let mut output = String::with_capacity(length);
    let mut counter = 0_u32;
    while output.len() < length {
        let mut hash = Sha256::new();
        hash.update(b"NOTE4C\0");
        hash.update(domain);
        hash.update(b"\0V1\0");
        hash.update(identity);
        hash.update(counter.to_le_bytes());
        for byte in hash.finalize() {
            if byte < READABLE_REJECTION_LIMIT {
                output.push(READABLE_CREDENTIAL_ALPHABET[usize::from(byte % 26)] as char);
                if output.len() == length {
                    break;
                }
            }
        }
        counter = counter.wrapping_add(1);
    }
    output
}

pub fn validate_admin_token(token: &str) -> Result<(), &'static str> {
    if !(ADMIN_TOKEN_MIN_LEN..=ADMIN_TOKEN_MAX_LEN).contains(&token.len())
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("admin token must contain 6..=32 ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

/// Map ESP-IDF `wifi_err_reason_t` values to bounded, credential-free codes.
///
/// The numeric values are part of ESP-IDF's public Wi-Fi event ABI. Unknown
/// or less actionable reasons deliberately collapse to a generic code rather
/// than being formatted from untrusted runtime data.
#[must_use]
pub const fn classify_wifi_disconnect_reason(reason: u16) -> &'static str {
    match reason {
        201 => "wifi_ap_not_found",
        2 => "wifi_authentication_timeout",
        15 | 202 | 204 => "wifi_authentication_failed",
        23 | 29 | 210 | 211 => "wifi_security_incompatible",
        5 | 9 | 203 => "wifi_association_failed",
        212 => "wifi_signal_too_weak",
        200 => "wifi_beacon_timeout",
        _ => "wifi_connection_failed",
    }
}

pub struct SerialProvisionRequest {
    pub request_id: String,
    pub credentials: WifiCredentials,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerialProvisionError {
    pub request_id: Option<String>,
    pub code: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialProvisionState {
    Ready,
    Accepted,
    Succeeded,
    Pending,
    Failed,
}

impl SerialProvisionState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Accepted => "accepted",
            Self::Succeeded => "succeeded",
            Self::Pending => "pending",
            Self::Failed => "failed",
        }
    }
}

pub fn parse_serial_hello(line: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(line).ok()?;
    let request_id = text
        .trim_end_matches(['\r', '\n'])
        .strip_prefix(SERIAL_PROVISION_HELLO_PREFIX)?;
    valid_serial_request_id(request_id).then_some(request_id)
}

fn valid_serial_request_id(request_id: &str) -> bool {
    request_id.len() == 16
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn parse_serial_provision_request(
    line: &[u8],
) -> Result<SerialProvisionRequest, SerialProvisionError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RawRequest {
        id: String,
        ssid: String,
        password: String,
    }

    if line.len() > SERIAL_PROVISION_LINE_MAX {
        return Err(SerialProvisionError {
            request_id: None,
            code: "line_too_long",
        });
    }
    let text = std::str::from_utf8(line).map_err(|_| SerialProvisionError {
        request_id: None,
        code: "invalid_utf8",
    })?;
    let text = text.trim_end_matches(['\r', '\n']);
    let payload =
        text.strip_prefix(SERIAL_PROVISION_REQUEST_PREFIX)
            .ok_or(SerialProvisionError {
                request_id: None,
                code: "not_provision_request",
            })?;
    let raw: RawRequest = serde_json::from_str(payload).map_err(|_| SerialProvisionError {
        request_id: None,
        code: "invalid_json",
    })?;
    if !valid_serial_request_id(&raw.id) {
        return Err(SerialProvisionError {
            request_id: None,
            code: "invalid_request_id",
        });
    }
    let request_id = raw.id;
    let credentials =
        WifiCredentials::new(raw.ssid, raw.password).map_err(|_| SerialProvisionError {
            request_id: Some(request_id.clone()),
            code: "invalid_credentials",
        })?;
    Ok(SerialProvisionRequest {
        request_id,
        credentials,
    })
}

#[must_use]
pub fn format_serial_provision_result(
    request_id: &str,
    state: SerialProvisionState,
    job_id: Option<u32>,
    code: Option<&str>,
) -> String {
    let mut value = json!({"id": request_id, "state": state.as_str()});
    if let Some(job_id) = job_id {
        value["job_id"] = json!(job_id);
    }
    if let Some(code) = code {
        value["code"] = json!(code);
    }
    format!(
        "{}{}\n",
        SERIAL_PROVISION_RESULT_PREFIX,
        serde_json::to_string(&value).expect("serial result JSON is serializable")
    )
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WifiCredentials {
    ssid: String,
    password: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeConfiguration {
    ntp_server: String,
    utc_offset_minutes: i16,
}

impl Default for TimeConfiguration {
    fn default() -> Self {
        Self {
            ntp_server: DEFAULT_NTP_SERVER.into(),
            utc_offset_minutes: DEFAULT_UTC_OFFSET_MINUTES,
        }
    }
}

impl TimeConfiguration {
    pub fn new(ntp_server: String, utc_offset_minutes: i16) -> Result<Self, &'static str> {
        let ntp_server = validate_ntp_server(&ntp_server)?;
        if !(UTC_OFFSET_MINUTES_MIN..=UTC_OFFSET_MINUTES_MAX).contains(&utc_offset_minutes) {
            return Err("utc_offset_minutes must be between -720 and 840");
        }
        Ok(Self {
            ntp_server,
            utc_offset_minutes,
        })
    }

    pub fn ntp_server(&self) -> &str {
        &self.ntp_server
    }

    pub fn utc_offset_minutes(&self) -> i16 {
        self.utc_offset_minutes
    }
}

fn validate_ntp_server(server: &str) -> Result<String, &'static str> {
    if server.is_empty()
        || server.len() > NTP_SERVER_MAX_LEN
        || !server.is_ascii()
        || server.trim() != server
        || server.contains(['/', ':', '\\', '@', '#', '?', '\0'])
    {
        return Err("ntp_server must be an ASCII hostname or IPv4 address");
    }
    if server.parse::<Ipv4Addr>().is_ok() {
        return Ok(server.to_owned());
    }
    if server.split('.').any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err("ntp_server must be an ASCII hostname or IPv4 address");
    }
    Ok(server.to_ascii_lowercase())
}

impl WifiCredentials {
    pub fn new(ssid: String, password: String) -> Result<Self, &'static str> {
        if ssid.is_empty() || ssid.len() > 32 {
            return Err("ssid must contain 1..=32 bytes");
        }
        if !(password.is_empty() || (8..=63).contains(&password.len())) {
            return Err("password must be empty or contain 8..=63 bytes");
        }
        Ok(Self { ssid, password })
    }

    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Display for WifiCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WifiCredentials {{ ssid: {:?}, password: <redacted> }}",
            self.ssid
        )
    }
}

impl fmt::Debug for WifiCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WifiCredentials")
            .field("ssid", &self.ssid)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    WifiScan,
    WifiConfigure(WifiCredentials),
    WifiForget {
        ssid: String,
    },
    TimeConfigure(TimeConfiguration),
    TimeSync,
    DisplayText {
        text: String,
    },
    DisplayStatus,
    TestRun {
        test: String,
    },
    WidgetActivate {
        id: String,
    },
    WidgetMount {
        id: String,
    },
    WidgetInput {
        id: String,
        keycode: i64,
        kind: i64,
        held_ms: u64,
    },
    SystemMode {
        diagnostic: bool,
    },
    ScreenRetry,
    WidgetPoll {
        id: String,
    },
    WidgetConfigure {
        id: String,
        settings: WidgetSettings,
    },
    WidgetUpload(WidgetCandidate),
    WidgetDelete {
        id: String,
    },
}

impl Command {
    pub fn worker(&self) -> Worker {
        match self {
            Self::WifiScan
            | Self::WifiConfigure(_)
            | Self::WifiForget { .. }
            | Self::TimeConfigure(_)
            | Self::TimeSync => Worker::Network,
            Self::SystemMode { .. }
            | Self::ScreenRetry
            | Self::WidgetMount { .. }
            | Self::WidgetInput { .. }
            | Self::WidgetActivate { .. }
            | Self::WidgetPoll { .. }
            | Self::WidgetConfigure { .. }
            | Self::WidgetUpload(_)
            | Self::WidgetDelete { .. } => Worker::Widget,
            Self::DisplayText { .. } | Self::DisplayStatus | Self::TestRun { .. } => Worker::Main,
        }
    }

    fn method(&self) -> &'static str {
        match self {
            Self::WifiScan => "wifi.scan",
            Self::WifiConfigure(_) => "wifi.configure",
            Self::WifiForget { .. } => "wifi.forget",
            Self::TimeConfigure(_) => "time.configure",
            Self::TimeSync => "time.sync",
            Self::DisplayText { .. } => "display.text",
            Self::DisplayStatus => "display.status",
            Self::TestRun { .. } => "test.run",
            Self::WidgetActivate { .. } => "widget.activate",
            Self::WidgetMount { .. } => "widget.mount",
            Self::WidgetInput { .. } => "widget.input",
            Self::SystemMode { .. } => "system.mode.set",
            Self::ScreenRetry => "screen.retry",
            Self::WidgetPoll { .. } => "widget.poll",
            Self::WidgetConfigure { .. } => "widget.configure",
            Self::WidgetUpload(_) => "widget.upload",
            Self::WidgetDelete { .. } => "widget.delete",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Worker {
    Network,
    Main,
    Widget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedCommand {
    pub routing_epoch: u64,
    pub job_id: u32,
    pub command: Command,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl JobState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
struct Job {
    id: u32,
    method: &'static str,
    state: JobState,
    result: Option<Value>,
}

#[derive(Clone, Debug)]
struct Event {
    seq: u64,
    kind: &'static str,
    job_id: Option<u32>,
    detail: Option<String>,
}

/// The stable machine-readable part of an error returned by the management
/// broker. `message` is for humans; callers should branch on `kind` and
/// `code`, and treat `details` as optional diagnostic data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Rpc,
    Widget,
    Job,
    UrlPolicy,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rpc => "rpc",
            Self::Widget => "widget",
            Self::Job => "job",
            Self::UrlPolicy => "url_policy",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuredError {
    pub kind: ErrorKind,
    pub code: &'static str,
    pub message: &'static str,
    pub details: Option<Value>,
}

pub type WidgetError = StructuredError;
pub type JobError = StructuredError;

/// Transport-neutral result for submitting a decoded widget package.
///
/// HTTP uses the status and structured error directly; the legacy RPC-shaped
/// adapter below only exists for broker unit tests and internal callers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WidgetEnqueueFailure {
    pub http_status: u16,
    pub rpc_code: i32,
    pub error: StructuredError,
}

impl StructuredError {
    pub fn rpc(code: &'static str, message: &'static str) -> Self {
        Self {
            kind: ErrorKind::Rpc,
            code,
            message,
            details: None,
        }
    }

    pub fn widget(code: &'static str, message: &'static str) -> Self {
        Self {
            kind: ErrorKind::Widget,
            code,
            message,
            details: None,
        }
    }

    pub fn job(code: &'static str, message: &'static str) -> Self {
        Self {
            kind: ErrorKind::Job,
            code,
            message,
            details: None,
        }
    }

    pub fn url_policy(code: &'static str, message: &'static str) -> Self {
        Self {
            kind: ErrorKind::UrlPolicy,
            code,
            message,
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for StructuredError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.kind.as_str(), self.code, self.message)
    }
}

impl std::error::Error for StructuredError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UrlScheme {
    Http,
    Https,
}

impl UrlScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HostMatcher {
    Exact(String),
    Subdomain(String),
    Ip(IpAddr),
}

/// A strict Chrome-style host permission pattern.
///
/// The accepted form is `http[s]://host[:port]/path`, with an exact hostname
/// or a single leading `*.` subdomain wildcard. `*` is also allowed once in
/// the path. Broad `*` host patterns and wildcard ports are intentionally not
/// accepted. The canonical form is available through [`Self::canonical`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostPattern {
    canonical: String,
    scheme: UrlScheme,
    host: HostMatcher,
    port: Option<u16>,
    path: String,
}

impl HostPattern {
    pub fn parse(input: &str) -> Result<Self, HostPatternError> {
        if input.is_empty() || input.len() > HOST_PATTERN_MAX {
            return Err(HostPatternError::new(
                "pattern_size",
                "host pattern is empty or too long",
            ));
        }
        reject_url_chars(input, true).map_err(|code| {
            HostPatternError::new(code, "host pattern contains unsupported characters")
        })?;
        if input.contains('?') || input.contains('#') || input.contains('@') {
            return Err(HostPatternError::new(
                "pattern_syntax",
                "host pattern must not contain query, fragment, or userinfo",
            ));
        }
        let (scheme, rest) = split_scheme(input).ok_or_else(|| {
            HostPatternError::new(
                "pattern_scheme",
                "host pattern scheme must be http or https",
            )
        })?;
        let slash = rest.find('/').ok_or_else(|| {
            HostPatternError::new("pattern_path", "host pattern must include a path")
        })?;
        let authority = &rest[..slash];
        let path = &rest[slash..];
        let (host, port) = parse_authority(authority, true)?;
        let path = normalize_path(path, true)
            .map_err(|code| HostPatternError::new(code, "host pattern path is invalid"))?;
        let canonical_host = canonical_host(&host);
        let canonical_authority = match port {
            Some(port) if port != scheme.default_port() => format!("{}:{}", canonical_host, port),
            None => canonical_host,
            Some(_) => canonical_host,
        };
        Ok(Self {
            canonical: format!("{}://{}{}", scheme.as_str(), canonical_authority, path),
            scheme,
            host,
            port,
            path,
        })
    }

    pub fn canonical(&self) -> &str {
        &self.canonical
    }

    pub fn scheme(&self) -> UrlScheme {
        self.scheme
    }

    /// Returns the canonical host. IPv6 literals omit the brackets here;
    /// brackets are restored in [`Self::canonical`].
    pub fn host(&self) -> String {
        match &self.host {
            HostMatcher::Exact(host) | HostMatcher::Subdomain(host) => host.clone(),
            HostMatcher::Ip(ip) => ip.to_string(),
        }
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn matches(&self, url: &ParsedUrl) -> bool {
        if self.scheme != url.scheme {
            return false;
        }
        if self.port.unwrap_or(self.scheme.default_port()) != url.port {
            return false;
        }
        let host_matches = match &self.host {
            HostMatcher::Exact(host) => url.ip.is_none() && url.host == *host,
            HostMatcher::Subdomain(suffix) => {
                url.ip.is_none()
                    && url.host.len() > suffix.len()
                    && url.host.ends_with(suffix)
                    && url.host.as_bytes()[url.host.len() - suffix.len() - 1] == b'.'
            }
            HostMatcher::Ip(expected) => url.ip == Some(*expected),
        };
        host_matches && path_matches(&self.path, &url.path)
    }

    pub fn matches_url(&self, url: &str) -> Result<bool, UrlPolicyError> {
        Ok(self.matches(&ParsedUrl::parse(url)?))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedUrl {
    scheme: UrlScheme,
    host: String,
    port: u16,
    path: String,
    ip: Option<IpAddr>,
}

impl ParsedUrl {
    pub fn parse(input: &str) -> Result<Self, UrlPolicyError> {
        parse_url(input)
    }

    pub fn scheme(&self) -> UrlScheme {
        self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn ip(&self) -> Option<IpAddr> {
        self.ip
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressClass {
    Public,
    Lan,
    Loopback,
    Unspecified,
    Multicast,
    Broadcast,
    DeviceSelf,
}

impl AddressClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Lan => "lan",
            Self::Loopback => "loopback",
            Self::Unspecified => "unspecified",
            Self::Multicast => "multicast",
            Self::Broadcast => "broadcast",
            Self::DeviceSelf => "device_self",
        }
    }

    fn forbidden(self) -> bool {
        matches!(
            self,
            Self::Loopback
                | Self::Unspecified
                | Self::Multicast
                | Self::Broadcast
                | Self::DeviceSelf
        )
    }
}

/// Classify a resolved destination. `device_self` is deliberately supplied by
/// the board/runtime caller because this crate does not own the device's
/// network identity.
pub fn classify_ip(ip: IpAddr, device_self: Option<IpAddr>) -> AddressClass {
    if device_self.is_some_and(|self_ip| equivalent_ip(ip, self_ip)) {
        return AddressClass::DeviceSelf;
    }
    match ip {
        IpAddr::V4(ip) => classify_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(mapped) = mapped_ipv4(ip) {
                return classify_ip(IpAddr::V4(mapped), device_self);
            }
            if ip.is_unspecified() {
                AddressClass::Unspecified
            } else if ip.is_loopback() {
                AddressClass::Loopback
            } else if ip.is_multicast() {
                AddressClass::Multicast
            } else if is_ipv6_lan(ip) {
                AddressClass::Lan
            } else {
                AddressClass::Public
            }
        }
    }
}

/// Compile and canonicalize manifest host permissions. Duplicate patterns are
/// rejected after canonicalization, so aliases cannot consume hidden slots.
pub fn compile_host_permissions(raw: &[String]) -> Result<Vec<HostPattern>, HostPatternError> {
    if raw.len() > HOST_PERMISSION_CAPACITY {
        return Err(HostPatternError::new(
            "pattern_count",
            "too many host permissions",
        ));
    }
    let mut compiled = Vec::with_capacity(raw.len());
    for value in raw {
        let pattern = HostPattern::parse(value)?;
        if compiled
            .iter()
            .any(|existing: &HostPattern| existing.canonical() == pattern.canonical())
        {
            return Err(HostPatternError::new(
                "pattern_duplicate",
                "duplicate host permission",
            ));
        }
        compiled.push(pattern);
    }
    Ok(compiled)
}

/// Authorize one request URL. Every redirect must call this function again
/// with its new URL (and freshly resolved addresses); no redirect is trusted
/// merely because the initial URL was permitted.
pub fn authorize_url(
    url: &str,
    host_permissions: &[HostPattern],
    insecure_tls: bool,
    resolved_ips: &[IpAddr],
    device_self: Option<IpAddr>,
) -> Result<ParsedUrl, UrlPolicyError> {
    let parsed = ParsedUrl::parse(url)?;
    if !host_permissions
        .iter()
        .any(|pattern| pattern.matches(&parsed))
    {
        return Err(UrlPolicyError::new(
            "host_permission_denied",
            "URL does not match a host permission",
        ));
    }
    if insecure_tls && parsed.scheme != UrlScheme::Https {
        return Err(UrlPolicyError::new(
            "insecure_tls_requires_https",
            "insecure TLS is only meaningful for HTTPS",
        ));
    }
    let mut addresses = resolved_ips.to_vec();
    if let Some(ip) = parsed.ip {
        // A literal URL is authoritative. Including it even when a caller
        // accidentally supplies a different resolver result prevents an
        // unsafe literal (for example 127.0.0.1) from being reclassified as
        // a public address.
        if !addresses.contains(&ip) {
            addresses.push(ip);
        }
    } else if addresses.is_empty() {
        return Err(UrlPolicyError::new(
            "dns_resolution_required",
            "DNS names must be resolved before request authorization",
        ));
    }
    for ip in &addresses {
        let class = classify_ip(*ip, device_self);
        if class.forbidden() {
            return Err(UrlPolicyError::new(
                class.as_str(),
                "destination address is not allowed",
            ));
        }
    }
    Ok(parsed)
}

/// Check the capability half of the HTTP policy. URL/redirect checks belong
/// to [`authorize_url`], while this function makes the code-level
/// `http.insecure_tls` request opt-in explicit.
pub fn authorize_http_capabilities(
    capabilities: &[String],
    insecure_tls: bool,
) -> Result<(), WidgetError> {
    if !capabilities.iter().any(|cap| cap == CAP_HTTP_FETCH) {
        return Err(StructuredError::widget(
            "http_capability_missing",
            "http.fetch capability is required",
        ));
    }
    if insecure_tls && !capabilities.iter().any(|cap| cap == CAP_HTTP_INSECURE_TLS) {
        return Err(StructuredError::widget(
            "insecure_tls_capability_missing",
            "http.insecure_tls capability is required",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostPatternError {
    pub code: &'static str,
    pub message: &'static str,
}

impl HostPatternError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    pub fn into_widget_error(self) -> WidgetError {
        StructuredError::widget(self.code, self.message)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UrlPolicyError {
    pub code: &'static str,
    pub message: &'static str,
}

impl UrlPolicyError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    pub fn into_structured_error(self) -> StructuredError {
        StructuredError::url_policy(self.code, self.message)
    }
}

fn split_scheme(input: &str) -> Option<(UrlScheme, &str)> {
    let (scheme, rest) = input.split_once("://")?;
    let scheme = match scheme {
        "http" => UrlScheme::Http,
        "https" => UrlScheme::Https,
        _ => return None,
    };
    Some((scheme, rest))
}

fn reject_url_chars(input: &str, allow_wildcard: bool) -> Result<(), &'static str> {
    if input
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err("pattern_characters");
    }
    if input.contains('\\') {
        return Err("pattern_backslash");
    }
    if !allow_wildcard && input.contains('*') {
        return Err("url_wildcard");
    }
    Ok(())
}

fn parse_authority(
    authority: &str,
    allow_wildcard: bool,
) -> Result<(HostMatcher, Option<u16>), HostPatternError> {
    if authority.is_empty() {
        return Err(HostPatternError::new("host_empty", "host is empty"));
    }
    if authority.contains('@') {
        return Err(HostPatternError::new(
            "userinfo_forbidden",
            "userinfo is not allowed",
        ));
    }

    let (host_text, port) = if authority.starts_with('[') {
        let close = authority.find(']').ok_or_else(|| {
            HostPatternError::new("ipv6_syntax", "bracketed IPv6 host is malformed")
        })?;
        let host_text = &authority[1..close];
        if host_text.is_empty() || host_text.contains('%') {
            return Err(HostPatternError::new(
                "ipv6_syntax",
                "IPv6 host is malformed",
            ));
        }
        let suffix = &authority[close + 1..];
        let port = if suffix.is_empty() {
            None
        } else if let Some(port) = suffix.strip_prefix(':') {
            Some(parse_port(port)?)
        } else {
            return Err(HostPatternError::new(
                "authority_syntax",
                "characters after bracketed host are invalid",
            ));
        };
        let ip = host_text.parse::<IpAddr>().map_err(|_| {
            HostPatternError::new("ipv6_syntax", "bracketed host must be an IPv6 literal")
        })?;
        if !matches!(ip, IpAddr::V6(_)) {
            return Err(HostPatternError::new(
                "ipv6_syntax",
                "brackets are only valid for IPv6 literals",
            ));
        }
        return Ok((HostMatcher::Ip(ip), port));
    } else {
        if authority.matches(':').count() > 1 {
            return Err(HostPatternError::new(
                "ipv6_syntax",
                "IPv6 literals must be enclosed in brackets",
            ));
        }
        match authority.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() => (host, Some(parse_port(port)?)),
            Some((_host, _)) => {
                return Err(HostPatternError::new("port_syntax", "port is empty"));
            }
            None => (authority, None),
        }
    };

    if host_text == "*" || host_text.is_empty() {
        return Err(HostPatternError::new(
            "host_wildcard",
            "broad host wildcard is not allowed",
        ));
    }
    if host_text.contains('*') {
        if !allow_wildcard || !host_text.starts_with("*.") {
            return Err(HostPatternError::new(
                "host_wildcard",
                "only a leading subdomain wildcard is allowed",
            ));
        }
        let suffix = host_text.strip_prefix("*.").unwrap_or_default();
        if suffix.split('.').count() < 2 || !valid_dns_name(suffix) {
            return Err(HostPatternError::new(
                "host_wildcard",
                "subdomain wildcard suffix is invalid",
            ));
        }
        return Ok((HostMatcher::Subdomain(suffix.to_ascii_lowercase()), port));
    }

    if let Ok(ip) = host_text.parse::<IpAddr>() {
        if ip.to_string() != host_text {
            return Err(HostPatternError::new(
                "ip_canonical",
                "IP literal must use canonical spelling",
            ));
        }
        return Ok((HostMatcher::Ip(ip), port));
    }
    if !valid_dns_name(host_text) {
        return Err(HostPatternError::new(
            "host_syntax",
            "hostname is malformed",
        ));
    }
    Ok((HostMatcher::Exact(host_text.to_ascii_lowercase()), port))
}

fn parse_port(port: &str) -> Result<u16, HostPatternError> {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(HostPatternError::new("port_syntax", "port must be numeric"));
    }
    let port = port
        .parse::<u32>()
        .map_err(|_| HostPatternError::new("port_syntax", "port is malformed"))?;
    if port == 0 || port > u32::from(u16::MAX) {
        return Err(HostPatternError::new(
            "port_range",
            "port must be 1..=65535",
        ));
    }
    Ok(port as u16)
}

fn valid_dns_name(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.ends_with('.') {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && label
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            && label
                .as_bytes()
                .last()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
    })
}

fn canonical_host(host: &HostMatcher) -> String {
    match host {
        HostMatcher::Exact(host) => host.clone(),
        HostMatcher::Subdomain(host) => format!("*.{}", host),
        HostMatcher::Ip(IpAddr::V4(ip)) => ip.to_string(),
        HostMatcher::Ip(IpAddr::V6(ip)) => format!("[{}]", ip),
    }
}

fn normalize_path(path: &str, allow_wildcard: bool) -> Result<String, &'static str> {
    if path.is_empty() || !path.starts_with('/') {
        return Err("path_syntax");
    }
    if contains_percent_code(path, b"2e")
        || contains_percent_code(path, b"2f")
        || contains_percent_code(path, b"5c")
        || contains_percent_code(path, b"00")
    {
        return Err("path_encoded_separator");
    }
    let parts: Vec<&str> = path.split('/').collect();
    let mut normalized = String::new();
    let mut wildcard_count = 0;
    for (index, part) in parts.iter().enumerate().skip(1) {
        let is_last = index + 1 == parts.len();
        if part.is_empty() {
            if !is_last {
                return Err("path_empty_segment");
            }
            normalized.push('/');
            continue;
        }
        if *part == "." {
            continue;
        }
        if *part == ".." {
            return Err("path_parent_segment");
        }
        for byte in part.bytes() {
            if byte.is_ascii_control() {
                return Err("path_characters");
            }
            if byte == b'*' {
                if !allow_wildcard {
                    return Err("path_wildcard");
                }
                wildcard_count += 1;
                if wildcard_count > 1 {
                    return Err("path_wildcard_count");
                }
            }
        }
        normalized.push('/');
        normalized.push_str(part);
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    Ok(normalized)
}

fn contains_percent_code(value: &str, code: &[u8; 2]) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(3).any(|window| {
        window[0] == b'%'
            && window[1].to_ascii_lowercase() == code[0]
            && window[2].to_ascii_lowercase() == code[1]
    })
}

fn path_matches(pattern: &str, path: &str) -> bool {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return pattern == path;
    };
    path.len() >= prefix.len() + suffix.len() && path.starts_with(prefix) && path.ends_with(suffix)
}

fn parse_url(input: &str) -> Result<ParsedUrl, UrlPolicyError> {
    if input.is_empty() || input.len() > HTTP_URL_MAX {
        return Err(UrlPolicyError::new("url_size", "URL is empty or too long"));
    }
    reject_url_chars(input, false)
        .map_err(|code| UrlPolicyError::new(code, "URL contains unsupported characters"))?;
    if input.contains('#') || input.contains('@') {
        return Err(UrlPolicyError::new(
            "url_syntax",
            "URL must not contain a fragment or userinfo",
        ));
    }
    let (scheme, rest) = split_scheme(input)
        .ok_or_else(|| UrlPolicyError::new("url_scheme", "URL scheme must be http or https"))?;
    let boundary = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..boundary];
    let suffix = &rest[boundary..];
    let (host, port) = parse_authority(authority, false)
        .map_err(|error| UrlPolicyError::new(error.code, error.message))?;
    let ip = match host {
        HostMatcher::Ip(ip) => Some(ip),
        HostMatcher::Exact(_) | HostMatcher::Subdomain(_) => None,
    };
    let host = match &host {
        HostMatcher::Exact(host) | HostMatcher::Subdomain(host) => host.clone(),
        HostMatcher::Ip(ip) => ip.to_string(),
    };
    let raw_path = suffix.split_once('?').map_or(suffix, |(path, _)| path);
    let raw_path = if raw_path.is_empty() { "/" } else { raw_path };
    let path = normalize_path(raw_path, false)
        .map_err(|code| UrlPolicyError::new(code, "URL path is invalid"))?;
    Ok(ParsedUrl {
        scheme,
        host,
        port: port.unwrap_or(scheme.default_port()),
        path,
        ip,
    })
}

fn classify_ipv4(ip: Ipv4Addr) -> AddressClass {
    let octets = ip.octets();
    if ip.is_unspecified() {
        AddressClass::Unspecified
    } else if ip.is_loopback() {
        AddressClass::Loopback
    } else if ip.is_multicast() {
        AddressClass::Multicast
    } else if ip == Ipv4Addr::new(255, 255, 255, 255) {
        AddressClass::Broadcast
    } else if (octets[0] == 10)
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
        || (octets[0] == 169 && octets[1] == 254)
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
    {
        AddressClass::Lan
    } else {
        AddressClass::Public
    }
}

fn mapped_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = ip.segments();
    if segments[..5] == [0, 0, 0, 0, 0] && segments[5] == 0xffff {
        Some(Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        ))
    } else {
        None
    }
}

fn equivalent_ip(left: IpAddr, right: IpAddr) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (IpAddr::V4(left), IpAddr::V6(right)) => mapped_ipv4(right) == Some(left),
        (IpAddr::V6(left), IpAddr::V4(right)) => mapped_ipv4(left) == Some(right),
        _ => false,
    }
}

fn is_ipv6_lan(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WidgetManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub abi: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub host_permissions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct WidgetRuntimeSpec {
    pub settings: WidgetSettings,
    pub id: String,
    pub source: String,
    pub capabilities: Vec<String>,
    pub host_permissions: Vec<String>,
    pub resources: Vec<WidgetResource>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WidgetCandidate {
    pub manifest: WidgetManifest,
    pub source: String,
    #[serde(default)]
    pub resources: Vec<WidgetResource>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WidgetResource {
    pub path: String,
    /// MVP resources are UTF-8 only. A binary blob endpoint belongs in a later
    /// storage-backed protocol and must not be smuggled through JSON/base64.
    pub text: String,
}

impl WidgetCandidate {
    fn ram_bytes(&self) -> usize {
        WidgetSettings::default().encoded_len()
            + widget_content_bytes(&self.manifest, &self.source, &self.resources)
    }
}

#[derive(Clone, Debug)]
struct WidgetPackage {
    manifest: WidgetManifest,
    source: String,
    resources: Vec<WidgetResource>,
    built_in: bool,
    settings: WidgetSettings,
    poll: PollSchedule,
    run_job: Option<u32>,
}

impl WidgetPackage {
    fn ram_bytes(&self) -> usize {
        self.settings.encoded_len()
            + widget_content_bytes(&self.manifest, &self.source, &self.resources)
    }
}

fn widget_content_bytes(
    manifest: &WidgetManifest,
    source: &str,
    resources: &[WidgetResource],
) -> usize {
    let string_count =
        5 + manifest.capabilities.len() + manifest.host_permissions.len() + resources.len() * 2;
    WIDGET_PACKAGE_ACCOUNTING_OVERHEAD
        + string_count * WIDGET_STRING_ACCOUNTING_OVERHEAD
        + manifest.id.len()
        + manifest.name.len()
        + manifest.version.len()
        + manifest.abi.len()
        + manifest.capabilities.iter().map(String::len).sum::<usize>()
        + manifest
            .host_permissions
            .iter()
            .map(String::len)
            .sum::<usize>()
        + source.len()
        + resources
            .iter()
            .map(|resource| resource.path.len() + resource.text.len())
            .sum::<usize>()
}

#[derive(Clone, Debug)]
pub struct NetworkStatus {
    pub provisioning: bool,
    pub mode: &'static str,
    pub ap_ssid: String,
    pub ap_ip: String,
    pub sta_ssid: Option<String>,
    pub sta_ip: Option<String>,
}

impl NetworkStatus {
    pub fn provisioning(ap_ssid: String) -> Self {
        Self {
            provisioning: true,
            mode: "softap",
            ap_ssid,
            ap_ip: "192.168.4.1".into(),
            sta_ssid: None,
            sta_ip: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SystemStatus {
    pub firmware: &'static str,
    pub free_heap: u32,
    pub minimum_free_heap: u32,
    pub display_busy: bool,
    pub main_screen: &'static str,
    pub active_widget: String,
    pub routing_epoch: u64,
    pub safe_mode: bool,
    pub mode: &'static str,
    pub input: Value,
    pub presentation: Value,
}

#[derive(Clone, Debug)]
struct TimeStatus {
    configuration: TimeConfiguration,
    sync_state: &'static str,
    source: &'static str,
    clock_valid: bool,
    last_ntp_sync_unix_ms: Option<i64>,
    rtc_updated: bool,
    last_error: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcReply {
    pub http_status: u16,
    pub body: String,
}

#[derive(Debug)]
pub struct Broker {
    commands: VecDeque<QueuedCommand>,
    jobs: VecDeque<Job>,
    events: VecDeque<Event>,
    next_job_id: u32,
    next_event_seq: u64,
    pub network: NetworkStatus,
    pub system: SystemStatus,
    time: TimeStatus,
    admin_token: String,
    widgets: Vec<WidgetPackage>,
    busy_widgets: Vec<String>,
    kv_cleanup: VecDeque<String>,
    storage_ready: bool,
    foreground_pending: bool,
    saved_wifi: Vec<String>,
}

impl Broker {
    pub fn new(network: NetworkStatus, firmware: &'static str, admin_token: String) -> Self {
        let main_screen = if network.provisioning {
            "provisioning"
        } else {
            "connecting"
        };
        Self {
            commands: VecDeque::with_capacity(COMMAND_QUEUE_CAPACITY),
            jobs: VecDeque::with_capacity(JOB_CAPACITY),
            events: VecDeque::with_capacity(EVENT_CAPACITY),
            next_job_id: 1,
            next_event_seq: 1,
            network,
            system: SystemStatus {
                firmware,
                free_heap: 0,
                minimum_free_heap: 0,
                display_busy: false,
                main_screen,
                active_widget: DEMO_WIDGET_ID.into(),
                routing_epoch: 1,
                mode: "normal",
                safe_mode: false,
                input: json!({"captured":0,"overflow":0,"pending":0}),
                presentation: json!({"state":"idle","refreshing":false}),
            },
            time: TimeStatus {
                configuration: TimeConfiguration::default(),
                sync_state: "waiting_network",
                source: "unset",
                clock_valid: false,
                last_ntp_sync_unix_ms: None,
                rtc_updated: false,
                last_error: None,
            },
            admin_token,
            widgets: vec![WidgetPackage {
                manifest: WidgetManifest {
                    id: DEMO_WIDGET_ID.into(),
                    name: "HELLO".into(),
                    version: "0.1.0".into(),
                    abi: WIDGET_ABI.into(),
                    capabilities: vec![
                        CAP_SCREEN_PRESENT.into(),
                        CAP_LOG_WRITE.into(),
                        CAP_STORAGE_KV.into(),
                    ],
                    host_permissions: Vec::new(),
                },
                source: "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }".into(),
                resources: Vec::new(),
                built_in: true,
                settings: WidgetSettings::default(),
                poll: PollSchedule::default(),
                run_job: None,
            }],
            busy_widgets: Vec::new(),
            kv_cleanup: VecDeque::with_capacity(KV_CLEANUP_QUEUE_CAPACITY),
            storage_ready: false,
            foreground_pending: false,
            saved_wifi: Vec::new(),
        }
    }

    /// Parse and dispatch one restricted JSON-RPC 2.0 request.
    ///
    /// Every request requires the independent admin token, including while the
    /// WPA2 SoftAP provisioning network is active.
    pub fn dispatch(&mut self, body: &[u8], supplied_token: Option<&str>) -> RpcReply {
        if body.len() > RPC_BODY_MAX {
            return error(Value::Null, 413, -32600, "request too large");
        }
        if supplied_token != Some(self.admin_token.as_str()) {
            return error(Value::Null, 401, -32001, "unauthorized");
        }

        let value: Value = match serde_json::from_slice(body) {
            Ok(value) => value,
            Err(_) => return error(Value::Null, 400, -32700, "parse error"),
        };
        if value.is_array() {
            return error(Value::Null, 400, -32600, "batch requests are not supported");
        }
        let Some(object) = value.as_object() else {
            return error(Value::Null, 400, -32600, "invalid request");
        };
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || !matches!(id, Value::String(_) | Value::Number(_))
        {
            return error(id, 400, -32600, "invalid request");
        }
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return error(id, 400, -32600, "invalid request");
        };
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));

        match method {
            "system.status" => ok(id, self.system_value()),
            "dashboard.get" => ok(id, self.dashboard_value()),
            "wifi.status" => ok(id, self.network_value()),
            "time.status" => ok(id, self.time_value()),
            "event.list" => self.event_list(id, params),
            "job.get" => self.job_get(id, params),
            "test.list" => ok(id, json!({ "tests": TEST_IDS })),
            "widget.list" => self.widget_list(id),
            "widget.get" => self.widget_get(id, params),
            "widget.status" => self.widget_status(id, params),
            "widget.configuration" => self.widget_configuration(id, params),
            "widget.configure" => self.widget_configure(id, params),
            "wifi.scan" => self.enqueue(id, Command::WifiScan),
            "wifi.saved" => ok(
                id,
                json!({"ssids":self.saved_wifi,"capacity":wifi_profiles::WIFI_PROFILE_CAPACITY}),
            ),
            "wifi.forget" => match params.get("ssid").and_then(Value::as_str) {
                Some(ssid) if !ssid.is_empty() && ssid.len() <= 32 => {
                    self.enqueue(id, Command::WifiForget { ssid: ssid.into() })
                }
                _ => error(id, 400, -32602, "invalid ssid"),
            },
            "wifi.configure" => self.configure_wifi(id, params),
            "time.configure" => self.configure_time(id, params),
            "time.sync" => self.enqueue(id, Command::TimeSync),
            "display.text" if self.system.mode == "diagnostic" => self.display_text(id, params),
            "display.status" if self.system.mode == "diagnostic" => {
                self.enqueue(id, Command::DisplayStatus)
            }
            "test.run" if self.system.mode == "diagnostic" => self.test_run(id, params),
            "widget.activate" => self.widget_activate(id, params),
            "widget.deactivate" => self.widget_activate(id, json!({"id":""})),
            "system.mode.set" => match params.get("mode").and_then(Value::as_str) {
                Some("normal") if !self.system.safe_mode => {
                    self.enqueue(id, Command::SystemMode { diagnostic: false })
                }
                Some("diagnostic") => self.enqueue(id, Command::SystemMode { diagnostic: true }),
                _ => error(id, 400, -32602, "invalid mode"),
            },
            "screen.retry" => self.enqueue(id, Command::ScreenRetry),
            "widget.delete" => self.widget_delete(id, params),
            _ => error(id, 404, -32601, "method not found"),
        }
    }

    /// Cheap transport-layer preflight used before allocating the request body.
    /// [`Self::dispatch`] deliberately repeats this check so callers cannot
    /// accidentally bypass authentication by omitting the preflight.
    pub fn is_authorized(&self, supplied_token: Option<&str>) -> bool {
        supplied_token == Some(self.admin_token.as_str())
    }

    pub fn replace_admin_token(&mut self, token: String) -> Result<(), &'static str> {
        validate_admin_token(&token)?;
        self.admin_token = token;
        Ok(())
    }

    #[must_use]
    pub fn job_state(&self, job_id: u32) -> Option<JobState> {
        self.jobs
            .iter()
            .find(|job| job.id == job_id)
            .map(|job| job.state)
    }

    /// Return only the stable error code from a failed job. The job result may
    /// contain richer authenticated diagnostics, but serial provisioning must
    /// never forward arbitrary result data or credentials.
    #[must_use]
    pub fn job_failure_code(&self, job_id: u32) -> Option<&str> {
        self.jobs
            .iter()
            .find(|job| job.id == job_id && job.state == JobState::Failed)?
            .result
            .as_ref()?
            .get("error")?
            .get("code")?
            .as_str()
    }

    pub fn take_for(&mut self, worker: Worker) -> Option<QueuedCommand> {
        let index = self
            .commands
            .iter()
            .position(|queued| queued.command.worker() == worker)?;
        let queued = self.commands.remove(index)?;
        self.set_job_state(queued.job_id, JobState::Running, None);
        Some(queued)
    }

    pub fn finish_job(&mut self, job_id: u32, succeeded: bool, result: Value) {
        let state = if succeeded {
            JobState::Succeeded
        } else {
            JobState::Failed
        };
        self.set_job_state(job_id, state, Some(result));
        self.push_event(
            if succeeded {
                "job.succeeded"
            } else {
                "job.failed"
            },
            Some(job_id),
            None,
        );
    }

    /// Finish a job with the same structured error shape exposed by RPC
    /// failures. Workers should use this for runtime failures instead of
    /// placing an ad-hoc string in the result object.
    pub fn finish_job_with_error(&mut self, job_id: u32, error: JobError) {
        let mut value = json!({
            "kind": error.kind.as_str(),
            "code": error.code,
            "message": error.message,
        });
        if let Some(details) = error.details {
            value["details"] = details;
        }
        self.finish_job(job_id, false, json!({"error": value}));
    }

    pub fn enqueue_host(&mut self, command: Command) -> Result<u32, JobError> {
        if self.commands.len() >= COMMAND_QUEUE_CAPACITY {
            return Err(StructuredError::job(
                "command_queue_full",
                "command queue is full",
            ));
        }
        let job_id = self.reserve_job_id()?;
        self.jobs.push_back(Job {
            id: job_id,
            method: command.method(),
            state: JobState::Queued,
            result: None,
        });
        if let Command::WidgetPoll { id }
        | Command::WidgetMount { id }
        | Command::WidgetInput { id, .. } = &command
        {
            if let Some(widget) = self
                .widgets
                .iter_mut()
                .find(|widget| widget.manifest.id == *id)
            {
                widget.run_job = Some(job_id);
            }
        }
        if let Command::WidgetInput { id, .. } = &command {
            if !self.busy_widgets.contains(id) { self.busy_widgets.push(id.clone()); }
        }
        self.commands.push_back(QueuedCommand {
            job_id,
            command,
            routing_epoch: self.system.routing_epoch,
        });
        self.push_event("job.queued", Some(job_id), None);
        Ok(job_id)
    }

    fn reserve_job_id(&mut self) -> Result<u32, JobError> {
        if self.jobs.len() >= JOB_CAPACITY {
            if let Some(index) = self
                .jobs
                .iter()
                .position(|job| matches!(job.state, JobState::Succeeded | JobState::Failed))
            {
                // Only terminal jobs may be retired. Queued/running jobs remain
                // queryable until they finish, even when the bounded ledger is full.
                self.jobs.remove(index);
            } else {
                return Err(StructuredError::job(
                    "job_capacity_full",
                    "job ledger is full of active jobs",
                ));
            }
        }
        for _ in 0..=JOB_CAPACITY {
            let job_id = self.next_job_id;
            self.next_job_id = self.next_job_id.wrapping_add(1).max(1);
            if !self.jobs.iter().any(|job| job.id == job_id) {
                return Ok(job_id);
            }
        }
        Err(StructuredError::job(
            "job_id_exhausted",
            "no free job identifier is available",
        ))
    }

    pub fn update_heap(&mut self, free: u32, minimum: u32) {
        self.system.free_heap = free;
        self.system.minimum_free_heap = minimum;
    }

    pub fn set_display_busy(&mut self, busy: bool) {
        self.system.display_busy = busy;
    }

    pub fn set_main_screen(&mut self, screen: &'static str) {
        self.system.main_screen = screen;
    }

    pub fn set_storage_ready(&mut self, ready: bool) {
        self.storage_ready = ready;
    }

    pub fn set_network_connected(&mut self, ssid: String, ip: String) {
        self.network.provisioning = false;
        self.network.mode = "station";
        self.network.sta_ssid = Some(ssid);
        self.network.sta_ip = Some(ip);
        self.push_event("wifi.connected", None, None);
    }

    pub fn set_network_provisioning(&mut self, detail: Option<String>) {
        self.network.provisioning = true;
        self.network.mode = "softap";
        self.network.sta_ip = None;
        self.push_event("wifi.provisioning", None, detail);
    }

    pub fn time_configuration(&self) -> TimeConfiguration {
        self.time.configuration.clone()
    }

    pub fn set_time_configuration(&mut self, configuration: TimeConfiguration) {
        self.time.configuration = configuration;
    }

    pub fn set_time_syncing(&mut self) {
        self.time.sync_state = "syncing";
        self.time.last_error = None;
    }

    pub fn set_time_waiting_for_network(&mut self) {
        self.time.sync_state = "waiting_network";
        self.time.last_error = None;
    }

    pub fn set_time_restored_from_rtc(&mut self) {
        self.time.clock_valid = true;
        self.time.source = "rtc";
        self.time.sync_state = "waiting_network";
        self.time.last_error = None;
    }

    pub fn set_time_synced(&mut self, unix_ms: i64) {
        self.time.clock_valid = true;
        self.time.source = "ntp";
        self.time.sync_state = "synced";
        self.time.last_ntp_sync_unix_ms = Some(unix_ms);
        self.time.rtc_updated = false;
        self.time.last_error = None;
        self.push_event("time.synced", None, None);
    }

    pub fn set_time_error(&mut self, code: &'static str) {
        self.time.sync_state = "error";
        self.time.last_error = Some(code);
        self.push_event("time.sync_failed", None, None);
    }

    pub fn set_rtc_updated(&mut self, updated: bool) {
        self.time.rtc_updated = updated;
    }

    pub fn time_is_valid(&self) -> bool {
        self.time.clock_valid
    }

    pub fn current_unix_ms(&self) -> Option<i64> {
        self.time.clock_valid.then(system_unix_ms).flatten()
    }

    fn configure_wifi(&mut self, id: Value, params: Value) -> RpcReply {
        #[derive(Deserialize)]
        struct Params {
            ssid: String,
            password: String,
        }
        let params: Params = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(_) => return error(id, 400, -32602, "invalid params"),
        };
        let credentials = match WifiCredentials::new(params.ssid, params.password) {
            Ok(credentials) => credentials,
            Err(message) => return error(id, 400, -32602, message),
        };
        self.enqueue(id, Command::WifiConfigure(credentials))
    }

    fn configure_time(&mut self, id: Value, params: Value) -> RpcReply {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            ntp_server: String,
            utc_offset_minutes: i16,
        }
        let params: Params = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(_) => return error(id, 400, -32602, "invalid params"),
        };
        let configuration =
            match TimeConfiguration::new(params.ntp_server, params.utc_offset_minutes) {
                Ok(configuration) => configuration,
                Err(message) => return error(id, 400, -32602, message),
            };
        self.enqueue(id, Command::TimeConfigure(configuration))
    }

    fn display_text(&mut self, id: Value, params: Value) -> RpcReply {
        #[derive(Deserialize)]
        struct Params {
            text: String,
        }
        let params: Params = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(_) => return error(id, 400, -32602, "invalid params"),
        };
        let text = match normalize_display_text(&params.text) {
            Ok(text) => text,
            Err(message) => return error(id, 400, -32602, message),
        };
        self.enqueue(id, Command::DisplayText { text })
    }

    fn test_run(&mut self, id: Value, params: Value) -> RpcReply {
        #[derive(Deserialize)]
        struct Params {
            test: String,
        }
        let params: Params = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(_) => return error(id, 400, -32602, "invalid params"),
        };
        if !TEST_IDS.contains(&params.test.as_str()) {
            return error(id, 400, -32602, "unknown test");
        }
        self.enqueue(id, Command::TestRun { test: params.test })
    }

    fn widget_get(&self, id: Value, params: Value) -> RpcReply {
        let Some(widget_id) = params.get("id").and_then(Value::as_str) else {
            return error(id, 400, -32602, "invalid params");
        };
        let Some(widget) = self
            .widgets
            .iter()
            .find(|widget| widget.manifest.id == widget_id)
        else {
            return structured_error_reply(
                id,
                404,
                -32004,
                StructuredError::widget("not_found", "widget not found"),
            );
        };
        ok(id, widget_value(widget, true, self.widget_busy(widget_id)))
    }

    fn widget_activate(&mut self, id: Value, params: Value) -> RpcReply {
        let Some(widget_id) = params.get("id").and_then(Value::as_str) else {
            return error(id, 400, -32602, "invalid params");
        };
        if !widget_id.is_empty()
            && !self
                .widgets
                .iter()
                .any(|widget| widget.manifest.id == widget_id)
        {
            return structured_error_reply(
                id,
                404,
                -32004,
                StructuredError::widget("not_found", "widget not found"),
            );
        }
        if !self.busy_widgets.is_empty() {
            return structured_error_reply(
                id,
                409,
                -32006,
                StructuredError::job("widget_busy", "widget already pending or running"),
            );
        }
        let tracked_id = widget_id.to_owned();
        let reply = self.enqueue(
            id,
            Command::WidgetActivate {
                id: widget_id.into(),
            },
        );
        if reply.http_status == 202 {
            self.busy_widgets.push(tracked_id);
        }
        reply
    }

    pub fn widget_settings(&self, widget_id: &str) -> Option<&WidgetSettings> {
        self.widgets
            .iter()
            .find(|widget| widget.manifest.id == widget_id)
            .map(|widget| &widget.settings)
    }

    pub fn commit_widget_settings(
        &mut self,
        widget_id: &str,
        settings: WidgetSettings,
    ) -> Result<(), &'static str> {
        self.validate_widget_settings(widget_id, &settings)?;
        let widget = self
            .widgets
            .iter_mut()
            .find(|widget| widget.manifest.id == widget_id)
            .ok_or("widget not found")?;
        widget.settings = settings;
        widget.poll = PollSchedule::default();
        self.push_event("widget.configured", None, Some(widget_id.into()));
        if self.system.active_widget == widget_id {
            self.foreground_pending = true;
        }
        Ok(())
    }

    fn validate_widget_settings(
        &self,
        widget_id: &str,
        settings: &WidgetSettings,
    ) -> Result<(), &'static str> {
        settings.validate()?;
        let old = self.widget_settings(widget_id).ok_or("widget not found")?;
        if self.widget_ram_bytes() - old.encoded_len() + settings.encoded_len() > WIDGET_RAM_MAX {
            return Err("widget configuration RAM limit exceeded");
        }
        Ok(())
    }

    fn widget_configuration(&self, id: Value, params: Value) -> RpcReply {
        let Some(widget_id) = params.get("id").and_then(Value::as_str) else {
            return error(id, 400, -32602, "invalid params");
        };
        match self.widget_settings(widget_id) {
            Some(settings) => ok(id, json!({"id":widget_id,"settings":settings})),
            None => error(id, 404, -32004, "widget not found"),
        }
    }

    fn widget_configure(&mut self, id: Value, params: Value) -> RpcReply {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            id: String,
            settings: WidgetSettings,
        }
        let Ok(params) = serde_json::from_value::<Params>(params) else {
            return error(id, 400, -32602, "invalid widget configuration");
        };
        if !self.storage_ready {
            return error(id, 409, -32006, "storage not initialized");
        }
        if !self.busy_widgets.is_empty() {
            return error(id, 409, -32006, "widget worker busy");
        }
        if let Err(message) = self.validate_widget_settings(&params.id, &params.settings) {
            return error(id, 400, -32602, message);
        }
        let widget_id = params.id.clone();
        let reply = self.enqueue(
            id,
            Command::WidgetConfigure {
                id: params.id,
                settings: params.settings,
            },
        );
        if reply.http_status == 202 {
            self.busy_widgets.push(widget_id);
        }
        reply
    }

    /// Called by the single widget worker, after the main loop has allowed
    /// scheduling. At most one run is pending; missed ticks are coalesced.
    pub fn poll_active_widget(&mut self, now_ms: u64) {
        if !self.storage_ready || self.system.mode != "normal" || !self.busy_widgets.is_empty() {
            return;
        }
        let Some(widget) = self
            .widgets
            .iter()
            .find(|widget| widget.manifest.id == self.system.active_widget)
        else {
            return;
        };
        if !self.foreground_pending && !widget.poll.due(now_ms, widget.settings.interval_seconds) {
            return;
        }
        let widget_id = widget.manifest.id.clone();
        if self
            .enqueue_host(if self.foreground_pending {
                Command::WidgetMount {
                    id: widget_id.clone(),
                }
            } else {
                Command::WidgetPoll {
                    id: widget_id.clone(),
                }
            })
            .is_ok()
        {
            self.foreground_pending = false;
            self.busy_widgets.push(widget_id);
        }
    }

    pub fn is_widget_poll_enabled(&self, widget_id: &str) -> bool {
        self.system.active_widget == widget_id
            && self
                .widget_settings(widget_id)
                .is_some_and(|settings| settings.interval_seconds != 0)
    }

    pub fn finish_widget_run(&mut self, widget_id: &str, now_ms: u64, succeeded: bool) {
        if let Some(widget) = self
            .widgets
            .iter_mut()
            .find(|widget| widget.manifest.id == widget_id)
        {
            widget
                .poll
                .finish(now_ms, widget.settings.interval_seconds, succeeded);
        }
    }

    fn widget_list(&self, id: Value) -> RpcReply {
        let widgets: Vec<Value> = self
            .widgets
            .iter()
            .map(|widget| widget_value(widget, false, self.widget_busy(&widget.manifest.id)))
            .collect();
        ok(
            id,
            json!({
                "widgets": widgets,
                "storage": if self.storage_ready { "assets_spiffs" } else { "uninitialized" },
                "persistent": self.storage_ready,
                "ram_bytes": self.widget_ram_bytes(),
                "ram_limit": WIDGET_RAM_MAX,
            }),
        )
    }

    /// Validate and enqueue a package decoded by the binary `.pnw` transport.
    /// Authentication and body framing remain the transport's responsibility.
    pub fn enqueue_widget_candidate_job(
        &mut self,
        upload: WidgetCandidate,
    ) -> Result<u32, WidgetEnqueueFailure> {
        let upload = match prepare_widget_candidate(upload) {
            Ok(upload) => upload,
            Err(error) => {
                return Err(WidgetEnqueueFailure {
                    http_status: 400,
                    rpc_code: -32602,
                    error,
                })
            }
        };
        if upload.manifest.id == DEMO_WIDGET_ID {
            return Err(WidgetEnqueueFailure {
                http_status: 403,
                rpc_code: -32003,
                error: StructuredError::widget(
                    "built_in_immutable",
                    "built-in widget is immutable",
                ),
            });
        }
        // Keep at most one source-bearing candidate alive. Together with the
        // peak check below this bounds the atomic old+new replacement window.
        if !self.busy_widgets.is_empty() {
            return Err(WidgetEnqueueFailure {
                http_status: 409,
                rpc_code: -32006,
                error: StructuredError::job("widget_worker_busy", "widget worker is busy"),
            });
        }
        let existing = self
            .widgets
            .iter()
            .position(|widget| widget.manifest.id == upload.manifest.id);
        if existing.is_none() && self.widgets.len() >= WIDGET_CAPACITY {
            return Err(WidgetEnqueueFailure {
                http_status: 507,
                rpc_code: -32005,
                error: StructuredError::widget("capacity_reached", "widget capacity reached"),
            });
        }
        if self.widget_ram_bytes() + upload.ram_bytes() > WIDGET_RAM_MAX {
            return Err(WidgetEnqueueFailure {
                http_status: 507,
                rpc_code: -32005,
                error: StructuredError::widget(
                    "peak_ram_exceeded",
                    "widget peak RAM limit exceeded",
                ),
            });
        }
        let tracked_id = upload.manifest.id.clone();
        let job_id = self
            .enqueue_host(Command::WidgetUpload(upload))
            .map_err(|error| {
                let (http_status, rpc_code) = match error.code {
                    "command_queue_full" | "job_capacity_full" | "job_id_exhausted" => {
                        (503, -32002)
                    }
                    _ => (500, -32000),
                };
                WidgetEnqueueFailure {
                    http_status,
                    rpc_code,
                    error,
                }
            })?;
        self.busy_widgets.push(tracked_id);
        Ok(job_id)
    }

    pub fn enqueue_widget_candidate(&mut self, id: Value, upload: WidgetCandidate) -> RpcReply {
        match self.enqueue_widget_candidate_job(upload) {
            Ok(job_id) => accepted(id, json!({"job_id": job_id, "state": "queued"})),
            Err(failure) => {
                structured_error_reply(id, failure.http_status, failure.rpc_code, failure.error)
            }
        }
    }

    /// Commit a previously validated and compiled candidate with a single
    /// replacement. The worker calls this only after Rune compilation passes.
    pub fn commit_widget(&mut self, candidate: WidgetCandidate) -> Result<String, WidgetError> {
        let candidate = prepare_widget_candidate(candidate)?;
        let settings = self
            .widget_settings(&candidate.manifest.id)
            .cloned()
            .unwrap_or_default();
        let replacement = WidgetPackage {
            manifest: candidate.manifest,
            source: candidate.source,
            resources: candidate.resources,
            built_in: false,
            settings,
            poll: PollSchedule::default(),
            run_job: None,
        };
        let existing = self
            .widgets
            .iter()
            .position(|widget| widget.manifest.id == replacement.manifest.id);
        if existing.is_none() && self.widgets.len() >= WIDGET_CAPACITY {
            return Err(StructuredError::widget(
                "capacity_reached",
                "widget capacity reached",
            ));
        }
        let replaced_bytes = existing
            .map(|index| self.widgets[index].ram_bytes())
            .unwrap_or(0);
        if self.widget_ram_bytes() - replaced_bytes + replacement.ram_bytes() > WIDGET_RAM_MAX {
            return Err(StructuredError::widget(
                "ram_limit_exceeded",
                "widget RAM limit exceeded",
            ));
        }
        let widget_id = replacement.manifest.id.clone();
        if let Some(index) = existing {
            self.widgets[index] = replacement;
        } else {
            self.widgets.push(replacement);
        }
        self.push_event("widget.uploaded", None, Some(widget_id.clone()));
        Ok(widget_id)
    }

    fn widget_delete(&mut self, id: Value, params: Value) -> RpcReply {
        let Some(widget_id) = params.get("id").and_then(Value::as_str) else {
            return error(id, 400, -32602, "invalid params");
        };
        let Some(index) = self
            .widgets
            .iter()
            .position(|widget| widget.manifest.id == widget_id)
        else {
            return structured_error_reply(
                id,
                404,
                -32004,
                StructuredError::widget("not_found", "widget not found"),
            );
        };
        if self.widgets[index].built_in || self.system.active_widget == widget_id {
            return structured_error_reply(
                id,
                403,
                -32003,
                StructuredError::widget(
                    "delete_forbidden",
                    "built-in or active widget cannot be deleted",
                ),
            );
        }
        if self.widget_busy(widget_id) {
            return structured_error_reply(
                id,
                409,
                -32006,
                StructuredError::job("widget_busy", "widget already pending or running"),
            );
        }
        let tracked_id = self.widgets[index].manifest.id.clone();
        let reply = self.enqueue(
            id,
            Command::WidgetDelete {
                id: tracked_id.clone(),
            },
        );
        if reply.http_status == 202 {
            self.busy_widgets.push(tracked_id);
        }
        reply
    }

    /// Remove a package from the in-memory registry after the storage worker
    /// has durably published its deletion tombstone and removed private data.
    pub fn commit_widget_delete(&mut self, widget_id: &str) -> Result<(), WidgetError> {
        let Some(index) = self
            .widgets
            .iter()
            .position(|widget| widget.manifest.id == widget_id)
        else {
            return Err(StructuredError::widget("not_found", "widget not found"));
        };
        if self.widgets[index].built_in || self.system.active_widget == widget_id {
            return Err(StructuredError::widget(
                "delete_forbidden",
                "built-in or active widget cannot be deleted",
            ));
        }
        self.widgets.remove(index);
        self.push_event("widget.deleted", None, Some(widget_id.to_owned()));
        Ok(())
    }

    /// Return one widget id whose isolated KV namespace must be deleted by
    /// the storage owner. The broker owns the signal, while the app owns the
    /// actual KV map and performs the destructive cleanup.
    pub fn take_widget_kv_cleanup(&mut self) -> Option<String> {
        self.kv_cleanup.pop_front()
    }

    pub fn pending_widget_kv_cleanup(&self) -> usize {
        self.kv_cleanup.len()
    }

    pub fn widget_runtime_spec(&self, widget_id: &str) -> Option<WidgetRuntimeSpec> {
        self.widgets
            .iter()
            .find(|widget| widget.manifest.id == widget_id)
            .map(|widget| WidgetRuntimeSpec {
                settings: widget.settings.clone(),
                id: widget.manifest.id.clone(),
                source: widget.source.clone(),
                capabilities: widget.manifest.capabilities.clone(),
                host_permissions: widget
                    .settings
                    .host_permissions
                    .clone()
                    .unwrap_or_else(|| widget.manifest.host_permissions.clone()),
                resources: widget.resources.clone(),
            })
    }

    pub fn activate_widget(&mut self, widget_id: &str) -> bool {
        if !widget_id.is_empty()
            && !self
                .widgets
                .iter()
                .any(|widget| widget.manifest.id == widget_id)
        {
            return false;
        }
        if let Some(widget) = self
            .widgets
            .iter_mut()
            .find(|widget| widget.manifest.id == widget_id)
        {
            widget.poll = PollSchedule::default();
        }
        self.system.active_widget = widget_id.into();
        self.foreground_pending = !widget_id.is_empty();
        self.push_event("widget.activated", None, Some(widget_id.into()));
        true
    }

    pub fn request_foreground_mount(&mut self) {
        self.foreground_pending = !self.system.active_widget.is_empty();
    }

    pub fn set_saved_wifi(&mut self, names: Vec<String>) {
        self.saved_wifi = names;
    }

    pub fn active_widget_can_sync(&self) -> bool {
        self.widgets
            .iter()
            .any(|widget| widget.manifest.id == self.system.active_widget && !widget.built_in)
    }

    /// A physical short press may join an existing run, but never queues a
    /// second invocation or interrupts a refresh already in progress.
    pub fn finish_widget_operation(&mut self, widget_id: &str) {
        if let Some(widget) = self
            .widgets
            .iter_mut()
            .find(|widget| widget.manifest.id == widget_id)
        {
            widget.run_job = None;
        }
        if let Some(index) = self.busy_widgets.iter().position(|id| id == widget_id) {
            self.busy_widgets.remove(index);
        }
    }

    fn widget_busy(&self, widget_id: &str) -> bool {
        self.busy_widgets.iter().any(|id| id == widget_id)
    }

    fn widget_status(&self, id: Value, params: Value) -> RpcReply {
        if let Some(widget_id) = params.get("id").and_then(Value::as_str) {
            if !self
                .widgets
                .iter()
                .any(|widget| widget.manifest.id == widget_id)
            {
                return structured_error_reply(
                    id,
                    404,
                    -32004,
                    StructuredError::widget("not_found", "widget not found"),
                );
            }
            return ok(
                id,
                json!({
                    "id": widget_id,
                    "active": self.system.active_widget == widget_id,
                    "state": if self.widget_busy(widget_id) { "busy" } else { "ready" },
                }),
            );
        }
        let widgets: Vec<Value> = self
            .widgets
            .iter()
            .map(|widget| {
                json!({
                    "id": widget.manifest.id,
                    "active": self.system.active_widget == widget.manifest.id,
                    "state": if self.widget_busy(&widget.manifest.id) { "busy" } else { "ready" },
                })
            })
            .collect();
        ok(id, json!({"widgets": widgets}))
    }

    fn widget_ram_bytes(&self) -> usize {
        self.widgets.iter().map(WidgetPackage::ram_bytes).sum()
    }

    fn event_list(&self, id: Value, params: Value) -> RpcReply {
        let after = params.get("after").and_then(Value::as_u64).unwrap_or(0);
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(16)
            .min(EVENT_PAGE_MAX as u64) as usize;
        let events: Vec<Value> = self
            .events
            .iter()
            .filter(|event| event.seq > after)
            .take(limit)
            .map(|event| {
                json!({
                    "seq": event.seq,
                    "kind": event.kind,
                    "job_id": event.job_id,
                    "detail": event.detail,
                })
            })
            .collect();
        ok(id, json!({"events": events}))
    }

    fn job_get(&self, id: Value, params: Value) -> RpcReply {
        let Some(job_id) = params.get("job_id").and_then(Value::as_u64) else {
            return error(id, 400, -32602, "invalid params");
        };
        let Some(job) = self.jobs.iter().find(|job| u64::from(job.id) == job_id) else {
            return structured_error_reply(
                id,
                404,
                -32004,
                StructuredError::job("not_found", "job not found"),
            );
        };
        ok(
            id,
            json!({
                "job_id": job.id,
                "method": job.method,
                "state": job.state.as_str(),
                "result": job.result,
            }),
        )
    }

    fn enqueue(&mut self, id: Value, command: Command) -> RpcReply {
        match self.enqueue_host(command) {
            Ok(job_id) => accepted(id, json!({"job_id": job_id, "state": "queued"})),
            Err(error) => {
                let (status, code) = match error.code {
                    "command_queue_full" | "job_capacity_full" | "job_id_exhausted" => {
                        (503, -32002)
                    }
                    _ => (500, -32000),
                };
                structured_error_reply(id, status, code, error)
            }
        }
    }

    fn set_job_state(&mut self, job_id: u32, state: JobState, result: Option<Value>) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.id == job_id) {
            job.state = state;
            job.result = result;
        }
    }

    fn push_event(&mut self, kind: &'static str, job_id: Option<u32>, detail: Option<String>) {
        if self.events.len() >= EVENT_CAPACITY {
            self.events.pop_front();
        }
        self.events.push_back(Event {
            seq: self.next_event_seq,
            kind,
            job_id,
            detail,
        });
        self.next_event_seq = self.next_event_seq.wrapping_add(1).max(1);
    }

    fn network_value(&self) -> Value {
        json!({
            "provisioning": self.network.provisioning,
            "mode": self.network.mode,
            "ap_ssid": self.network.ap_ssid,
            "ap_ip": self.network.ap_ip,
            "sta_ssid": self.network.sta_ssid,
            "sta_ip": self.network.sta_ip,
        })
    }

    /// One coherent, authenticated overview without installation secrets.
    fn dashboard_value(&self) -> Value {
        let widgets: Vec<Value> = self
            .widgets
            .iter()
            .map(|widget| {
                let mut value = widget_value(widget, false, self.widget_busy(&widget.manifest.id));
                value["active"] = json!(widget.manifest.id == self.system.active_widget);
                value["running"] = json!(widget
                    .run_job
                    .is_some_and(|id| self.job_state(id) == Some(JobState::Running)));
                value
            })
            .collect();
        json!({"system":self.system_value(),"widgets":widgets,"summary":{"widget_count":self.widgets.len(),"running_count":widgets.iter().filter(|value|value["running"]==true).count(),"default_widget":self.system.active_widget}})
    }

    fn system_value(&self) -> Value {
        json!({
            "firmware": self.system.firmware,
            "free_heap_bytes": self.system.free_heap,
            "minimum_free_heap_bytes": self.system.minimum_free_heap,
            "display_busy": self.system.display_busy,
            "main_screen": self.system.main_screen,
            "storage": {
                "ready": self.storage_ready,
                "kind": if self.storage_ready { "assets_spiffs" } else { "uninitialized" },
                "persistent": self.storage_ready,
            },
            "active_widget": self.system.active_widget,
            "mode": self.system.mode,
            "safe_mode": self.system.safe_mode,
            "input": self.system.input,
            "presentation": self.system.presentation,
            "network": self.network_value(),
            "time": self.time_value(),
        })
    }

    fn time_value(&self) -> Value {
        json!({
            "configuration": {
                "ntp_server": self.time.configuration.ntp_server,
                "utc_offset_minutes": self.time.configuration.utc_offset_minutes,
            },
            "sync_state": self.time.sync_state,
            "source": self.time.source,
            "clock_valid": self.time.clock_valid,
            "current_unix_ms": self.current_unix_ms(),
            "last_ntp_sync_unix_ms": self.time.last_ntp_sync_unix_ms,
            "rtc_updated": self.time.rtc_updated,
            "last_error": self.time.last_error,
        })
    }
}

fn system_unix_ms() -> Option<i64> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    i64::try_from(duration.as_millis()).ok()
}

fn widget_value(widget: &WidgetPackage, include_source: bool, busy: bool) -> Value {
    let mut value = json!({
        "manifest": {
            "id": widget.manifest.id,
            "name": widget.manifest.name,
            "version": widget.manifest.version,
            "abi": widget.manifest.abi,
            "capabilities": widget.manifest.capabilities,
            "host_permissions": widget.manifest.host_permissions,
        },
        "built_in": widget.built_in,
        "mutable": !widget.built_in,
        "resource_count": widget.resources.len(),
        "interval_seconds": widget.settings.interval_seconds,
        "ram_bytes": widget.ram_bytes(),
        "state": if busy { "busy" } else { "ready" },
    });
    if include_source {
        value["source"] = Value::String(widget.source.clone());
        value["resources"] = json!(widget.resources);
    }
    value
}

/// Normalize and validate an installation candidate without changing broker state.
/// Shared by device admission and the desktop editor.
pub fn prepare_widget_candidate(mut upload: WidgetCandidate) -> Result<WidgetCandidate, WidgetError> {
    let compiled = compile_host_permissions(&upload.manifest.host_permissions)
        .map_err(|error| StructuredError::widget(error.code, error.message))?;
    upload.manifest.host_permissions = compiled
        .iter()
        .map(|pattern| pattern.canonical().to_owned())
        .collect();

    let mut paths = Vec::with_capacity(upload.resources.len());
    for resource in &mut upload.resources {
        let original = resource.path.clone();
        let normalized = normalize_resource_path(&original).map_err(|code| {
            StructuredError::widget(code, "widget resource path is invalid")
                .with_details(json!({"path": original}))
        })?;
        if paths.iter().any(|path: &String| path == &normalized) {
            return Err(StructuredError::widget(
                "resource_duplicate",
                "duplicate widget resource path",
            )
            .with_details(json!({"path": normalized})));
        }
        paths.push(normalized.clone());
        resource.path = normalized;
    }
    validate_widget(&upload)?;
    Ok(upload)
}

pub fn normalize_widget_resource_path(path: &str) -> Result<String, WidgetError> {
    normalize_resource_path(path)
        .map_err(|code| StructuredError::widget(code, "widget resource path is invalid"))
}

fn normalize_resource_path(path: &str) -> Result<String, &'static str> {
    if path.is_empty() || path.len() > 64 || path.starts_with('/') || path.contains('\\') {
        return Err("resource_path_syntax");
    }
    if path.bytes().any(|byte| byte.is_ascii_control()) || path.contains(':') {
        return Err("resource_path_characters");
    }
    let mut normalized = String::new();
    for part in path.split('/') {
        if part.is_empty() {
            return Err("resource_path_empty_segment");
        }
        if part == "." {
            continue;
        }
        if part == ".." {
            return Err("resource_path_parent_segment");
        }
        normalized.push_str(if normalized.is_empty() { "" } else { "/" });
        normalized.push_str(part);
    }
    if normalized.is_empty() || normalized.len() > 64 {
        return Err("resource_path_syntax");
    }
    Ok(normalized)
}

fn validate_widget(upload: &WidgetCandidate) -> Result<(), WidgetError> {
    let manifest = &upload.manifest;
    if manifest.id.len() < 3
        || manifest.id.len() > 48
        || !manifest
            .id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '-')
    {
        return Err(StructuredError::widget("id_invalid", "invalid widget id"));
    }
    if manifest.name.is_empty() || manifest.name.len() > 32 || manifest.version.len() > 24 {
        return Err(StructuredError::widget(
            "metadata_invalid",
            "invalid widget name or version",
        ));
    }
    if manifest.abi != WIDGET_ABI {
        return Err(StructuredError::widget(
            "abi_unsupported",
            "unsupported widget ABI",
        ));
    }
    if upload.source.is_empty() || upload.source.len() > WIDGET_SOURCE_MAX {
        return Err(StructuredError::widget(
            "source_size",
            "widget source is empty or too large",
        ));
    }
    if manifest.capabilities.len() > WIDGET_CAPABILITIES.len()
        || manifest
            .capabilities
            .iter()
            .any(|cap| !WIDGET_CAPABILITIES.contains(&cap.as_str()))
    {
        return Err(StructuredError::widget(
            "capability_unsupported",
            "unsupported capability",
        ));
    }
    if manifest
        .capabilities
        .iter()
        .enumerate()
        .any(|(index, cap)| manifest.capabilities[..index].contains(cap))
    {
        return Err(StructuredError::widget(
            "capability_duplicate",
            "duplicate capability",
        ));
    }
    if !manifest.host_permissions.is_empty()
        && !manifest
            .capabilities
            .iter()
            .any(|cap| cap == CAP_HTTP_FETCH)
    {
        return Err(StructuredError::widget(
            "host_permissions_capability_missing",
            "host_permissions requires http.fetch capability",
        ));
    }
    if manifest
        .capabilities
        .iter()
        .any(|cap| cap == CAP_HTTP_INSECURE_TLS)
        && !manifest
            .capabilities
            .iter()
            .any(|cap| cap == CAP_HTTP_FETCH)
    {
        return Err(StructuredError::widget(
            "insecure_tls_dependency",
            "http.insecure_tls requires http.fetch capability",
        ));
    }
    if manifest.host_permissions.len() > HOST_PERMISSION_CAPACITY {
        return Err(StructuredError::widget(
            "host_permissions_count",
            "too many host permissions",
        ));
    }
    if compile_host_permissions(&manifest.host_permissions).is_err() {
        return Err(StructuredError::widget(
            "host_pattern_invalid",
            "invalid host permission pattern",
        ));
    }
    let resource_bytes: usize = upload
        .resources
        .iter()
        .map(|resource| resource.path.len() + resource.text.len())
        .sum();
    if resource_bytes > WIDGET_RESOURCE_MAX
        || upload.resources.len() > 8
        || upload.resources.iter().any(|resource| {
            resource.text.len() > WIDGET_RESOURCE_ITEM_MAX
                || normalize_resource_path(&resource.path).as_ref() != Ok(&resource.path)
        })
    {
        return Err(StructuredError::widget(
            "resources_invalid",
            "invalid or oversized widget resources",
        ));
    }
    Ok(())
}

fn ok(id: Value, result: Value) -> RpcReply {
    reply(200, json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn accepted(id: Value, result: Value) -> RpcReply {
    reply(202, json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error(id: Value, http_status: u16, code: i32, message: &'static str) -> RpcReply {
    structured_error_reply(
        id,
        http_status,
        code,
        StructuredError::rpc("rpc_error", message),
    )
}

fn structured_error_reply(
    id: Value,
    http_status: u16,
    rpc_code: i32,
    error: StructuredError,
) -> RpcReply {
    let mut data = json!({
        "kind": error.kind.as_str(),
        "code": error.code,
    });
    if let Some(details) = error.details {
        data["details"] = details;
    }
    reply(
        http_status,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": rpc_code,
                "message": error.message,
                "data": data,
            }
        }),
    )
}

fn reply(http_status: u16, value: Value) -> RpcReply {
    RpcReply {
        http_status,
        body: serde_json::to_string(&value).expect("JSON values are serializable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broker(provisioning: bool) -> Broker {
        let mut network = NetworkStatus::provisioning("NOTE4C-1234".into());
        network.provisioning = provisioning;
        Broker::new(network, "test", "secret-token".into())
    }

    fn call(broker: &mut Broker, method: &str, params: Value) -> RpcReply {
        broker.dispatch(
            serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": method,
                "params": params,
            }))
            .unwrap()
            .as_bytes(),
            Some("secret-token"),
        )
    }

    fn submit_n4w_candidate(broker: &mut Broker, value: Value) -> RpcReply {
        let candidate: WidgetCandidate = serde_json::from_value(value).unwrap();
        broker.enqueue_widget_candidate(json!(7), candidate)
    }

    fn polling_candidate(id: &str) -> WidgetCandidate {
        WidgetCandidate {
            manifest: WidgetManifest {
                id: id.into(),
                name: "TODO".into(),
                version: "1".into(),
                abi: WIDGET_ABI.into(),
                capabilities: vec![CAP_HTTP_FETCH.into()],
                host_permissions: vec![],
            },
            source: "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }".into(),
            resources: vec![],
        }
    }

    #[test]
    fn physical_input_marks_widget_busy_until_dispatch_finishes() {
        let mut state=broker(false);state.set_storage_ready(true);
        state.commit_widget(polling_candidate("test.input")).unwrap();
        let job=state.enqueue_host(Command::WidgetInput {id:"test.input".into(),keycode:2,kind:0,held_ms:200}).unwrap();
        assert_eq!(call(&mut state,"widget.configure",json!({"id":"test.input","settings":{"interval_seconds":0,"values":{},"host_permissions":null}})).http_status,409);
        state.take_for(Worker::Widget).unwrap();state.finish_job(job,true,json!({}));state.finish_widget_operation("test.input");
        assert_eq!(call(&mut state,"widget.configure",json!({"id":"test.input","settings":{"interval_seconds":0,"values":{},"host_permissions":null}})).http_status,202);
    }

    #[test]
    fn configuration_is_committed_separately_and_hidden_from_normal_responses() {
        let mut state = broker(false);
        state.set_storage_ready(true);
        state.commit_widget(polling_candidate("test.todo")).unwrap();
        let settings = WidgetSettings {
            interval_seconds: 300,
            host_permissions: Some(vec!["https://example.test/feed".into()]),
            values: json!({"url":"https://example.test/feed", "bearer_token":"TEST_PRIVATE_TOKEN"}),
        };
        assert_eq!(
            call_without_token(&mut state, "widget.configuration").http_status,
            401
        );
        let accepted = call(
            &mut state,
            "widget.configure",
            json!({"id":"test.todo","settings":settings}),
        );
        assert_eq!(accepted.http_status, 202);
        assert_eq!(
            state.widget_settings("test.todo").unwrap().interval_seconds,
            0
        );
        assert_eq!(
            call(&mut state, "widget.activate", json!({"id":"test.todo"})).http_status,
            409
        );
        let queued = state.take_for(Worker::Widget).unwrap();
        assert!(!format!("{:?}", queued.command).contains("TEST_PRIVATE_TOKEN"));
        let Command::WidgetConfigure { id, settings } = queued.command else {
            panic!("wrong worker command")
        };
        state.commit_widget_settings(&id, settings.clone()).unwrap();
        state.finish_job(queued.job_id, true, json!({"configured":true}));
        state.finish_widget_operation(&id);
        for method in [
            "widget.list",
            "widget.get",
            "widget.status",
            "event.list",
            "system.status",
        ] {
            assert!(!call(&mut state, method, json!({"id":"test.todo"}))
                .body
                .contains("TEST_PRIVATE_TOKEN"));
        }
        assert!(call(
            &mut state,
            "widget.configuration",
            json!({"id":"test.todo"})
        )
        .body
        .contains("TEST_PRIVATE_TOKEN"));
        assert_eq!(
            state
                .widget_runtime_spec("test.todo")
                .unwrap()
                .host_permissions,
            vec!["https://example.test/feed"]
        );
        state.commit_widget(polling_candidate("test.todo")).unwrap();
        assert_eq!(state.widget_settings("test.todo"), Some(&settings));
        let mut invalid = settings.clone();
        invalid.interval_seconds = 1;
        assert!(state.commit_widget_settings("test.todo", invalid).is_err());
        assert_eq!(state.widget_settings("test.todo"), Some(&settings));
    }

    #[test]
    fn dashboard_is_authenticated_coherent_and_excludes_configuration_secrets() {
        let mut state = broker(false);
        state.set_storage_ready(true);
        state.commit_widget(polling_candidate("test.todo")).unwrap();
        state
            .commit_widget_settings(
                "test.todo",
                WidgetSettings {
                    values: json!({"password":"DONT_LEAK"}),
                    ..Default::default()
                },
            )
            .unwrap();
        state.activate_widget("test.todo");
        let job = state
            .enqueue_host(Command::WidgetInput {
                id: "test.todo".into(),
                keycode: 2,
                kind: 0,
                held_ms: 120,
            })
            .unwrap();
        state.take_for(Worker::Widget).unwrap();
        assert_eq!(
            call_without_token(&mut state, "dashboard.get").http_status,
            401
        );
        let reply = call(&mut state, "dashboard.get", json!({}));
        assert!(!reply.body.contains("DONT_LEAK"));
        let data: Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(data["result"]["summary"]["widget_count"], 2);
        assert_eq!(data["result"]["summary"]["running_count"], 1);
        assert_eq!(data["result"]["summary"]["default_widget"], "test.todo");
        assert_eq!(data["result"]["system"]["storage"]["ready"], true);
        assert_eq!(state.job_state(job), Some(JobState::Running));
    }

    #[test]
    fn mode_changes_are_authenticated_explicit_worker_operations() {
        let mut state = broker(true);
        assert_eq!(state.system.mode, "normal");
        for method in ["test.list", "system.status", "dashboard.get", "widget.list"] {
            assert_eq!(call(&mut state, method, json!({})).http_status, 200);
            assert_eq!(state.system.mode, "normal");
            assert!(state.take_for(Worker::Widget).is_none());
            assert!(state.take_for(Worker::Main).is_none());
        }
        for method in ["system.mode.set", "screen.retry"] {
            assert_eq!(call_without_token(&mut state, method).http_status, 401);
        }
        assert_eq!(
            call(&mut state, "system.mode.set", json!({"mode":"other"})).http_status,
            400
        );
        assert_eq!(
            call(&mut state, "system.mode.set", json!({"mode":"diagnostic"})).http_status,
            202
        );
        assert_eq!(state.system.mode, "normal"); // Commit belongs to the worker.
        assert!(state.take_for(Worker::Main).is_none());
        let command = state.take_for(Worker::Widget).unwrap();
        assert_eq!(command.command, Command::SystemMode { diagnostic: true });
        state.finish_job(command.job_id, true, json!({}));
        state.system.mode = "diagnostic";
        assert_eq!(
            call(&mut state, "system.mode.set", json!({"mode":"normal"})).http_status,
            202
        );
        let command = state.take_for(Worker::Widget).unwrap();
        assert_eq!(command.command, Command::SystemMode { diagnostic: false });
        state.finish_job(command.job_id, true, json!({}));
        state.system.safe_mode = true;
        assert_eq!(
            call(&mut state, "system.mode.set", json!({"mode":"normal"})).http_status,
            400
        );
        assert!(state.take_for(Worker::Widget).is_none());
    }

    #[test]
    fn diagnostic_commands_cannot_escape_normal_mode_and_retry_is_separate() {
        let mut state = broker(true);
        assert_eq!(
            call(&mut state, "display.text", json!({"text":"TEST"})).http_status,
            404
        );
        assert_eq!(
            call(&mut state, "test.run", json!({"test":"SYS"})).http_status,
            404
        );
        assert!(state.take_for(Worker::Main).is_none());
        assert_eq!(call(&mut state, "screen.retry", json!({})).http_status, 202);
        assert_eq!(
            state.take_for(Worker::Widget).unwrap().command,
            Command::ScreenRetry
        );
        assert_eq!(state.system.mode, "normal");
        state.system.mode = "diagnostic";
        assert_eq!(
            call(&mut state, "display.text", json!({"text":"TEST"})).http_status,
            202
        );
        assert!(matches!(
            state.take_for(Worker::Main).unwrap().command,
            Command::DisplayText { .. }
        ));
    }

    #[test]
    fn presentation_metadata_is_separate_from_script_jobs_and_contains_no_settings() {
        let mut state = broker(true);
        state.commit_widget(polling_candidate("test.todo")).unwrap();
        state
            .commit_widget_settings(
                "test.todo",
                WidgetSettings {
                    values: json!({"password":"DO_NOT_EXPOSE"}),
                    ..Default::default()
                },
            )
            .unwrap();
        state.system.presentation = json!({"state":"failed","submission_id":5,"refreshing":false,"error":"display timeout"});
        state.system.input = json!({"captured":12,"overflow":2,"pending":3});
        for method in ["system.status", "dashboard.get"] {
            let reply = call(&mut state, method, json!({}));
            assert!(!reply.body.contains("DO_NOT_EXPOSE"));
            let value: Value = serde_json::from_str(&reply.body).unwrap();
            let status = if method == "dashboard.get" {
                &value["result"]["system"]
            } else {
                &value["result"]
            };
            assert_eq!(status["presentation"]["submission_id"], 5);
            assert_eq!(status["presentation"]["state"], "failed");
            assert_eq!(status["input"]["overflow"], 2);
            assert_eq!(status["mode"], "normal");
        }
    }

    #[test]
    fn scheduler_only_polls_active_widgets_without_overlap_or_catchup() {
        let mut state = broker(false);
        state.set_storage_ready(true);
        state.commit_widget(polling_candidate("test.todo")).unwrap();
        state
            .commit_widget_settings(
                "test.todo",
                WidgetSettings {
                    interval_seconds: 60,
                    ..Default::default()
                },
            )
            .unwrap();
        state.poll_active_widget(0);
        assert!(state.take_for(Worker::Widget).is_none());
        state.activate_widget("test.todo");
        state.set_display_busy(true);
        state.poll_active_widget(0); // Physical refresh does not block script scheduling.
        let first = state.take_for(Worker::Widget).unwrap();
        assert!(matches!(first.command, Command::WidgetMount { .. }));
        state.poll_active_widget(1_000_000);
        assert!(state.take_for(Worker::Widget).is_none());
        state.finish_widget_run("test.todo", 1_000_000, false);
        state.finish_job(first.job_id, false, json!({"failed":true}));
        state.finish_widget_operation("test.todo");
        state.poll_active_widget(1_119_999);
        assert!(state.take_for(Worker::Widget).is_none());
        state.poll_active_widget(1_120_000);
        let second = state.take_for(Worker::Widget).unwrap();
        state.activate_widget(DEMO_WIDGET_ID);
        assert!(!state.is_widget_poll_enabled("test.todo"));
        state.finish_widget_run("test.todo", 1_120_001, true);
        state.finish_job(second.job_id, true, json!({"skipped":true}));
        state.finish_widget_operation("test.todo");
        state.poll_active_widget(u64::MAX);
        let mounted = state.take_for(Worker::Widget).unwrap();
        assert!(matches!(mounted.command, Command::WidgetMount { .. }));
        state.finish_job(mounted.job_id, true, json!({}));
        state.finish_widget_operation(&state.system.active_widget.clone());
        state.poll_active_widget(u64::MAX);
        assert!(state.take_for(Worker::Widget).is_none());
        state
            .commit_widget_settings("test.todo", WidgetSettings::default())
            .unwrap();
        state.activate_widget("test.todo");
        state.poll_active_widget(u64::MAX);
        let mounted = state.take_for(Worker::Widget).unwrap();
        assert!(matches!(mounted.command, Command::WidgetMount { .. }));
        state.finish_job(mounted.job_id, true, json!({}));
        state.finish_widget_operation(&state.system.active_widget.clone());
        state.poll_active_widget(u64::MAX);
        assert!(state.take_for(Worker::Widget).is_none());
    }

    #[test]
    fn rejects_batch_oversize_and_bad_auth() {
        let mut state = broker(true);
        assert_eq!(state.dispatch(b"[]", Some("secret-token")).http_status, 400);
        assert_eq!(
            state
                .dispatch(&vec![b'x'; RPC_BODY_MAX + 1], None)
                .http_status,
            413
        );
        assert_eq!(
            call_without_token(&mut state, "system.status").http_status,
            401
        );
    }

    #[test]
    fn device_bootstrap_credentials_are_stable_versioned_and_domain_separated() {
        let identity = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        assert_eq!(derive_setup_ap_password(&identity), "YECV-PMMA-74GM");
        assert_eq!(derive_admin_bootstrap_token(&identity), "YUPFHU6Z");
        assert_ne!(
            derive_setup_ap_password(&identity).replace('-', "")[..8],
            derive_admin_bootstrap_token(&identity)
        );
        assert_ne!(
            derive_admin_bootstrap_token(&identity),
            derive_admin_bootstrap_token(&[0, 1, 2, 3, 4, 6])
        );
    }

    #[test]
    fn admin_token_validation_and_rotation_are_explicit() {
        for invalid in [
            "SHORT",
            "HAS SPACE",
            "含中文字符",
            "A/B/C/D",
            &"X".repeat(33),
        ] {
            assert!(validate_admin_token(invalid).is_err());
        }
        assert!(validate_admin_token("A2C4E6G7").is_ok());
        assert!(validate_admin_token("custom-token_2").is_ok());

        let mut state = broker(true);
        state
            .replace_admin_token("NEW-TOKEN_2".into())
            .expect("valid replacement token");
        assert_eq!(state.dispatch(b"{}", Some("secret-token")).http_status, 401);
        assert_ne!(state.dispatch(b"{}", Some("NEW-TOKEN_2")).http_status, 401);
    }

    #[test]
    fn serial_provision_protocol_is_bounded_and_never_formats_credentials() {
        assert_eq!(classify_wifi_disconnect_reason(201), "wifi_ap_not_found");
        assert_eq!(
            classify_wifi_disconnect_reason(2),
            "wifi_authentication_timeout"
        );
        assert_eq!(
            classify_wifi_disconnect_reason(202),
            "wifi_authentication_failed"
        );
        assert_eq!(
            classify_wifi_disconnect_reason(23),
            "wifi_security_incompatible"
        );
        assert_eq!(
            classify_wifi_disconnect_reason(210),
            "wifi_security_incompatible"
        );
        assert_eq!(
            classify_wifi_disconnect_reason(203),
            "wifi_association_failed"
        );
        assert_eq!(classify_wifi_disconnect_reason(212), "wifi_signal_too_weak");
        assert_eq!(classify_wifi_disconnect_reason(200), "wifi_beacon_timeout");
        assert_eq!(
            classify_wifi_disconnect_reason(65_535),
            "wifi_connection_failed"
        );

        let secret = "private-password";
        let line = format!(
            "{SERIAL_PROVISION_REQUEST_PREFIX}{{\"id\":\"0123456789abcdef\",\"ssid\":\"MY-NET\",\"password\":\"{secret}\"}}\r\n"
        );
        let request = parse_serial_provision_request(line.as_bytes()).expect("valid request");
        assert_eq!(request.request_id, "0123456789abcdef");
        assert_eq!(request.credentials.ssid(), "MY-NET");
        assert_eq!(request.credentials.password(), secret);

        let accepted = format_serial_provision_result(
            &request.request_id,
            SerialProvisionState::Accepted,
            Some(7),
            None,
        );
        assert!(accepted.starts_with(SERIAL_PROVISION_RESULT_PREFIX));
        assert!(accepted.contains("accepted"));
        assert!(!accepted.contains("MY-NET"));
        assert!(!accepted.contains(secret));
        let pending = format_serial_provision_result(
            &request.request_id,
            SerialProvisionState::Pending,
            Some(7),
            Some("device_timeout"),
        );
        assert!(pending.contains("\"state\":\"pending\""));
        assert!(pending.contains("\"code\":\"device_timeout\""));
        assert!(!pending.contains("MY-NET"));
        assert!(!pending.contains(secret));

        let invalid = format!(
            "{SERIAL_PROVISION_REQUEST_PREFIX}{{\"id\":\"0123456789abcdef\",\"ssid\":\"MY-NET\",\"password\":\"short\"}}"
        );
        let error = match parse_serial_provision_request(invalid.as_bytes()) {
            Ok(_) => panic!("short password must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.request_id.as_deref(), Some("0123456789abcdef"));
        assert_eq!(error.code, "invalid_credentials");
        let oversized = vec![b'x'; SERIAL_PROVISION_LINE_MAX + 1];
        let error = match parse_serial_provision_request(&oversized) {
            Ok(_) => panic!("oversized serial line must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.code, "line_too_long");

        let mut state = broker(true);
        let job_id = state.enqueue_host(Command::WifiScan).unwrap();
        let _ = state.take_for(Worker::Network).unwrap();
        state.finish_job_with_error(
            job_id,
            StructuredError::job("wifi_ap_not_found", "access point was not found"),
        );
        assert_eq!(state.job_failure_code(job_id), Some("wifi_ap_not_found"));
    }

    fn call_without_token(broker: &mut Broker, method: &str) -> RpcReply {
        let request = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"method":method})).unwrap();
        broker.dispatch(&request, None)
    }

    #[test]
    fn system_status_reports_server_controlled_main_screen() {
        let mut state = broker(true);
        let initial: Value =
            serde_json::from_str(&call(&mut state, "system.status", json!({})).body).unwrap();
        assert_eq!(initial["result"]["main_screen"], "provisioning");

        state.set_main_screen("provisioned");
        let connected: Value =
            serde_json::from_str(&call(&mut state, "system.status", json!({})).body).unwrap();
        assert_eq!(connected["result"]["main_screen"], "provisioned");

        state.set_main_screen("widget");
        let widget: Value =
            serde_json::from_str(&call(&mut state, "system.status", json!({})).body).unwrap();
        assert_eq!(widget["result"]["main_screen"], "widget");
    }

    #[test]
    fn queues_typed_commands_and_tracks_jobs() {
        let mut state = broker(true);
        state.system.mode = "diagnostic";
        let response = call(&mut state, "display.text", json!({"text":"hello 42"}));
        assert_eq!(response.http_status, 202);
        let queued = state.take_for(Worker::Main).unwrap();
        assert_eq!(
            queued.command,
            Command::DisplayText {
                text: "HELLO 42".into()
            }
        );
        state.finish_job(queued.job_id, true, json!({"refresh_ms": 24100}));
        let response = call(&mut state, "job.get", json!({"job_id": queued.job_id}));
        assert!(response.body.contains("succeeded"));
        assert!(response.body.contains("24100"));
    }

    #[test]
    fn credentials_are_never_returned_or_formatted_in_cleartext() {
        let mut state = broker(true);
        let response = call(
            &mut state,
            "wifi.configure",
            json!({"ssid":"MY-NET","password":"private-password"}),
        );
        assert_eq!(response.http_status, 202);
        assert!(!response.body.contains("private-password"));
        let queued = state.take_for(Worker::Network).unwrap();
        assert!(!format!("{:?}", queued.command).contains("private-password"));
        assert!(!format!(
            "{}",
            match &queued.command {
                Command::WifiConfigure(c) => c,
                _ => unreachable!(),
            }
        )
        .contains("private-password"));
    }

    #[test]
    fn widget_registry_supports_binary_transport_foreground_selection_and_delete() {
        let mut state = broker(true);
        assert!(call(&mut state, "widget.list", json!({}))
            .body
            .contains(DEMO_WIDGET_ID));
        let upload = json!({
            "manifest": {
                "id": "sample.weather",
                "name": "WEATHER",
                "version": "1.0.0",
                "abi": WIDGET_ABI,
                                "capabilities": ["screen.present", "http.fetch"],
                "host_permissions": ["https://api.example.com/*"]
            },
            "source": "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }",
            "resources": [{"path":"label.txt", "text":"TODAY"}]
        });
        assert_eq!(submit_n4w_candidate(&mut state, upload).http_status, 202);
        let second_upload = json!({
            "manifest": {
                "id": "sample.second",
                "name": "SECOND",
                "version": "1.0.0",
                "abi": WIDGET_ABI,
                                "capabilities": [],
                "host_permissions": []
            },
            "source": "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }",
            "resources": []
        });
        assert_eq!(
            submit_n4w_candidate(&mut state, second_upload).http_status,
            409
        );
        let upload = state.take_for(Worker::Widget).unwrap();
        let Command::WidgetUpload(candidate) = upload.command else {
            panic!("expected widget upload")
        };
        assert_eq!(state.commit_widget(candidate).unwrap(), "sample.weather");
        state.finish_job(upload.job_id, true, json!({"stored":"assets_spiffs"}));
        state.finish_widget_operation("sample.weather");
        assert_eq!(
            call(&mut state, "widget.run", json!({"id":"sample.weather"})).http_status,
            404
        );
        assert!(state.take_for(Worker::Widget).is_none());
        assert_eq!(
            call(
                &mut state,
                "widget.activate",
                json!({"id":"sample.weather"})
            )
            .http_status,
            202
        );
        let activation = state.take_for(Worker::Widget).unwrap();
        assert!(state.activate_widget("sample.weather"));
        state.finish_job(activation.job_id, true, json!({"active":"sample.weather"}));
        state.finish_widget_operation("sample.weather");
        assert_eq!(
            call(&mut state, "widget.delete", json!({"id":"sample.weather"})).http_status,
            403
        );
        assert_eq!(
            call(&mut state, "widget.activate", json!({"id":DEMO_WIDGET_ID})).http_status,
            202
        );
        assert_eq!(
            call(&mut state, "widget.install", json!({})).http_status,
            404
        );
        assert_eq!(
            call(&mut state, "widget.upload", json!({})).http_status,
            404
        );
    }

    #[test]
    fn widget_upload_rejects_raw_unit_and_broad_host_wildcards_atomically() {
        let mut state = broker(true);
        let before = call(&mut state, "widget.list", json!({})).body;
        let bad = json!({
            "manifest": {
                "id": "sample.bad",
                "name": "BAD",
                "version": "1",
                "abi": WIDGET_ABI,
                                "capabilities": ["http.fetch"],
                "host_permissions": ["https://*/*"]
            },
            "source": "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }",
            "resources": []
        });
        assert_eq!(submit_n4w_candidate(&mut state, bad).http_status, 400);
        let after = call(&mut state, "widget.list", json!({})).body;
        assert_eq!(before, after);
        assert_eq!(
            call(&mut state, "widget.uploadUnit", json!({"bytes":"AAAA"})).http_status,
            404
        );
    }

    #[test]
    fn validates_wifi_and_display_inputs() {
        let mut state = broker(true);
        state.system.mode = "diagnostic";
        assert_eq!(
            call(
                &mut state,
                "wifi.configure",
                json!({"ssid":"x","password":"short"})
            )
            .http_status,
            400
        );
        assert_eq!(
            call(&mut state, "display.text", json!({"text":"emoji 💥"})).http_status,
            400
        );
        assert_eq!(
            call(&mut state, "test.run", json!({"test":"raw_flash"})).http_status,
            400
        );
    }

    #[test]
    fn time_configuration_is_bounded_queued_and_reported() {
        assert_eq!(
            TimeConfiguration::new("NTP.EXAMPLE.COM".into(), 480)
                .unwrap()
                .ntp_server(),
            "ntp.example.com"
        );
        for invalid in [
            "",
            "https://ntp.example.com",
            "-bad.example",
            "bad..example",
        ] {
            assert!(TimeConfiguration::new(invalid.into(), 0).is_err());
        }
        assert!(TimeConfiguration::new("ntp.example.com".into(), 841).is_err());

        let mut state = broker(true);
        let response = call(
            &mut state,
            "time.configure",
            json!({"ntp_server":"ntp.example.com","utc_offset_minutes":480}),
        );
        assert_eq!(response.http_status, 202);
        let queued = state.take_for(Worker::Network).unwrap();
        assert_eq!(
            queued.command,
            Command::TimeConfigure(TimeConfiguration::new("ntp.example.com".into(), 480).unwrap())
        );

        state
            .set_time_configuration(TimeConfiguration::new("ntp.example.com".into(), 480).unwrap());
        state.set_time_synced(1_725_366_896_000);
        state.set_rtc_updated(true);
        let status: Value =
            serde_json::from_str(&call(&mut state, "time.status", json!({})).body).unwrap();
        assert_eq!(
            status["result"]["configuration"]["ntp_server"],
            "ntp.example.com"
        );
        assert_eq!(status["result"]["configuration"]["utc_offset_minutes"], 480);
        assert_eq!(status["result"]["sync_state"], "synced");
        assert_eq!(status["result"]["source"], "ntp");
        assert_eq!(status["result"]["rtc_updated"], true);
        assert_eq!(status["result"]["clock_valid"], true);
    }

    #[test]
    fn strict_host_patterns_are_canonical_and_boundary_safe() {
        let pattern = HostPattern::parse("https://*.Example.COM/api/*").unwrap();
        assert_eq!(pattern.canonical(), "https://*.example.com/api/*");
        assert_eq!(pattern.host(), "example.com");
        assert_eq!(pattern.scheme(), UrlScheme::Https);

        let child = ParsedUrl::parse("https://a.example.com/api/v1?x=1").unwrap();
        assert!(pattern.matches(&child));
        assert!(!pattern.matches(&ParsedUrl::parse("https://example.com/api/v1").unwrap()));
        assert!(!pattern.matches(&ParsedUrl::parse("https://a.example.com.evil/api/v1").unwrap()));
        assert!(!pattern.matches(&ParsedUrl::parse("http://a.example.com/api/v1").unwrap()));

        for invalid in [
            "https://*/*",
            "https://example.com",
            "https://example.com:0/*",
            "https://example.com:65536/*",
            "https://example.com/a/../*",
            "https://example.com/*/*",
            "https://user@example.com/*",
        ] {
            assert!(HostPattern::parse(invalid).is_err(), "accepted {invalid}");
        }

        let duplicate = vec![
            "https://EXAMPLE.com/*".to_owned(),
            "https://example.com:443/*".to_owned(),
        ];
        assert_eq!(
            compile_host_permissions(&duplicate).unwrap_err().code,
            "pattern_duplicate"
        );
    }

    #[test]
    fn url_policy_allows_public_and_lan_but_rejects_special_destinations() {
        let patterns = compile_host_permissions(&[
            "https://api.example.com/*".to_owned(),
            "http://api.example.com/*".to_owned(),
            "https://192.168.4.1/*".to_owned(),
            "https://127.0.0.1/*".to_owned(),
            "https://0.0.0.0/*".to_owned(),
            "https://224.0.0.1/*".to_owned(),
            "https://255.255.255.255/*".to_owned(),
        ])
        .unwrap();

        // Code-level tls_verify:false is allowed for every explicitly
        // permitted HTTPS destination, including public addresses.
        assert!(authorize_url(
            "https://api.example.com/v1",
            &patterns,
            true,
            &["1.1.1.1".parse().unwrap()],
            None,
        )
        .is_ok());
        assert!(authorize_url("https://192.168.4.1/status", &patterns, false, &[], None,).is_ok());
        assert_eq!(
            authorize_url(
                "http://api.example.com/v1",
                &patterns,
                true,
                &["1.1.1.1".parse().unwrap()],
                None,
            )
            .unwrap_err()
            .code,
            "insecure_tls_requires_https"
        );
        assert_eq!(
            authorize_url("https://api.example.com/v1", &patterns, false, &[], None,)
                .unwrap_err()
                .code,
            "dns_resolution_required"
        );
        assert_eq!(
            authorize_url(
                "https://api.example.com/v1",
                &patterns,
                false,
                &["127.0.0.1".parse().unwrap()],
                None,
            )
            .unwrap_err()
            .code,
            "loopback"
        );
        for (url, code) in [
            ("https://127.0.0.1/x", "loopback"),
            ("https://0.0.0.0/x", "unspecified"),
            ("https://224.0.0.1/x", "multicast"),
            ("https://255.255.255.255/x", "broadcast"),
        ] {
            assert_eq!(
                authorize_url(url, &patterns, false, &[], None)
                    .unwrap_err()
                    .code,
                code
            );
        }
        assert_eq!(
            authorize_url(
                "https://127.0.0.1/x",
                &patterns,
                false,
                &["1.1.1.1".parse().unwrap()],
                None,
            )
            .unwrap_err()
            .code,
            "loopback"
        );
        assert_eq!(
            authorize_url(
                "https://api.example.com/v1",
                &patterns,
                false,
                &["1.1.1.1".parse().unwrap()],
                Some("1.1.1.1".parse().unwrap()),
            )
            .unwrap_err()
            .code,
            "device_self"
        );
        assert_eq!(
            authorize_url(
                "https://other.example.com/v1",
                &patterns,
                false,
                &["1.1.1.1".parse().unwrap()],
                None,
            )
            .unwrap_err()
            .code,
            "host_permission_denied"
        );
        let ipv6 = compile_host_permissions(&["https://[::1]/*".to_owned()]).unwrap();
        assert_eq!(ipv6[0].canonical(), "https://[::1]/*");
        assert_eq!(
            authorize_url("https://[::1]/x", &ipv6, false, &[], None)
                .unwrap_err()
                .code,
            "loopback"
        );
        assert_eq!(
            classify_ip("::ffff:127.0.0.1".parse().unwrap(), None),
            AddressClass::Loopback
        );
    }

    #[test]
    fn http_capability_and_manifest_v3_rules_are_explicit() {
        assert_eq!(
            authorize_http_capabilities(&[CAP_SCREEN_PRESENT.to_owned()], false)
                .unwrap_err()
                .code,
            "http_capability_missing"
        );
        assert_eq!(
            authorize_http_capabilities(&[CAP_HTTP_FETCH.to_owned()], true)
                .unwrap_err()
                .code,
            "insecure_tls_capability_missing"
        );
        assert!(authorize_http_capabilities(
            &[CAP_HTTP_FETCH.to_owned(), CAP_HTTP_INSECURE_TLS.to_owned()],
            true
        )
        .is_ok());

        let old_manifest = json!({
            "id": "sample.old",
            "name": "OLD",
            "version": "1",
            "abi": "note4c.rune/1",
                        "capabilities": [],
            "allowed_hosts": []
        });
        assert!(serde_json::from_value::<WidgetManifest>(old_manifest).is_err());
        let mut legacy = polling_candidate("sample.legacy");
        legacy.manifest.abi = "note4c.rune/2".into();
        let mut state = broker(true);
        let response = state.enqueue_widget_candidate(json!(7), legacy);
        let body: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(body["error"]["data"]["code"], "abi_unsupported");
        let mut manifest =
            serde_json::to_value(polling_candidate("sample.entry").manifest).unwrap();
        manifest["entry"] = json!("main");
        assert!(serde_json::from_value::<WidgetManifest>(manifest).is_err());
    }

    #[test]
    fn resources_are_normalized_and_duplicate_aliases_rejected() {
        let mut state = broker(true);
        let manifest = WidgetManifest {
            id: "sample.limits".into(),
            name: "LIMITS".into(),
            version: "1".into(),
            abi: WIDGET_ABI.into(),
            capabilities: vec![],
            host_permissions: vec![],
        };
        let within_total_limit = WidgetCandidate {
            manifest: manifest.clone(),
            source: "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }".into(),
            resources: (0..4)
                .map(|index| WidgetResource {
                    path: format!("assets/{index}.txt"),
                    text: "x".repeat(900),
                })
                .collect(),
        };
        assert!(validate_widget(&within_total_limit).is_ok());
        let oversized_item = WidgetCandidate {
            manifest,
            source: "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }".into(),
            resources: vec![WidgetResource {
                path: "assets/large.txt".into(),
                text: "x".repeat(WIDGET_RESOURCE_ITEM_MAX + 1),
            }],
        };
        assert_eq!(
            validate_widget(&oversized_item).unwrap_err().code,
            "resources_invalid"
        );

        let duplicate = json!({
            "manifest": {
                "id": "sample.paths",
                "name": "PATHS",
                "version": "1",
                "abi": WIDGET_ABI,
                                "capabilities": [],
                "host_permissions": []
            },
            "source": "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }",
            "resources": [
                {"path":"assets/./label.txt", "text":"A"},
                {"path":"assets/label.txt", "text":"B"}
            ]
        });
        let response = submit_n4w_candidate(&mut state, duplicate);
        let body: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(response.http_status, 400);
        assert_eq!(body["error"]["data"]["kind"], "widget");
        assert_eq!(body["error"]["data"]["code"], "resource_duplicate");
        assert_eq!(state.take_for(Worker::Widget), None);

        let valid = json!({
            "manifest": {
                "id": "sample.paths",
                "name": "PATHS",
                "version": "1",
                "abi": WIDGET_ABI,
                                "capabilities": [],
                "host_permissions": []
            },
            "source": "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }",
            "resources": [{"path":"assets/./label..txt", "text":"A"}]
        });
        assert_eq!(submit_n4w_candidate(&mut state, valid).http_status, 202);
        let queued = state.take_for(Worker::Widget).unwrap();
        let Command::WidgetUpload(candidate) = queued.command else {
            panic!("expected widget upload")
        };
        assert_eq!(candidate.resources[0].path, "assets/label..txt");
        assert_eq!(
            state.commit_widget(candidate.clone()).unwrap(),
            "sample.paths"
        );
        let spec = state.widget_runtime_spec("sample.paths").unwrap();
        assert_eq!(spec.resources[0].path, "assets/label..txt");
        state.finish_widget_operation("sample.paths");
    }

    #[test]
    fn active_jobs_are_retained_when_ledger_is_full() {
        let mut state = broker(true);
        state.system.mode = "diagnostic";
        let mut job_ids = Vec::new();
        for _ in 0..JOB_CAPACITY {
            let job_id = state
                .enqueue_host(Command::DisplayStatus)
                .expect("active job should fit");
            let queued = state
                .take_for(Worker::Main)
                .expect("command should be taken");
            assert_eq!(queued.job_id, job_id);
            job_ids.push(job_id);
        }
        let full = state
            .enqueue_host(Command::DisplayStatus)
            .expect_err("active jobs must not be evicted");
        assert_eq!(full.kind, ErrorKind::Job);
        assert_eq!(full.code, "job_capacity_full");
        let rejected = call(&mut state, "display.status", json!({}));
        let rejected_body: Value = serde_json::from_str(&rejected.body).unwrap();
        assert_eq!(rejected.http_status, 503);
        assert_eq!(rejected_body["error"]["data"]["kind"], "job");
        assert_eq!(rejected_body["error"]["data"]["code"], "job_capacity_full");
        let first = call(&mut state, "job.get", json!({"job_id": job_ids[0]}));
        assert_eq!(first.http_status, 200);
        assert!(first.body.contains("running"));

        state.finish_job(job_ids[0], true, json!({"done":true}));
        let replacement = state
            .enqueue_host(Command::DisplayStatus)
            .expect("a terminal job can be retired");
        assert_ne!(replacement, job_ids[0]);
        assert_eq!(
            call(&mut state, "job.get", json!({"job_id": job_ids[1]})).http_status,
            200
        );
        assert_eq!(
            call(&mut state, "job.get", json!({"job_id": job_ids[0]})).http_status,
            404
        );
        state.finish_job_with_error(
            replacement,
            StructuredError::job("worker_timeout", "worker timed out")
                .with_details(json!({"timeout_ms": 5000})),
        );
        let failed = call(&mut state, "job.get", json!({"job_id": replacement}));
        assert!(failed.body.contains("worker_timeout"));
        assert!(failed.body.contains("timeout_ms"));
    }

    #[test]
    fn widget_delete_is_a_worker_job_before_registry_commit() {
        let mut state = broker(true);
        let candidate: WidgetCandidate = serde_json::from_value(json!({
            "manifest": {
                "id": "sample.delete",
                "name": "DELETE",
                "version": "1",
                "abi": WIDGET_ABI,
                                "capabilities": ["storage.kv"],
                "host_permissions": []
            },
            "source": "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }",
            "resources": []
        }))
        .unwrap();
        assert_eq!(state.commit_widget(candidate).unwrap(), "sample.delete");
        let response = call(&mut state, "widget.delete", json!({"id":"sample.delete"}));
        assert_eq!(response.http_status, 202);
        assert_eq!(
            call(&mut state, "widget.activate", json!({"id":"sample.delete"})).http_status,
            409
        );
        assert!(state.widget_runtime_spec("sample.delete").is_some());
        let queued = state.take_for(Worker::Widget).unwrap();
        assert_eq!(
            queued.command,
            Command::WidgetDelete {
                id: "sample.delete".into()
            }
        );
        state.commit_widget_delete("sample.delete").unwrap();
        state.finish_job(queued.job_id, true, json!({"persistent":true}));
        state.finish_widget_operation("sample.delete");
        assert!(state.widget_runtime_spec("sample.delete").is_none());
    }
}
