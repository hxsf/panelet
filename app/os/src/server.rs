// SPDX-License-Identifier: MPL-2.0

use std::collections::VecDeque;
use std::convert::TryInto;
use std::future::Future;
use std::io::Read as _;
use std::net::{IpAddr, ToSocketAddrs};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::server::{Connection as ServerConnection, Request as ServerRequest};
use embedded_svc::io::Write as _;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::http::client::{
    Configuration as HttpClientConfiguration, EspHttpConnection, FollowRedirectsPolicy,
    Method as HttpClientMethod,
};
use esp_idf_svc::http::server::{Configuration as HttpConfiguration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::ipv4::{
    Configuration as IpConfiguration, Ipv4Addr, Mask, RouterConfiguration, Subnet,
};
use esp_idf_svc::netif::{EspNetif, NetifConfiguration, NetifStack};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::sntp::{EspSntp, OperatingMode, SntpConf, SyncMode};
use esp_idf_svc::sys::EspError;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, ClientConfiguration, Configuration as WifiConfiguration,
    EspWifi, PmfConfiguration, WifiDriver, WifiEvent,
};
use panelet_core::{pcf8563_utc_to_unix_seconds, unix_seconds_to_civil, RtcSample};
use panelet_management::wifi_profiles::{WifiProfiles, WIFI_PROFILES_MAX_BYTES};
use panelet_management::{
    authorize_url, classify_wifi_disconnect_reason, compile_host_permissions,
    derive_admin_bootstrap_token, derive_setup_ap_password, format_serial_provision_result,
    parse_serial_hello, parse_serial_provision_request, validate_admin_token, Broker, Command,
    JobState, NetworkStatus, ParsedUrl, QueuedCommand, SerialProvisionState, StructuredError,
    TimeConfiguration, WidgetCandidate, WidgetResource, Worker, ADMIN_TOKEN_MAX_LEN,
    NTP_SERVER_MAX_LEN, RPC_BODY_MAX, SERIAL_PROVISION_LINE_MAX,
};
use panelet_rune_abi_poc::display::{DisplayBroker, Presentation, SubmissionState};
use panelet_rune_abi_poc::lifecycle::{ComponentEvent, ComponentSession};
use panelet_rune_abi_poc::{
    context_with_os, mvp_context, BatteryReading, DeviceInfo, HostError, HostFuture, HostIo,
    HostResult, HttpRequest, HttpResponse, LocalTimeReading, OsBindings, RtcReading, Std,
    StdConfig, WifiStatus,
};
use panelet_widget_store::{StoreError, WidgetStore};
use rune::alloc::limit;
use rune::{Context as RuneContext, Source, Sources, Vm};
use serde_json::json;

use crate::storage::{AssetStorage, StorageStatus};

const NVS_NAMESPACE: &str = "note4c_net";
const CREDENTIAL_SCHEMA: u8 = 1;
const TIME_CONFIGURATION_SCHEMA: u8 = 1;
const RTC_OWNERSHIP_SCHEMA: u8 = 1;
const AP_ADDRESS: &str = "192.168.4.1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const NETWORK_POLL: Duration = Duration::from_millis(50);
const HTTP_STACK_SIZE: usize = 10 * 1024;
const INDEX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/server.html.gz"));
const WIDGET_INSTRUCTION_BUDGET: usize = 100_000;
const WIDGET_MEMORY_LIMIT: usize = 64 * 1024;
// The TODO source overflows the former 32 KiB compiler worker on hardware.
// Keep one bounded worker and report its stack watermark after compile/run.
const WIDGET_STACK_SIZE: usize = 64 * 1024;
// The bundled examples exhaust 256-512 KiB during Rune's transient compiler
// allocations. Keep compilation bounded while giving both measured headroom.
const WIDGET_COMPILE_MEMORY_LIMIT: usize = 1024 * 1024;
const WIDGET_WALL_TIME: Duration = Duration::from_secs(120);
const HTTP_WORKER_COUNT: usize = 2;
const HTTP_QUEUE_CAPACITY: usize = 4;
const RUNTIME_HARDWARE_QUEUE_CAPACITY: usize = 8;
// A queued RTC operation can arrive just after the main hardware owner starts
// a measured ~25 s full EPD refresh. Keep this bounded, but long enough to
// cover one full-refresh-only panel transaction plus scheduling headroom.
const RTC_COORDINATION_TIMEOUT: Duration = Duration::from_secs(45);
const HTTP_RESPONSE_BUFFER: usize = 1024;
const HTTP_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "location",
    "cache-control",
    "etag",
    "last-modified",
    "retry-after",
];

#[derive(Clone, Copy)]
struct WifiDisconnectObservation {
    reason: u16,
    rssi: i8,
}

#[derive(Default)]
struct WifiAttemptObservation {
    expected_ssid: Option<String>,
    associated: bool,
    disconnect: Option<WifiDisconnectObservation>,
}

#[derive(Clone, Copy)]
struct WifiConnectionFailure {
    code: &'static str,
    message: &'static str,
    disconnect: Option<WifiDisconnectObservation>,
}

enum WifiConnectionOutcome {
    Connected,
    Rejected(WifiConnectionFailure),
}

enum CandidateConnectionOutcome {
    Connected(Credentials),
    Rejected(WifiConnectionFailure),
}

fn is_terminal_connection_disconnect(reason: u16) -> bool {
    u32::from(reason) != esp_idf_svc::sys::wifi_err_reason_t_WIFI_REASON_ROAMING
}

/// A one-shot completion shared by a widget future and the main hardware
/// loop.  The producer never blocks waiting for the 24-second panel refresh;
/// it only stores the result and wakes the VM thread.
pub struct Reply<T> {
    state: Arc<Mutex<ReplyState<T>>>,
}

struct ReplyState<T> {
    result: Option<HostResult<T>>,
    waker: Option<Waker>,
}

impl<T> Clone for Reply<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<T> Reply<T> {
    fn pair() -> (Self, ReplyFuture<T>) {
        let reply = Self {
            state: Arc::new(Mutex::new(ReplyState {
                result: None,
                waker: None,
            })),
        };
        let future = ReplyFuture {
            reply: reply.clone(),
        };
        (reply, future)
    }

    pub fn complete(&self, result: HostResult<T>) {
        let waker = self.state.lock().ok().and_then(|mut state| {
            if state.result.is_none() {
                state.result = Some(result);
            }
            state.waker.take()
        });
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

struct ReplyFuture<T> {
    reply: Reply<T>,
}

impl<T> Future for ReplyFuture<T> {
    type Output = HostResult<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Ok(mut state) = self.reply.state.lock() else {
            return Poll::Ready(Err(HostError::new(
                "internal",
                "runtime",
                "completion lock poisoned",
                false,
            )));
        };
        if let Some(result) = state.result.take() {
            Poll::Ready(result)
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// Requests consumed by the single main-loop owner of NOTE4C hardware.
/// Keeping the completion handles in the enum makes it impossible for the
/// network/widget tasks to access `Board` concurrently.
pub enum RuntimeHardwareRequest {
    Battery {
        cancelled: Arc<AtomicBool>,
        reply: Reply<BatteryReading>,
    },
    Rtc {
        cancelled: Arc<AtomicBool>,
        reply: Reply<RtcReading>,
    },
    RtcSetSystem {
        cancelled: Arc<AtomicBool>,
        reply: Reply<()>,
    },
}

pub struct ServerHandle {
    polling_enabled: Arc<AtomicBool>,
    broker: Arc<Mutex<Broker>>,
    hardware: Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
    main_screens: Arc<MainScreenController>,
    display: DisplayBroker,
}

#[derive(Clone)]
pub struct ProvisioningInfo {
    pub ap_ssid: String,
    pub ap_password: String,
    pub admin_token: String,
    pub address: &'static str,
}

#[derive(Clone)]
pub struct ProvisionedInfo {
    pub ssid: String,
    pub address: String,
    pub admin_token: String,
}

#[derive(Clone)]
pub enum MainScreen {
    Provisioning(ProvisioningInfo),
    Provisioned(ProvisionedInfo),
}

struct MainScreenState {
    pending: VecDeque<MainScreen>,
    provisioning: ProvisioningInfo,
    last_network_screen: Option<MainScreen>,
}

struct MainScreenController {
    state: Mutex<MainScreenState>,
}

impl MainScreenController {
    fn new(provisioning: ProvisioningInfo, show_provisioning: bool) -> Self {
        let mut pending = VecDeque::with_capacity(1);
        let last_network_screen =
            show_provisioning.then(|| MainScreen::Provisioning(provisioning.clone()));
        if show_provisioning {
            pending.push_back(MainScreen::Provisioning(provisioning.clone()));
        }
        Self {
            state: Mutex::new(MainScreenState {
                pending,
                provisioning,
                last_network_screen,
            }),
        }
    }

    fn show_provisioning(&self) {
        if let Ok(mut state) = self.state.lock() {
            let screen = MainScreen::Provisioning(state.provisioning.clone());
            state.last_network_screen = Some(screen.clone());
            state.pending.clear();
            state.pending.push_back(screen);
        }
    }

    fn show_provisioned(&self, ssid: String, address: String) {
        if let Ok(mut state) = self.state.lock() {
            let screen = MainScreen::Provisioned(ProvisionedInfo {
                ssid,
                address,
                admin_token: state.provisioning.admin_token.clone(),
            });
            state.last_network_screen = Some(screen.clone());
            state.pending.clear();
            state.pending.push_back(screen);
        }
    }

    fn update_admin_token(&self, token: String) {
        if let Ok(mut state) = self.state.lock() {
            state.provisioning.admin_token = token.clone();
            let refreshed = state
                .last_network_screen
                .clone()
                .map(|screen| match screen {
                    MainScreen::Provisioning(mut info) => {
                        info.admin_token = token.clone();
                        MainScreen::Provisioning(info)
                    }
                    MainScreen::Provisioned(mut info) => {
                        info.admin_token = token;
                        MainScreen::Provisioned(info)
                    }
                });
            if let Some(screen) = refreshed {
                state.last_network_screen = Some(screen.clone());
                state.pending.clear();
                state.pending.push_back(screen);
            }
        }
    }

    fn take(&self) -> Option<MainScreen> {
        self.state.lock().ok()?.pending.pop_front()
    }
}

enum WidgetFailure {
    Static(&'static str),
    Host(HostError),
}

impl From<&'static str> for WidgetFailure {
    fn from(code: &'static str) -> Self {
        Self::Static(code)
    }
}

impl WidgetFailure {
    fn into_job_error(self) -> StructuredError {
        match self {
            Self::Static(code) => StructuredError::job(code, "widget operation failed"),
            Self::Host(error) => StructuredError::job(
                "widget_host_error",
                "widget host operation returned an error",
            )
            .with_details(json!({
                "code": error.code,
                "group": error.group,
                "message": error.message,
                "retryable": error.retryable,
            })),
        }
    }
}

impl ServerHandle {
    pub fn start(modem: Modem<'static>) -> Result<Self, EspError> {
        let partition = EspDefaultNvsPartition::take()?;
        let store = CredentialStore::new(partition.clone())?;
        let admin_store = Arc::new(Mutex::new(CredentialStore::new(partition.clone())?));
        let active = store.load_active()?;
        let time_configuration = store.load_time_configuration()?.unwrap_or_default();
        let rtc_owned = store.rtc_owned()?;
        let factory_identity = factory_identity()?;
        let ap_ssid = ap_ssid(&factory_identity);
        let ap_password = derive_setup_ap_password(&factory_identity);
        let admin_token = store
            .load_admin_custom_token()?
            .unwrap_or_else(|| derive_admin_bootstrap_token(&factory_identity));
        let provisioning = ProvisioningInfo {
            ap_ssid: ap_ssid.clone(),
            ap_password: ap_password.clone(),
            admin_token: admin_token.clone(),
            address: AP_ADDRESS,
        };
        let main_screens = Arc::new(MainScreenController::new(provisioning, active.is_none()));
        let mut network = NetworkStatus::provisioning(ap_ssid);
        if active.is_some() {
            network.provisioning = false;
            network.mode = "connecting";
        }
        let broker = Arc::new(Mutex::new(Broker::new(
            network,
            env!("CARGO_PKG_VERSION"),
            admin_token,
        )));
        if let Ok(mut state) = broker.lock() {
            state.set_saved_wifi(store.load_profiles()?.names());
        }
        let storage = Arc::new(AssetStorage::probe());
        if let Ok(mut state) = broker.lock() {
            state.set_storage_ready(storage.status() == StorageStatus::Ready);
            state.set_time_configuration(time_configuration);
        }
        restore_widgets(&broker, &storage);
        let hardware = Arc::new(Mutex::new(VecDeque::with_capacity(
            RUNTIME_HARDWARE_QUEUE_CAPACITY,
        )));
        let http_queue = HttpQueue::new();
        start_http_workers(&http_queue)?;
        // Start the consumer before making the HTTP producer reachable. Rust
        // threads cannot be rolled back after spawn; this ordering avoids a
        // live management endpoint whose widget jobs can never be consumed.
        let widget_broker = Arc::clone(&broker);
        let worker_storage = Arc::clone(&storage);
        let worker_hardware = Arc::clone(&hardware);
        let worker_http = Arc::clone(&http_queue);
        let display = DisplayBroker::new();
        let worker_display = display.clone();
        let polling_enabled = Arc::new(AtomicBool::new(false));
        let worker_polling = Arc::clone(&polling_enabled);
        thread::Builder::new()
            .name("panelet-widget".into())
            .stack_size(WIDGET_STACK_SIZE)
            .spawn(move || {
                widget_task(
                    widget_broker,
                    worker_storage,
                    worker_hardware,
                    worker_http,
                    worker_polling,
                    worker_display,
                )
            })
            .map_err(|_| EspError::from_infallible::<{ esp_idf_svc::sys::ESP_FAIL }>())?;
        let worker_broker = Arc::clone(&broker);
        let server_storage = Arc::clone(&storage);
        let network_screens = Arc::clone(&main_screens);
        let network_admin_store = Arc::clone(&admin_store);
        let network_hardware = Arc::clone(&hardware);
        let network_display=display.clone();
        thread::Builder::new()
            .name("panelet-network".into())
            .stack_size(24 * 1024)
            .spawn(move || {
                if let Err(error) = network_task(
                    modem,
                    partition,
                    store,
                    worker_broker,
                    server_storage,
                    network_screens,
                    network_admin_store,
                    network_hardware,
                    active,
                    ap_password,
                    rtc_owned,
                    network_display,
                ) {
                    log::error!("management network task stopped: {}", error);
                }
            })
            .map_err(|_| EspError::from_infallible::<{ esp_idf_svc::sys::ESP_FAIL }>())?;
        if let Err(error) = start_serial_provisioning(Arc::clone(&broker)) {
            log::warn!(
                "serial provisioning unavailable; initialization error code: {}",
                error.code()
            );
        }
        Ok(Self {
            polling_enabled,
            broker,
            hardware,
            main_screens,
            display,
        })
    }

    pub fn enable_widget_polling(&self, enabled: bool) {
        self.polling_enabled.store(enabled, Ordering::SeqCst);
        if enabled && !self.diagnostic_mode() {
            let _ = self.display.retry(uptime_ms().max(0) as u64);
        }
    }
    pub fn diagnostic_mode(&self) -> bool {
        self.broker
            .lock()
            .map(|b| b.system.mode == "diagnostic")
            .unwrap_or(true)
    }
    pub fn set_safe_mode(&self, safe: bool) {
        if let Ok(mut broker) = self.broker.lock() {
            broker.system.safe_mode = safe;
            if safe {
                broker.system.mode = "diagnostic";
            }
        }
    }
    pub fn invalidate_screen(&self) {
        self.display.invalidate_presented();
    }
    pub fn take_presentation(&self) -> Option<Presentation> {
        if self.diagnostic_mode() {
            return None;
        }
        let result = self.display.poll(uptime_ms().max(0) as u64);
        if result.is_none() && matches!(self.display.status().state, SubmissionState::Unchanged) {
            self.set_main_screen("widget");
        }
        self.update_presentation();
        result
    }
    pub fn finish_presentation(&self, frame: Presentation, result: Result<(), String>) {
        let succeeded = result.is_ok();
        let _ = self.display.finish(frame, result);
        if succeeded {
            self.set_main_screen("widget");
        }
        self.update_presentation();
    }
    fn update_presentation(&self) {
        if let Ok(mut broker) = self.broker.lock() {
            sync_presentation(&mut broker, &self.display);
        }
    }

    pub fn command_is_current(&self, command: &QueuedCommand, diagnostic: bool) -> bool {
        self.broker
            .lock()
            .map(|b| {
                b.system.routing_epoch == command.routing_epoch
                    && (!diagnostic || b.system.mode == "diagnostic")
            })
            .unwrap_or(false)
    }

    pub fn take_main_command(&self) -> Option<QueuedCommand> {
        self.broker.lock().ok()?.take_for(Worker::Main)
    }

    pub fn take_runtime_hardware(&self) -> Option<RuntimeHardwareRequest> {
        self.hardware.lock().ok()?.pop_front()
    }

    pub fn take_main_screen(&self) -> Option<MainScreen> {
        self.main_screens.take()
    }

    pub fn finish(&self, job_id: u32, succeeded: bool, result: serde_json::Value) {
        if let Ok(mut broker) = self.broker.lock() {
            broker.finish_job(job_id, succeeded, result);
        }
    }

    pub fn set_display_busy(&self, busy: bool) {
        if let Ok(mut broker) = self.broker.lock() {
            broker.set_display_busy(busy);
        }
    }

    pub fn set_main_screen(&self, screen: &'static str) {
        if let Ok(mut broker) = self.broker.lock() {
            broker.set_main_screen(screen);
        }
    }

    pub fn update_heap(&self) {
        let free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
        let minimum = unsafe { esp_idf_svc::sys::esp_get_minimum_free_heap_size() };
        if let Ok(mut broker) = self.broker.lock() {
            broker.update_heap(free, minimum);
        }
    }
}

fn start_serial_provisioning(broker: Arc<Mutex<Broker>>) -> Result<(), EspError> {
    unsafe {
        if !esp_idf_svc::sys::usb_serial_jtag_is_driver_installed() {
            let mut configuration = esp_idf_svc::sys::usb_serial_jtag_driver_config_t {
                tx_buffer_size: 1024,
                rx_buffer_size: 1024,
            };
            EspError::convert(esp_idf_svc::sys::usb_serial_jtag_driver_install(
                &mut configuration,
            ))?;
        }
        // The generated bindings expose the compatible legacy alias. It
        // switches stdin/stdout from the tiny polling FIFO to the installed
        // interrupt-driven driver for bounded line reception.
        esp_idf_svc::sys::esp_vfs_usb_serial_jtag_use_driver();
    }
    thread::Builder::new()
        .name("panelet-serial".into())
        .stack_size(8 * 1024)
        .spawn(move || serial_provisioning_task(broker))
        .map(|_| ())
        .map_err(|_| EspError::from_infallible::<{ esp_idf_svc::sys::ESP_FAIL }>())
}

fn serial_provisioning_task(broker: Arc<Mutex<Broker>>) {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut line = Vec::with_capacity(256);
    let mut overlong = false;
    loop {
        // The ESP-IDF USB Serial/JTAG VFS returns -1 while no host is sending
        // USB SOF packets (for example, on battery or a charge-only supply).
        // Stay available for a later host connection instead of terminating
        // the provisioning thread at boot. A disconnect also invalidates any
        // partial command received from the previous serial session.
        if unsafe { !esp_idf_svc::sys::usb_serial_jtag_is_connected() } {
            line.clear();
            overlong = false;
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        let mut byte = [0_u8; 1];
        match input.read(&mut byte) {
            Ok(0) => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Ok(_) if byte[0] != b'\n' => {
                if line.len() < SERIAL_PROVISION_LINE_MAX {
                    line.push(byte[0]);
                } else {
                    overlong = true;
                }
                continue;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                // Connection state can change between the check above and
                // read(). Treat VFS failures as transient so unplug/replug
                // never permanently disables serial provisioning.
                line.clear();
                overlong = false;
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        }

        if !overlong {
            if let Some(request_id) = parse_serial_hello(&line) {
                write_serial_provision_result(request_id, SerialProvisionState::Ready, None, None);
            } else if line
                .starts_with(panelet_management::SERIAL_PROVISION_REQUEST_PREFIX.as_bytes())
            {
                handle_serial_provision_line(&broker, &line);
            }
        }
        line.clear();
        overlong = false;
    }
}

fn handle_serial_provision_line(broker: &Arc<Mutex<Broker>>, line: &[u8]) {
    let request = match parse_serial_provision_request(line) {
        Ok(request) => request,
        Err(error) => {
            if let Some(request_id) = error.request_id {
                write_serial_provision_result(
                    &request_id,
                    SerialProvisionState::Failed,
                    None,
                    Some(error.code),
                );
            }
            return;
        }
    };
    let job_id = match broker
        .lock()
        .map_err(|_| "broker_unavailable")
        .and_then(|mut broker| {
            broker
                .enqueue_host(Command::WifiConfigure(request.credentials))
                .map_err(|error| error.code)
        }) {
        Ok(job_id) => job_id,
        Err(code) => {
            write_serial_provision_result(
                &request.request_id,
                SerialProvisionState::Failed,
                None,
                Some(code),
            );
            return;
        }
    };
    write_serial_provision_result(
        &request.request_id,
        SerialProvisionState::Accepted,
        Some(job_id),
        None,
    );

    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let (state, failure_code) = broker
            .lock()
            .ok()
            .map(|broker| {
                (
                    broker.job_state(job_id),
                    broker.job_failure_code(job_id).map(serial_job_error_code),
                )
            })
            .unwrap_or((None, None));
        match state {
            Some(JobState::Succeeded) => {
                write_serial_provision_result(
                    &request.request_id,
                    SerialProvisionState::Succeeded,
                    Some(job_id),
                    None,
                );
                return;
            }
            Some(JobState::Failed) => {
                let code = failure_code.unwrap_or("network_rejected");
                write_serial_provision_result(
                    &request.request_id,
                    SerialProvisionState::Failed,
                    Some(job_id),
                    Some(code),
                );
                return;
            }
            Some(JobState::Queued | JobState::Running) => {}
            None => {
                write_serial_provision_result(
                    &request.request_id,
                    SerialProvisionState::Failed,
                    Some(job_id),
                    Some("job_unavailable"),
                );
                return;
            }
        }
        if Instant::now() >= deadline {
            write_serial_provision_result(
                &request.request_id,
                // The worker owns the command once it is running and may
                // still promote the candidate. Report that ambiguity instead
                // of claiming the operation failed while it can change NVS
                // later.
                SerialProvisionState::Pending,
                Some(job_id),
                Some("device_timeout"),
            );
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn serial_job_error_code(code: &str) -> &'static str {
    match code {
        "wifi_ap_not_found" => "wifi_ap_not_found",
        "wifi_authentication_timeout" => "wifi_authentication_timeout",
        "wifi_authentication_failed" => "wifi_authentication_failed",
        "wifi_security_incompatible" => "wifi_security_incompatible",
        "wifi_association_failed" => "wifi_association_failed",
        "wifi_signal_too_weak" => "wifi_signal_too_weak",
        "wifi_beacon_timeout" => "wifi_beacon_timeout",
        "wifi_dhcp_timeout" => "wifi_dhcp_timeout",
        "wifi_connection_timeout" => "wifi_connection_timeout",
        "wifi_connection_failed" => "wifi_connection_failed",
        "wifi_driver_error" => "wifi_driver_error",
        _ => "network_rejected",
    }
}

fn write_serial_provision_result(
    request_id: &str,
    state: SerialProvisionState,
    job_id: Option<u32>,
    code: Option<&str>,
) {
    let message = format_serial_provision_result(request_id, state, job_id, code);
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let _ = std::io::Write::write_all(&mut output, message.as_bytes());
    let _ = std::io::Write::flush(&mut output);
}

fn restore_widgets(broker: &Arc<Mutex<Broker>>, storage: &Arc<AssetStorage>) {
    log::info!("widget storage status: {:?}", storage.status());
    if storage.status() != StorageStatus::Ready {
        return;
    }
    let report = match storage.with_store(WidgetStore::load_widgets) {
        Ok(report) => report,
        Err(error) => {
            log::error!("widget store recovery failed: {}", error);
            return;
        }
    };
    for issue in report.issues {
        log::warn!(
            "ignored stored widget {:?}: {}",
            issue.physical_name,
            issue.message
        );
    }
    for candidate in report.widgets {
        let widget_id = candidate.manifest.id.clone();
        // Upload compiles before committing the package, and the store
        // re-decodes its verified bytes before publication. Rebuilding a full
        // Rune compiler context for every package during boot needlessly
        // consumes scarce internal RAM; execution still compiles with the
        // same bounded path and reports any cross-version incompatibility.
        if let Err(error) = broker
            .lock()
            .map_err(|_| "broker lock")
            .and_then(|mut broker| {
                broker
                    .commit_widget(candidate)
                    .map_err(|_| "broker admission")
            })
        {
            log::warn!("ignored stored widget during restore: {}", error);
            continue;
        }
        match storage.with_store(|store| store.widget_settings(&widget_id)) {
            Ok(settings) => {
                if let Ok(mut state) = broker.lock() {
                    if state.commit_widget_settings(&widget_id, settings).is_err() {
                        log::warn!(
                            "stored widget configuration rejected; automatic polling disabled"
                        );
                    }
                }
            }
            Err(_) => {
                log::warn!("stored widget configuration unreadable; automatic polling disabled")
            }
        }
    }
    if let Ok(active) = storage.with_store(WidgetStore::active_widget) {
        if let Ok(mut broker) = broker.lock() {
            let _ = broker.activate_widget(active.as_deref().unwrap_or(""));
        }
    }
}

fn sync_input_stats(broker:&mut Broker) {
    let input=crate::input::stats();
    broker.system.input=json!({"captured":input.captured,"overflow":input.overflow,"pending":input.pending,"minimum_stack_free_bytes":input.minimum_stack_free_bytes});
}

fn sync_presentation(broker: &mut Broker, display: &DisplayBroker) {
    let status = display.status();
    let state = match status.state {
        SubmissionState::Idle => "idle",
        SubmissionState::Pending => "pending",
        SubmissionState::Presenting => "presenting",
        SubmissionState::Presented => "presented",
        SubmissionState::Unchanged => "unchanged",
        SubmissionState::Failed => "failed",
    };
    let (refresh_mode, window) = match status.refresh {
        Some(panelet_rune_abi_poc::display::RefreshKind::Full) => (Some("full"), None),
        Some(panelet_rune_abi_poc::display::RefreshKind::Partial(w)) => (Some("partial"), Some(json!({
            "x":w.x_start(), "y":w.y_start(), "width":w.x_end()-w.x_start()+1,
            "height":w.y_end()-w.y_start()+1
        }))),
        None => (None, None),
    };
    broker.system.presentation = json!({"submission_id":status.submission_id,"state":state,"refreshing":status.refreshing,"error":status.error,"refresh_mode":refresh_mode,"window":window});
    sync_input_stats(broker);
}

fn run_widget_with(
    broker: &Arc<Mutex<Broker>>,
    storage: &Arc<AssetStorage>,
    hardware: &Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
    http: &Arc<HttpQueue>,
    display: &DisplayBroker,
    id: &str,
    session: &mut Option<(String, ComponentSession)>,
    event: ComponentEvent,
) -> Result<serde_json::Value, WidgetFailure> {
    let spec = broker
        .lock()
        .ok()
        .and_then(|broker| broker.widget_runtime_spec(id))
        .ok_or("widget_not_found")?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let io = ManagementHostIo {
        widget_id: spec.id.clone(),
        broker: Arc::clone(broker),
        storage: Arc::clone(storage),
        hardware: Arc::clone(hardware),
        http: Arc::clone(http),
        host_permissions: spec.host_permissions.clone(),
        resources: spec.resources.clone(),
        cancelled: Arc::clone(&cancelled),
        display: display.clone(),
    };
    let (unix_ms, utc_offset_minutes) = broker
        .lock()
        .map(|state| {
            let configuration = state.time_configuration();
            (state.current_unix_ms(), configuration.utc_offset_minutes())
        })
        .unwrap_or((None, 0));
    let local_time = unix_ms.and_then(|unix_ms| {
        let local_seconds = unix_ms
            .div_euclid(1000)
            .checked_add(i64::from(utc_offset_minutes) * 60)?;
        let civil = unix_seconds_to_civil(local_seconds).ok()?;
        Some(LocalTimeReading {
            year: civil.year,
            month: civil.month,
            day: civil.day,
            weekday: civil.weekday,
            hour: civil.hour,
            minute: civil.minute,
            second: civil.second,
            utc_offset_minutes,
            unix_ms,
        })
    });
    let mut config = StdConfig {
        widget_config: spec.settings.values.to_string(),
        uptime_ms: uptime_ms().max(0) as u64,
        unix_ms,
        local_time,
        capabilities: spec.capabilities,
        host_permissions: spec.host_permissions,
        device: DeviceInfo {
            model: "NOTE4C".to_owned(),
            firmware: env!("CARGO_PKG_VERSION").to_owned(),
            screen_width: panelet_rune_abi_poc::CANVAS_WIDTH,
            screen_height: panelet_rune_abi_poc::CANVAS_HEIGHT,
            color_count: 4,
        },
        wall_time_ms: WIDGET_WALL_TIME.as_millis() as u64,
    };
    if session.is_none() && matches!(event, ComponentEvent::BeforeDestroy) {
        return Ok(json!({"destroyed":true}));
    }
    if session.is_none() {
        display.reset_canvas_base();
        let bindings = OsBindings::new(display.clone(), display.epoch());
        let vm = compile_widget_vm(&spec.source, &bindings)?;
        *session = Some((
            id.to_owned(),
            ComponentSession::new(vm, bindings, WIDGET_MEMORY_LIMIT, WIDGET_INSTRUCTION_BUDGET)
                .map_err(WidgetFailure::Host)?,
        ));
    }
    let (_, runtime) = session.as_mut().ok_or("widget_session_missing")?;
    let deadline = if matches!(event, ComponentEvent::BeforeDestroy) {
        Duration::from_secs(5)
    } else {
        WIDGET_WALL_TIME
    };
    config.uptime_ms = uptime_ms().max(0) as u64;
    let result = poll_widget_future(
        runtime.dispatch(Std::new(Box::new(io), config), event),
        deadline,
    );
    cancelled.store(true, Ordering::Release);
    let result = match result {
        Some(result) => result.map_err(WidgetFailure::Host),
        None => Err(WidgetFailure::Static("rune_execution_deadline")),
    };
    if result.is_err() || matches!(event, ComponentEvent::BeforeDestroy) {
        *session = None;
    }
    log_widget_stack("run_finish");
    result?;
    Ok(json!({
        "widget_id": spec.id,
        "completed": true,
        "instruction_budget": WIDGET_INSTRUCTION_BUDGET,
        "rune_memory_limit_bytes": WIDGET_MEMORY_LIMIT,
    }))
}

fn validate_and_commit_widget_with(
    broker: &Arc<Mutex<Broker>>,
    storage: &Arc<AssetStorage>,
    candidate: WidgetCandidate,
) -> Result<String, &'static str> {
    compile_rune_source(&candidate.source)?;
    storage
        .with_store(|store| store.put_widget(&candidate))
        .map_err(|_| "widget_store_failed")?;
    broker
        .lock()
        .map_err(|_| "broker_unavailable")?
        .commit_widget(candidate)
        .map_err(|_| "widget_commit_failed")
}

fn destroy_widget_session(
    broker: &Arc<Mutex<Broker>>,
    storage: &Arc<AssetStorage>,
    hardware: &Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
    http: &Arc<HttpQueue>,
    display: &DisplayBroker,
    session: &mut Option<(String, ComponentSession)>,
) {
    if let Some((id, _)) = session.as_ref() {
        let id = id.clone();
        if let Err(error) = run_widget_with(
            broker,
            storage,
            hardware,
            http,
            display,
            &id,
            session,
            ComponentEvent::BeforeDestroy,
        ) {
            log::warn!(
                "component before_destroy failed: {}",
                error.into_job_error()
            );
        }
    }
    *session = None;
}

fn widget_task(
    broker: Arc<Mutex<Broker>>,
    storage: Arc<AssetStorage>,
    hardware: Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
    http: Arc<HttpQueue>,
    polling_enabled: Arc<AtomicBool>,
    display: DisplayBroker,
) -> ! {
    let mut session = None;
    let mut input_epoch = 1u64;
    loop {
        let diagnostic = broker
            .lock()
            .map(|b| b.system.mode == "diagnostic")
            .unwrap_or(true);
        crate::input::transition(
            input_epoch,
            if diagnostic {
                crate::input::InputMode::Diagnostic
            } else {
                crate::input::InputMode::Normal
            },
        );
        if let Ok(mut state) = broker.lock() {
            sync_presentation(&mut state, &display);
        }
        if let Ok(mut state) = broker.lock() {
            while let Some(widget_id) = state.take_widget_kv_cleanup() {
                if let Err(error) = storage.with_store(|store| store.delete_widget(&widget_id)) {
                    log::error!(
                        "persistent widget cleanup failed for {:?}: {}",
                        widget_id,
                        error
                    );
                }
            }
        }
        if polling_enabled.load(Ordering::SeqCst) {
            if let Ok(mut state) = broker.lock() {
                state.poll_active_widget(uptime_ms().max(0) as u64);
            }
        }
        let mut queued = broker
            .lock()
            .ok()
            .and_then(|mut state| state.take_for(Worker::Widget));
        if queued.is_none() && !diagnostic && polling_enabled.load(Ordering::SeqCst) {
            if let Some(input) = crate::input::peek(crate::input::InputMode::Normal) {
                if input.epoch != input_epoch {
                    let _ = crate::input::pop(crate::input::InputMode::Normal);
                }
                if input.epoch == input_epoch {
                    if let crate::input::InputPayload::Script(event) = input.payload {
                        if let Ok(mut state) = broker.lock() {
                            let id = state.system.active_widget.clone();
                            if id.is_empty() {
                                let _ = crate::input::pop(crate::input::InputMode::Normal);
                            }
                            if !id.is_empty() {
                                let event: panelet_rune_abi_poc::lifecycle::InputEvent =
                                    event.into();
                                if state
                                    .enqueue_host(Command::WidgetInput {
                                        id,
                                        keycode: event.keycode,
                                        kind: event.kind,
                                        held_ms: event.held_ms,
                                    })
                                    .is_ok()
                                {
                                    let _ = crate::input::pop(crate::input::InputMode::Normal);
                                    queued = state.take_for(Worker::Widget);
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(queued) = queued {
            let widget_id = match &queued.command {
                Command::WidgetMount { id }
                | Command::WidgetInput { id, .. }
                | Command::WidgetActivate { id }
                | Command::WidgetPoll { id }
                | Command::WidgetConfigure { id, .. } => id.clone(),
                Command::WidgetUpload(candidate) => candidate.manifest.id.clone(),
                Command::SystemMode { .. } | Command::ScreenRetry => broker
                    .lock()
                    .map(|b| b.system.active_widget.clone())
                    .unwrap_or_default(),
                Command::WidgetDelete { id } => id.clone(),
                _ => unreachable!("widget worker only dequeues widget commands"),
            };
            let is_run = matches!(
                &queued.command,
                Command::WidgetPoll { .. }
                    | Command::WidgetMount { .. }
                    | Command::WidgetInput { .. }
            );
            // A mutation of an inactive package must not evict the foreground.
            let activation = matches!(
                &queued.command,
                Command::WidgetActivate { .. } | Command::SystemMode { .. }
            );
            let mutation = matches!(
                &queued.command,
                Command::WidgetConfigure { .. }
                    | Command::WidgetUpload(_)
                    | Command::WidgetDelete { .. }
            );
            if activation
                || session.as_ref().is_some_and(|(id, _)| {
                    (is_run && id != &widget_id) || (mutation && id == &widget_id)
                })
            {
                destroy_widget_session(&broker, &storage, &hardware, &http, &display, &mut session);
            }
            if activation
                || (mutation
                    && broker
                        .lock()
                        .map(|b| b.system.active_widget == widget_id)
                        .unwrap_or(false))
            {
                let epoch = display.begin_epoch();
                if let Ok(mut state) = broker.lock() {
                    state.system.routing_epoch = epoch;
                }
                input_epoch = input_epoch.wrapping_add(1);
            }
            let result = match queued.command {
                Command::SystemMode { diagnostic } => {
                    if let Ok(mut state)=broker.lock() {
                        state.system.mode=if diagnostic {"diagnostic"} else {"normal"};
                        if !diagnostic {state.request_foreground_mount();}
                        if diagnostic || state.system.active_widget.is_empty() {let _=state.enqueue_host(Command::DisplayStatus);}
                    }
                    Ok(json!({"mode":if diagnostic {"diagnostic"} else {"normal"}}))
                }
                Command::ScreenRetry => display.retry(uptime_ms().max(0) as u64).map(|()|json!({"accepted":true})).map_err(|_|WidgetFailure::Static("no_retryable_frame")),
                Command::WidgetActivate { id } => {
                    storage.with_store(|store| store.set_active_widget(&id))
                        .map_err(|_| WidgetFailure::Static("active_widget_store_failed"))
                        .and_then(|()| {
                            let mut state = broker.lock().map_err(|_| WidgetFailure::Static("broker_unavailable"))?;
                            if !state.activate_widget(&id) { return Err(WidgetFailure::Static("widget_not_found")); }
                            if id.is_empty() { let _ = state.enqueue_host(Command::DisplayStatus); }
                            Ok(json!({"active":id,"persistent":true}))
                        })
                }
                Command::WidgetMount { id } => run_widget_with(&broker, &storage, &hardware, &http, &display, &id, &mut session, ComponentEvent::Mount),
                Command::WidgetInput { id, keycode, kind, held_ms } => run_widget_with(&broker, &storage, &hardware, &http, &display, &id, &mut session, ComponentEvent::Input(panelet_rune_abi_poc::lifecycle::InputEvent {keycode,kind,held_ms})),
                Command::WidgetPoll { id } => {
                    let enabled = polling_enabled.load(Ordering::SeqCst) && broker.lock().map(|state| state.is_widget_poll_enabled(&id)).unwrap_or(false);
                    if enabled {
                        run_widget_with(&broker, &storage, &hardware, &http, &display, &id, &mut session, ComponentEvent::Timer)
                    } else {
                        Ok(json!({"skipped":true,"reason":"inactive_or_disabled"}))
                    }
                }
                Command::WidgetConfigure { id, settings } => {
                    storage.with_store(|store| store.set_widget_settings(&id, &settings))
                        .map_err(|_| WidgetFailure::Static("widget_config_store_failed"))
                        .and_then(|()| {
                            broker.lock().map_err(|_| WidgetFailure::Static("broker_unavailable"))?
                                .commit_widget_settings(&id, settings).map_err(|_| WidgetFailure::Static("widget_config_commit_failed"))?;
                            Ok(json!({"id":id,"configured":true,"persistent":true}))
                        })
                }
                Command::WidgetUpload(candidate) => {
                    validate_and_commit_widget_with(&broker, &storage, candidate)
                        .map(|id| json!({"id": id, "stored":"assets_spiffs", "persistent":true}))
                        .map_err(WidgetFailure::Static)
                }
                Command::WidgetDelete { id } => storage
                    .with_store(|store| store.delete_widget(&id))
                    .map_err(|_| WidgetFailure::Static("widget_store_delete_failed"))
                    .and_then(|()| {
                        broker
                            .lock()
                            .map_err(|_| WidgetFailure::Static("broker_unavailable"))?
                            .commit_widget_delete(&id)
                            .map_err(|_| WidgetFailure::Static("widget_delete_commit_failed"))?;
                        Ok(json!({"id":id,"deleted":true,"private_data_deleted":true,"persistent":true}))
                    }),
                _ => unreachable!("widget worker only dequeues widget commands"),
            };
            if result.is_err() {
                input_epoch = input_epoch.wrapping_add(1);
            }
            let active = broker
                .lock()
                .map(|state| state.system.active_widget.clone())
                .unwrap_or_default();
            if session.as_ref().is_some_and(|(id, _)| id != &active) {
                destroy_widget_session(&broker, &storage, &hardware, &http, &display, &mut session);
            }
            if let Ok(mut state) = broker.lock() {
                if activation
                    || (mutation && widget_id == active)
                    || (is_run && widget_id != active)
                {
                    state.request_foreground_mount();
                }
                if is_run {
                    state.finish_widget_run(&widget_id, uptime_ms().max(0) as u64, result.is_ok());
                }
                match result {
                    Ok(value) => state.finish_job(queued.job_id, true, value),
                    Err(error) => {
                        state.finish_job_with_error(queued.job_id, error.into_job_error())
                    }
                }
                state.finish_widget_operation(&widget_id);
            }
        }
        thread::sleep(NETWORK_POLL);
    }
}

fn compile_widget_vm(source: &str, bindings: &OsBindings) -> Result<Vm, WidgetFailure> {
    let mut sources = Sources::new();
    sources
        .insert(Source::memory(source).map_err(|_| "rune_source_invalid")?)
        .map_err(|_| "rune_source_insert_failed")?;
    let context = build_mvp_context_with(Some(bindings))?;
    let runtime = Arc::new(context.runtime().map_err(|_| "rune_runtime_failed")?);
    let unit = Arc::new(
        limit::with(WIDGET_COMPILE_MEMORY_LIMIT, || {
            rune::prepare(&mut sources).with_context(&context).build()
        })
        .call()
        .map_err(|_| "rune_compile_failed_or_memory_limit")?,
    );
    log_widget_stack("run_compile_finish");
    Ok(Vm::new(runtime, unit))
}

struct ThreadWaker(thread::Thread);

impl std::task::Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn poll_widget_future<F>(future: F, timeout: Duration) -> Option<F::Output>
where
    F: Future,
{
    let mut future = Box::pin(future);
    let deadline = Instant::now() + timeout;
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return Some(value),
            Poll::Pending => {
                let now = Instant::now();
                if now >= deadline {
                    return None;
                }
                thread::park_timeout((deadline - now).min(Duration::from_millis(10)));
            }
        }
    }
}

fn log_widget_stack(stage: &str) {
    let free_units = unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) };
    let free_bytes = free_units as usize * std::mem::size_of::<esp_idf_svc::sys::StackType_t>();
    let internal_free =
        unsafe { esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_INTERNAL) };
    log::info!("{{\"event\":\"widget_stack\",\"stage\":\"{}\",\"minimum_free_bytes\":{},\"configured_bytes\":{},\"internal_heap_free_bytes\":{}}}",stage,free_bytes,WIDGET_STACK_SIZE,internal_free);
}

fn compile_rune_source(source: &str) -> Result<(), &'static str> {
    log_widget_stack("upload_compile_start");
    let mut sources = Sources::new();
    sources
        .insert(Source::memory(source).map_err(|_| "rune_source_invalid")?)
        .map_err(|_| "rune_source_insert_failed")?;
    let context = build_mvp_context()?;
    let unit = limit::with(WIDGET_COMPILE_MEMORY_LIMIT, || {
        rune::prepare(&mut sources).with_context(&context).build()
    })
    .call()
    .map_err(|_| "rune_compile_failed_or_memory_limit")?;
    // Reflection only: do not execute constructors or host calls during admission.
    let vm = Vm::new(Arc::new(Default::default()), Arc::new(unit));
    for name in [
        panelet_rune_abi_poc::lifecycle::COMPONENT_FACTORY,
        panelet_rune_abi_poc::lifecycle::COMPONENT_DISPATCH,
    ] {
        vm.lookup_function([name])
            .map_err(|_| "component_entry_missing")?;
    }
    log_widget_stack("upload_compile_finish");
    Ok(())
}

fn build_mvp_context() -> Result<RuneContext, &'static str> {
    build_mvp_context_with(None)
}

fn build_mvp_context_with(bindings: Option<&OsBindings>) -> Result<RuneContext, &'static str> {
    let internal_before =
        unsafe { esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_INTERNAL) };
    let psram_before =
        unsafe { esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_SPIRAM) };
    match match bindings {
        Some(bindings) => context_with_os(bindings),
        None => mvp_context(),
    } {
        Ok(context) => {
            let internal_after = unsafe {
                esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_INTERNAL)
            };
            let psram_after = unsafe {
                esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_SPIRAM)
            };
            log::info!(
                "Rune context ready: internal_heap={} -> {}, psram_heap={} -> {}",
                internal_before,
                internal_after,
                psram_before,
                psram_after
            );
            Ok(context)
        }
        Err(error) => {
            let internal_largest = unsafe {
                esp_idf_svc::sys::heap_caps_get_largest_free_block(
                    esp_idf_svc::sys::MALLOC_CAP_INTERNAL,
                )
            };
            let psram_largest = unsafe {
                esp_idf_svc::sys::heap_caps_get_largest_free_block(
                    esp_idf_svc::sys::MALLOC_CAP_SPIRAM,
                )
            };
            log::error!(
                "Rune context failed: {}; internal_free={}, internal_largest={}, psram_free={}, psram_largest={}",
                error,
                internal_before,
                internal_largest,
                psram_before,
                psram_largest
            );
            Err("rune_context_failed")
        }
    }
}

fn uptime_ms() -> i64 {
    unsafe { esp_idf_svc::sys::esp_timer_get_time() / 1000 }
}

struct HttpQueueState {
    jobs: VecDeque<HttpWork>,
}

struct HttpQueue {
    state: Mutex<HttpQueueState>,
    wake: Condvar,
}

impl HttpQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(HttpQueueState {
                jobs: VecDeque::with_capacity(HTTP_QUEUE_CAPACITY),
            }),
            wake: Condvar::new(),
        })
    }

    fn push(&self, work: HttpWork) -> Result<(), HttpWork> {
        let Ok(mut state) = self.state.lock() else {
            return Err(work);
        };
        if state.jobs.len() >= HTTP_QUEUE_CAPACITY {
            return Err(work);
        }
        state.jobs.push_back(work);
        self.wake.notify_one();
        Ok(())
    }

    fn pop(&self) -> HttpWork {
        let mut state = self.state.lock().expect("HTTP queue lock poisoned");
        loop {
            if let Some(work) = state.jobs.pop_front() {
                return work;
            }
            state = self.wake.wait(state).expect("HTTP queue lock poisoned");
        }
    }
}

struct HttpWork {
    request: HttpRequest,
    host_permissions: Vec<String>,
    broker: Arc<Mutex<Broker>>,
    cancelled: Arc<AtomicBool>,
    reply: Reply<HttpResponse>,
}

fn start_http_workers(queue: &Arc<HttpQueue>) -> Result<(), EspError> {
    for index in 0..HTTP_WORKER_COUNT {
        let queue = Arc::clone(queue);
        thread::Builder::new()
            .name(format!("panelet-http-{index}"))
            .stack_size(16 * 1024)
            .spawn(move || loop {
                let work = queue.pop();
                let result = if work.cancelled.load(Ordering::Acquire) {
                    Err(HostError::cancelled())
                } else {
                    http_fetch_blocking(
                        &work.request,
                        &work.host_permissions,
                        &work.broker,
                        &work.cancelled,
                    )
                };
                work.reply.complete(result);
            })
            .map_err(|_| EspError::from_infallible::<{ esp_idf_svc::sys::ESP_FAIL }>())?;
    }
    Ok(())
}

struct ManagementHostIo {
    widget_id: String,
    broker: Arc<Mutex<Broker>>,
    storage: Arc<AssetStorage>,
    hardware: Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
    http: Arc<HttpQueue>,
    host_permissions: Vec<String>,
    resources: Vec<WidgetResource>,
    cancelled: Arc<AtomicBool>,
    display: DisplayBroker,
}

struct SleepFuture {
    deadline: Instant,
}

impl Future for SleepFuture {
    type Output = HostResult<()>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if Instant::now() >= self.deadline {
            Poll::Ready(Ok(()))
        } else {
            // The widget executor parks for at most 10 ms between polls. Do
            // not self-wake here: doing so would turn sleep into a busy loop.
            Poll::Pending
        }
    }
}

fn enqueue_hardware(
    queue: &Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
    request: RuntimeHardwareRequest,
) -> HostResult<()> {
    let Ok(mut queue) = queue.lock() else {
        return Err(HostError::new(
            "internal",
            "runtime",
            "hardware queue lock poisoned",
            false,
        ));
    };
    if queue.len() >= RUNTIME_HARDWARE_QUEUE_CAPACITY {
        return Err(HostError::quota("hardware request queue is full"));
    }
    queue.push_back(request);
    Ok(())
}

fn store_result<T>(result: Result<T, StoreError>) -> HostResult<T> {
    result.map_err(|error| match error {
        StoreError::NotInitialized => HostError::new(
            "storage_not_initialized",
            "storage",
            "assets storage requires explicit initialization",
            false,
        ),
        error @ StoreError::AlreadyInitialized => HostError::invalid_argument(error.to_string()),
        error @ (StoreError::InvalidPath | StoreError::InvalidKey) => {
            HostError::invalid_argument(error.to_string())
        }
        error @ StoreError::Limit(_) => HostError::quota(error.to_string()),
        error @ (StoreError::Collision | StoreError::Corrupt(_)) => {
            HostError::new("storage_corrupt", "storage", error.to_string(), false)
        }
        error @ (StoreError::Io(_) | StoreError::Json(_) | StoreError::Package(_)) => {
            HostError::new("storage_io", "storage", error.to_string(), true)
        }
    })
}

impl HostIo for ManagementHostIo {
    fn screen_submitted(&self) {
        if let Ok(mut broker) = self.broker.lock() {
            sync_presentation(&mut broker, &self.display);
        }
    }
    fn http_fetch(&self, request: HttpRequest) -> HostFuture<HttpResponse> {
        let (reply, future) = Reply::pair();
        let work = HttpWork {
            request,
            host_permissions: self.host_permissions.clone(),
            broker: Arc::clone(&self.broker),
            cancelled: Arc::clone(&self.cancelled),
            reply: reply.clone(),
        };
        if self.http.push(work).is_err() {
            reply.complete(Err(HostError::quota("HTTP worker queue is full")));
        }
        Box::pin(future)
    }

    fn storage_has(&self, key: String) -> HostFuture<bool> {
        let result = store_result(
            self.storage
                .with_store(|store| store.kv_has(&self.widget_id, &key)),
        );
        Box::pin(std::future::ready(result))
    }

    fn storage_get(&self, key: String) -> HostFuture<Option<String>> {
        let result = store_result(
            self.storage
                .with_store(|store| store.kv_get(&self.widget_id, &key)),
        );
        Box::pin(std::future::ready(result))
    }

    fn storage_set(&self, key: String, value: String) -> HostFuture<()> {
        let result = store_result(
            self.storage
                .with_store(|store| store.kv_set(&self.widget_id, &key, &value)),
        );
        Box::pin(std::future::ready(result))
    }

    fn storage_delete(&self, key: String) -> HostFuture<()> {
        let result = store_result(
            self.storage
                .with_store(|store| store.kv_delete(&self.widget_id, &key)),
        );
        Box::pin(std::future::ready(result))
    }

    fn resource_get(&self, path: String) -> HostFuture<Option<String>> {
        let result = Ok(self
            .resources
            .iter()
            .find(|resource| resource.path == path)
            .map(|resource| resource.text.clone()));
        Box::pin(std::future::ready(result))
    }

    fn resource_list(&self) -> HostFuture<Vec<String>> {
        let result = Ok(self
            .resources
            .iter()
            .map(|resource| resource.path.clone())
            .collect());
        Box::pin(std::future::ready(result))
    }

    fn fs_private_exists(&self, path: String) -> HostFuture<bool> {
        let result = store_result(
            self.storage
                .with_store(|store| store.private_exists(&self.widget_id, &path)),
        );
        Box::pin(std::future::ready(result))
    }

    fn fs_private_read_text(&self, path: String) -> HostFuture<Option<String>> {
        let result = store_result(
            self.storage
                .with_store(|store| store.private_read(&self.widget_id, &path)),
        );
        Box::pin(std::future::ready(result))
    }

    fn fs_private_write_text(&self, path: String, text: String) -> HostFuture<()> {
        let result = store_result(
            self.storage
                .with_store(|store| store.private_write(&self.widget_id, &path, &text)),
        );
        Box::pin(std::future::ready(result))
    }

    fn fs_private_delete(&self, path: String) -> HostFuture<()> {
        let result = store_result(
            self.storage
                .with_store(|store| store.private_delete(&self.widget_id, &path)),
        );
        Box::pin(std::future::ready(result))
    }

    fn fs_private_list(&self, prefix: String) -> HostFuture<Vec<String>> {
        let result = store_result(
            self.storage
                .with_store(|store| store.private_list(&self.widget_id, &prefix)),
        );
        Box::pin(std::future::ready(result))
    }

    fn fs_shared_exists(&self, path: String) -> HostFuture<bool> {
        let result = store_result(self.storage.with_store(|store| store.shared_exists(&path)));
        Box::pin(std::future::ready(result))
    }

    fn fs_shared_read_text(&self, path: String) -> HostFuture<Option<String>> {
        let result = store_result(self.storage.with_store(|store| store.shared_read(&path)));
        Box::pin(std::future::ready(result))
    }

    fn fs_shared_write_text(&self, path: String, text: String) -> HostFuture<()> {
        let result = store_result(
            self.storage
                .with_store(|store| store.shared_write(&path, &text)),
        );
        Box::pin(std::future::ready(result))
    }

    fn fs_shared_delete(&self, path: String) -> HostFuture<()> {
        let result = store_result(self.storage.with_store(|store| store.shared_delete(&path)));
        Box::pin(std::future::ready(result))
    }

    fn fs_shared_list(&self, prefix: String) -> HostFuture<Vec<String>> {
        let result = store_result(self.storage.with_store(|store| store.shared_list(&prefix)));
        Box::pin(std::future::ready(result))
    }

    fn log(&self, level: String, message: String) -> HostFuture<()> {
        let level = match level.as_str() {
            "debug" => log::Level::Debug,
            "warn" => log::Level::Warn,
            "error" => log::Level::Error,
            _ => log::Level::Info,
        };
        log::log!(
            level,
            "widget_log id={:?} message={:?}",
            self.widget_id,
            message
        );
        Box::pin(std::future::ready(Ok(())))
    }

    fn sleep_ms(&self, duration_ms: u64) -> HostFuture<()> {
        Box::pin(SleepFuture {
            deadline: Instant::now() + Duration::from_millis(duration_ms),
        })
    }

    fn battery(&self) -> HostFuture<BatteryReading> {
        let (reply, future) = Reply::pair();
        let request = RuntimeHardwareRequest::Battery {
            cancelled: Arc::clone(&self.cancelled),
            reply: reply.clone(),
        };
        if let Err(error) = enqueue_hardware(&self.hardware, request) {
            reply.complete(Err(error));
        }
        Box::pin(future)
    }

    fn rtc(&self) -> HostFuture<RtcReading> {
        let (reply, future) = Reply::pair();
        let request = RuntimeHardwareRequest::Rtc {
            cancelled: Arc::clone(&self.cancelled),
            reply: reply.clone(),
        };
        if let Err(error) = enqueue_hardware(&self.hardware, request) {
            reply.complete(Err(error));
        }
        Box::pin(future)
    }

    fn wifi_status(&self) -> HostFuture<WifiStatus> {
        let result = self
            .broker
            .lock()
            .map(|broker| {
                let network = &broker.network;
                WifiStatus {
                    connected: !network.provisioning && network.sta_ip.is_some(),
                    ssid: network
                        .sta_ssid
                        .clone()
                        .unwrap_or_else(|| network.ap_ssid.clone()),
                    ip: network
                        .sta_ip
                        .clone()
                        .or_else(|| Some(network.ap_ip.clone())),
                    provisioning: network.provisioning,
                    mode: network.mode.to_owned(),
                    rssi: None,
                }
            })
            .map_err(|_| HostError::new("internal", "wifi", "network lock poisoned", false));
        Box::pin(std::future::ready(result))
    }

    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

fn http_fetch_blocking(
    request: &HttpRequest,
    raw_permissions: &[String],
    broker: &Arc<Mutex<Broker>>,
    cancelled: &AtomicBool,
) -> HostResult<HttpResponse> {
    let permissions = compile_host_permissions(raw_permissions)
        .map_err(|error| HostError::new("host_permission_invalid", "http", error.message, false))?;
    let mut url = request.url.clone();
    let mut method_name = request.method.clone();
    let mut request_body = request.body.clone();
    let mut redirects = 0_u8;

    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(HostError::cancelled());
        }
        let parsed = ParsedUrl::parse(&url)
            .map_err(|error| HostError::new("url_policy_denied", "http", error.message, false))?;
        let addresses = resolve_destination(&parsed)?;
        let device_ips = device_ips(broker);
        if addresses.iter().any(|address| device_ips.contains(address)) {
            return Err(HostError::new(
                "device_self",
                "http",
                "requests to the device's own address are denied",
                false,
            ));
        }
        let device_self = device_ips.first().copied();
        authorize_url(
            &url,
            &permissions,
            !request.tls_verify,
            &addresses,
            device_self,
        )
        .map_err(|error| HostError::new(error.code, "http", error.message, false))?;

        let configuration = HttpClientConfiguration {
            timeout: Some(Duration::from_millis(request.timeout_ms)),
            follow_redirects_policy: FollowRedirectsPolicy::FollowNone,
            buffer_size: Some(HTTP_RESPONSE_BUFFER),
            buffer_size_tx: Some(HTTP_RESPONSE_BUFFER),
            crt_bundle_attach: if parsed.scheme().as_str() == "https" && request.tls_verify {
                Some(esp_idf_svc::sys::esp_crt_bundle_attach)
            } else {
                None
            },
            ..Default::default()
        };
        let connection = EspHttpConnection::new(&configuration).map_err(|error| {
            HostError::new(
                "http_client_init",
                "http",
                format!("HTTP client initialization failed: {error}"),
                true,
            )
        })?;
        let mut client = HttpClient::wrap(connection);
        let method = client_method(&method_name)?;
        let headers: Vec<(&str, &str)> = request
            .headers
            .as_slice()
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str()))
            .collect();
        let mut outgoing = client.request(method, &url, &headers).map_err(|error| {
            HostError::new(
                "http_request",
                "http",
                format!("HTTP request setup failed: {error}"),
                true,
            )
        })?;
        if let Some(body) = request_body.as_deref() {
            outgoing.write_all(body.as_bytes()).map_err(|error| {
                HostError::new(
                    "http_write",
                    "http",
                    format!("HTTP request body write failed: {error}"),
                    true,
                )
            })?;
        }
        let mut response = outgoing.submit().map_err(|error| {
            HostError::new(
                "http_submit",
                "http",
                format!("HTTP request failed: {error}"),
                true,
            )
        })?;
        if cancelled.load(Ordering::Acquire) {
            return Err(HostError::cancelled());
        }
        // Re-authorize the name after connection as a second rebinding check.
        // ESP-IDF's safe wrapper does not expose the connected socket peer, so
        // this detects DNS changes but does not constitute peer-IP pinning.
        let post_connect_addresses = resolve_destination(&parsed)?;
        if post_connect_addresses
            .iter()
            .any(|address| device_ips.contains(address))
        {
            return Err(HostError::new(
                "device_self",
                "http",
                "post-connect DNS resolved to the device itself",
                false,
            ));
        }
        authorize_url(
            &url,
            &permissions,
            !request.tls_verify,
            &post_connect_addresses,
            device_self,
        )
        .map_err(|error| HostError::new(error.code, "http", error.message, false))?;
        let status = response.status();
        if is_redirect(status) {
            let location = response
                .header("Location")
                .map(str::to_owned)
                .ok_or_else(|| {
                    HostError::new(
                        "redirect_location_missing",
                        "http",
                        "redirect has no Location header",
                        false,
                    )
                })?;
            if redirects >= request.max_redirects {
                return Err(HostError::new(
                    "redirect_limit",
                    "http",
                    "redirect limit reached",
                    false,
                ));
            }
            if parsed.scheme().as_str() == "https" && location.starts_with("http://") {
                return Err(HostError::new(
                    "https_downgrade",
                    "http",
                    "HTTPS redirects may not downgrade to HTTP",
                    false,
                ));
            }
            if !(location.starts_with("http://") || location.starts_with("https://")) {
                return Err(HostError::new(
                    "redirect_location_invalid",
                    "http",
                    "only absolute HTTP(S) redirect locations are supported",
                    false,
                ));
            }
            let next = ParsedUrl::parse(&location).map_err(|error| {
                HostError::new("redirect_location_invalid", "http", error.message, false)
            })?;
            if next.scheme() != parsed.scheme()
                || next.host() != parsed.host()
                || next.port() != parsed.port()
            {
                return Err(HostError::new(
                    "cross_origin_redirect_denied",
                    "http",
                    "cross-origin redirects are denied",
                    false,
                ));
            }
            if status == 303 && method_name != "HEAD" {
                method_name = "GET".to_owned();
                request_body = None;
            }
            url = location;
            redirects = redirects.saturating_add(1);
            continue;
        }

        let max_body = request
            .max_body_bytes
            .min(panelet_rune_abi_poc::HTTP_BODY_MAX_BYTES);
        let mut body = Vec::with_capacity(max_body.min(HTTP_RESPONSE_BUFFER));
        let mut chunk = [0_u8; HTTP_RESPONSE_BUFFER];
        loop {
            let read = response.read(&mut chunk).map_err(|error| {
                HostError::new(
                    "http_read",
                    "http",
                    format!("HTTP response read failed: {error}"),
                    true,
                )
            })?;
            if read == 0 {
                break;
            }
            if body.len().saturating_add(read) > max_body {
                return Err(HostError::new(
                    "response_too_large",
                    "http",
                    format!("response exceeds {max_body} bytes"),
                    false,
                ));
            }
            body.extend_from_slice(&chunk[..read]);
        }
        let body = String::from_utf8(body).map_err(|_| {
            HostError::new(
                "response_not_utf8",
                "http",
                "HTTP response body is not UTF-8",
                false,
            )
        })?;
        let response_headers = response_headers(&response);
        return Ok(HttpResponse::new(status, body).with_headers(response_headers));
    }
}

fn resolve_destination(parsed: &ParsedUrl) -> HostResult<Vec<IpAddr>> {
    if let Some(ip) = parsed.ip() {
        return Ok(vec![ip]);
    }
    let resolved: Vec<IpAddr> = (parsed.host(), parsed.port())
        .to_socket_addrs()
        .map_err(|error| {
            HostError::new(
                "dns_resolution_failed",
                "http",
                format!("DNS resolution failed: {error}"),
                true,
            )
        })?
        .map(|address| address.ip())
        .collect();
    if resolved.is_empty() {
        Err(HostError::new(
            "dns_resolution_empty",
            "http",
            "DNS returned no addresses",
            true,
        ))
    } else {
        Ok(resolved)
    }
}

fn device_ips(broker: &Arc<Mutex<Broker>>) -> Vec<IpAddr> {
    broker
        .lock()
        .ok()
        .into_iter()
        .flat_map(|broker| {
            broker
                .network
                .sta_ip
                .iter()
                .chain(std::iter::once(&broker.network.ap_ip))
                .filter_map(|ip| ip.parse().ok())
                .collect::<Vec<IpAddr>>()
        })
        .collect()
}

fn client_method(method: &str) -> HostResult<HttpClientMethod> {
    Ok(match method {
        "GET" => HttpClientMethod::Get,
        "HEAD" => HttpClientMethod::Head,
        "POST" => HttpClientMethod::Post,
        "PUT" => HttpClientMethod::Put,
        "PATCH" => HttpClientMethod::Patch,
        "DELETE" => HttpClientMethod::Delete,
        "OPTIONS" => HttpClientMethod::Options,
        _ => {
            return Err(HostError::new(
                "http_method_invalid",
                "http",
                "HTTP method is not supported by the adapter",
                false,
            ))
        }
    })
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn response_headers(
    response: &impl embedded_svc::http::Headers,
) -> Vec<panelet_rune_abi_poc::HttpHeader> {
    HTTP_RESPONSE_HEADERS
        .iter()
        .filter_map(|name| {
            response
                .header(name)
                .map(|value| panelet_rune_abi_poc::HttpHeader::new(*name, value))
        })
        .collect()
}

fn set_system_unix_seconds(unix_seconds: i64) -> Result<(), &'static str> {
    let seconds = unix_seconds
        .try_into()
        .map_err(|_| "rtc_time_out_of_range")?;
    let value = esp_idf_svc::sys::timeval {
        tv_sec: seconds,
        tv_usec: 0,
    };
    let result = unsafe { esp_idf_svc::sys::settimeofday(&value, core::ptr::null()) };
    if result == 0 {
        Ok(())
    } else {
        Err("settimeofday_failed")
    }
}

fn queue_rtc_read(
    hardware: &Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
) -> HostResult<RtcReading> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let (reply, future) = Reply::pair();
    enqueue_hardware(
        hardware,
        RuntimeHardwareRequest::Rtc {
            cancelled: Arc::clone(&cancelled),
            reply,
        },
    )?;
    match poll_widget_future(future, RTC_COORDINATION_TIMEOUT) {
        Some(result) => result,
        None => {
            cancelled.store(true, Ordering::Release);
            Err(HostError::timeout("RTC read timed out"))
        }
    }
}

fn restore_system_time_from_rtc(
    broker: &Arc<Mutex<Broker>>,
    hardware: &Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
) {
    let result = queue_rtc_read(hardware).and_then(|rtc| {
        if !rtc.valid || rtc.voltage_low || rtc.century_bit {
            return Err(HostError::new(
                "rtc_untrusted",
                "time",
                "RTC value is not trusted for system clock restore",
                false,
            ));
        }
        let sample = RtcSample {
            second: rtc.second,
            minute: rtc.minute,
            hour: rtc.hour,
            day: rtc.day,
            weekday: rtc.weekday,
            month: rtc.month,
            year: rtc.year_00_99,
            century_bit: rtc.century_bit,
            voltage_low: rtc.voltage_low,
        };
        let seconds = pcf8563_utc_to_unix_seconds(sample).map_err(|_| {
            HostError::new(
                "rtc_untrusted",
                "time",
                "RTC value is outside the owned UTC convention",
                false,
            )
        })?;
        set_system_unix_seconds(seconds)
            .map_err(|code| HostError::new(code, "time", "system clock restore failed", true))
    });
    match result {
        Ok(()) => {
            if let Ok(mut state) = broker.lock() {
                state.set_time_restored_from_rtc();
            }
            log::info!("system clock restored from owned RTC state");
        }
        Err(error) => {
            if let Ok(mut state) = broker.lock() {
                state.set_time_error("rtc_restore_failed");
            }
            log::warn!("owned RTC restore failed: code={}", error.code);
        }
    }
}

fn restart_sntp(
    service: &mut Option<EspSntp<'static>>,
    configuration: &TimeConfiguration,
    pending_sync: &Arc<Mutex<Option<Duration>>>,
    broker: &Arc<Mutex<Broker>>,
) -> Result<(), EspError> {
    *service = None;
    if let Ok(mut state) = broker.lock() {
        state.set_time_syncing();
    }
    let mut conf = SntpConf::default();
    conf.servers.fill("");
    conf.servers[0] = configuration.ntp_server();
    conf.operating_mode = OperatingMode::Poll;
    conf.sync_mode = SyncMode::Immediate;
    let callback_pending = Arc::clone(pending_sync);
    *service = Some(EspSntp::new_with_callback(&conf, move |timestamp| {
        if let Ok(mut pending) = callback_pending.lock() {
            *pending = Some(timestamp);
        }
    })?);
    Ok(())
}

fn persist_sntp_to_rtc(
    store: &CredentialStore,
    broker: &Arc<Mutex<Broker>>,
    hardware: &Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
    timestamp: Duration,
) {
    let unix_ms = i64::try_from(timestamp.as_millis()).unwrap_or(i64::MAX);
    if let Ok(mut state) = broker.lock() {
        state.set_time_synced(unix_ms);
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let (reply, future) = Reply::pair();
    let queued = enqueue_hardware(
        hardware,
        RuntimeHardwareRequest::RtcSetSystem {
            cancelled: Arc::clone(&cancelled),
            reply,
        },
    );
    let result = match queued {
        Ok(()) => match poll_widget_future(future, RTC_COORDINATION_TIMEOUT) {
            Some(result) => result,
            None => {
                cancelled.store(true, Ordering::Release);
                Err(HostError::timeout("RTC write timed out"))
            }
        },
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => match store.set_rtc_owned() {
            Ok(()) => {
                if let Ok(mut state) = broker.lock() {
                    state.set_rtc_updated(true);
                }
                log::info!("NTP time stored in external RTC");
            }
            Err(error) => {
                log::warn!("RTC ownership marker write failed: code={}", error.code());
            }
        },
        Err(error) => {
            log::warn!("NTP succeeded but RTC update failed: code={}", error.code);
        }
    }
}

fn network_task(
    modem: Modem<'static>,
    partition: EspDefaultNvsPartition,
    store: CredentialStore,
    broker: Arc<Mutex<Broker>>,
    storage: Arc<AssetStorage>,
    main_screens: Arc<MainScreenController>,
    admin_store: Arc<Mutex<CredentialStore>>,
    hardware: Arc<Mutex<VecDeque<RuntimeHardwareRequest>>>,
    active: Option<Credentials>,
    ap_password: String,
    rtc_owned: bool,
    display: DisplayBroker,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if rtc_owned {
        restore_system_time_from_rtc(&broker, &hardware);
    } else if let Ok(mut state) = broker.lock() {
        state.set_time_waiting_for_network();
    }
    let pending_sync = Arc::new(Mutex::new(None));
    let mut sntp = None;
    let mut station_connected = false;
    let sys_loop = EspSystemEventLoop::take()?;
    let wifi_observation = Arc::new(Mutex::new(WifiAttemptObservation::default()));
    let driver = WifiDriver::new(modem, sys_loop.clone(), Some(partition))?;
    let sta_netif = EspNetif::new(NetifStack::Sta)?;
    let mut ap_netif_configuration = NetifConfiguration::wifi_default_router();
    ap_netif_configuration.ip_configuration = Some(IpConfiguration::Router(RouterConfiguration {
        subnet: Subnet {
            gateway: Ipv4Addr::new(192, 168, 4, 1),
            mask: Mask(24),
        },
        dhcp_enabled: true,
        dns: None,
        secondary_dns: None,
    }));
    let ap_netif = EspNetif::new_with_conf(&ap_netif_configuration)?;
    let mut wifi = EspWifi::wrap_all(driver, sta_netif, ap_netif)?;
    // Subscribe only after the driver's status tracker and default netif
    // handlers are installed. That makes a diagnostic callback observe the
    // same-or-newer link/IP state instead of racing stale state from them.
    let event_observation = Arc::clone(&wifi_observation);
    let _wifi_diagnostic_subscription =
        sys_loop.subscribe::<WifiEvent, _>(move |event| match event {
            WifiEvent::StaConnected(connected) => {
                if let Ok(mut observation) = event_observation.lock() {
                    if observation
                        .expected_ssid
                        .as_ref()
                        .is_some_and(|ssid| ssid.as_bytes() == connected.ssid())
                    {
                        observation.associated = true;
                        observation.disconnect = None;
                    }
                }
            }
            WifiEvent::StaDisconnected(disconnected) => {
                if let Ok(mut observation) = event_observation.lock() {
                    let event_ssid = disconnected.ssid();
                    let reason = disconnected.reason();
                    if observation
                        .expected_ssid
                        .as_ref()
                        .is_some_and(|ssid| ssid.as_bytes() == event_ssid)
                    {
                        observation.associated = false;
                        if is_terminal_connection_disconnect(reason) {
                            // Copy only bounded diagnostics while the event
                            // payload is alive. Debug formatting the event
                            // would expose the SSID and BSSID on serial.
                            observation.disconnect = Some(WifiDisconnectObservation {
                                reason,
                                rssi: disconnected.rssi(),
                            });
                        }
                    }
                }
            }
            _ => {}
        })?;
    let ap = ap_configuration(&broker, &ap_password)?;
    wifi.set_configuration(&WifiConfiguration::Mixed(
        active
            .as_ref()
            .map(client_configuration)
            .transpose()?
            .unwrap_or_default(),
        ap.clone(),
    ))?;
    wifi.start()?;
    let actual_ap = wifi.ap_netif().get_ip_info()?.ip.to_string();
    if actual_ap != AP_ADDRESS {
        log::error!("unexpected management AP address: {}", actual_ap);
        return Err(Box::new(EspError::from_infallible::<
            { esp_idf_svc::sys::ESP_FAIL },
        >()));
    }

    let _server = start_http_server(
        Arc::clone(&broker),
        storage,
        Arc::clone(&admin_store),
        Arc::clone(&main_screens),
        display,
    )?;
    log::info!("management HTTP server started; credentials are displayed only on the panel");

    let mut next_generation = active
        .as_ref()
        .map(|credentials| credentials.generation.wrapping_add(1).max(1))
        .unwrap_or(1);
    for profile in store.load_profiles()?.iter() {
        let credentials = Credentials::new(profile.ssid(), profile.password(), next_generation)?;
        disconnect_sta(&mut wifi, &wifi_observation)?;
        wifi.set_configuration(&WifiConfiguration::Mixed(
            client_configuration(&credentials)?,
            ap.clone(),
        ))?;
        match try_connect(&mut wifi, &credentials, &wifi_observation)? {
            WifiConnectionOutcome::Connected => {
                connected(&broker, &main_screens, &wifi, &credentials);
                wifi.set_configuration(&WifiConfiguration::Client(client_configuration(
                    &credentials,
                )?))?;
                station_connected = true;
                store.promote_candidate(&credentials)?;
                if let Ok(mut state) = broker.lock() {
                    state.set_saved_wifi(store.load_profiles()?.names());
                }
                let configuration = broker
                    .lock()
                    .map(|state| state.time_configuration())
                    .unwrap_or_default();
                if let Err(error) = restart_sntp(&mut sntp, &configuration, &pending_sync, &broker)
                {
                    if let Ok(mut state) = broker.lock() {
                        state.set_time_error("sntp_start_failed");
                    }
                    log::warn!("SNTP initialization failed: code={}", error.code());
                }
                break;
            }
            WifiConnectionOutcome::Rejected(_) => {
                mark_provisioning(&broker, &main_screens, "saved network unavailable");
            }
        }
    }

    let mut next_reconnect = Instant::now() + Duration::from_secs(60);

    loop {
        if let Some(timestamp) = pending_sync
            .lock()
            .ok()
            .and_then(|mut pending| pending.take())
        {
            persist_sntp_to_rtc(&store, &broker, &hardware, timestamp);
        }
        if let Some(queued) = broker
            .lock()
            .ok()
            .and_then(|mut state| state.take_for(Worker::Network))
        {
            match queued.command {
                Command::WifiScan => {
                    let result = wifi.scan_n::<12>();
                    match result {
                        Ok((aps, total)) => {
                            let values: Vec<_> = aps
                                .into_iter()
                                .map(|ap| json!({"ssid": ap.ssid.as_str(), "rssi": ap.signal_strength, "channel": ap.channel, "secured": ap.auth_method != Some(AuthMethod::None)}))
                                .collect();
                            finish(
                                &broker,
                                queued.job_id,
                                true,
                                json!({"access_points": values, "total_seen": total, "truncated": total > 12}),
                            );
                        }
                        Err(error) => finish(
                            &broker,
                            queued.job_id,
                            false,
                            json!({"error_code": error.code()}),
                        ),
                    }
                }
                Command::WifiForget { ssid } => {
                    let result = (|| -> Result<(), EspError> {
                        let mut profiles = store.load_profiles()?;
                        profiles.forget(&ssid);
                        store.save_profiles(&profiles)?;
                        if let Ok(mut state) = broker.lock() {
                            state.set_saved_wifi(profiles.names());
                        }
                        Ok(())
                    })();
                    finish(
                        &broker,
                        queued.job_id,
                        result.is_ok(),
                        json!({"forgotten":result.is_ok()}),
                    );
                }
                Command::WifiConfigure(credentials) => {
                    if !store.load_profiles()?.can_save(credentials.ssid()) {
                        finish_error(
                            &broker,
                            queued.job_id,
                            StructuredError::job(
                                "wifi_profiles_full",
                                "Forget a saved Wi-Fi network before adding another",
                            ),
                        );
                        continue;
                    }
                    let candidate = Credentials::new(
                        credentials.ssid(),
                        credentials.password(),
                        next_generation,
                    );
                    let outcome = candidate.and_then(|candidate| {
                        store.save_candidate(&candidate)?;
                        station_connected = false;
                        sntp = None;
                        if let Ok(mut pending) = pending_sync.lock() {
                            *pending = None;
                        }
                        // ESP-IDF requires an explicit disconnect before
                        // reconnecting with a changed station configuration.
                        // Waiting for both L2 and the old IPv4 state to clear
                        // also prevents the old network from being promoted as
                        // a successful candidate.
                        disconnect_sta(&mut wifi, &wifi_observation)?;
                        wifi.set_configuration(&WifiConfiguration::Mixed(
                            client_configuration(&candidate)?,
                            ap.clone(),
                        ))?;
                        match try_connect(&mut wifi, &candidate, &wifi_observation)? {
                            WifiConnectionOutcome::Connected => {
                                store.promote_candidate(&candidate)?;
                                if let Ok(mut state) = broker.lock() {
                                    state.set_saved_wifi(store.load_profiles()?.names());
                                }
                                next_generation = candidate.generation.wrapping_add(1).max(1);
                                connected(&broker, &main_screens, &wifi, &candidate);
                                wifi.set_configuration(&WifiConfiguration::Client(
                                    client_configuration(&candidate)?,
                                ))?;
                                station_connected = true;
                                let configuration = broker
                                    .lock()
                                    .map(|state| state.time_configuration())
                                    .unwrap_or_default();
                                if let Err(error) =
                                    restart_sntp(&mut sntp, &configuration, &pending_sync, &broker)
                                {
                                    if let Ok(mut state) = broker.lock() {
                                        state.set_time_error("sntp_start_failed");
                                    }
                                    log::warn!(
                                        "SNTP initialization failed after Wi-Fi connect: code={}",
                                        error.code()
                                    );
                                }
                                Ok(CandidateConnectionOutcome::Connected(candidate))
                            }
                            WifiConnectionOutcome::Rejected(failure) => {
                                if let Err(error) = store.clear_candidate() {
                                    // Preserve the actual connection reason for
                                    // the job. A future candidate save first
                                    // invalidates this slot again.
                                    log::error!(
                                        "failed to invalidate rejected Wi-Fi candidate: code={}",
                                        error.code()
                                    );
                                }
                                mark_provisioning(
                                    &broker,
                                    &main_screens,
                                    "candidate network failed",
                                );
                                Ok(CandidateConnectionOutcome::Rejected(failure))
                            }
                        }
                    });
                    match outcome {
                        Ok(CandidateConnectionOutcome::Connected(candidate)) => finish(
                            &broker,
                            queued.job_id,
                            true,
                            json!({"ssid": candidate.ssid, "connected": true}),
                        ),
                        Ok(CandidateConnectionOutcome::Rejected(failure)) => {
                            finish_error(&broker, queued.job_id, failure.structured_error())
                        }
                        Err(error) => {
                            let _ = store.clear_candidate();
                            mark_provisioning(&broker, &main_screens, "candidate processing error");
                            finish_error(
                                &broker,
                                queued.job_id,
                                StructuredError::job(
                                    "wifi_driver_error",
                                    "Wi-Fi driver rejected the connection operation",
                                )
                                .with_details(json!({"esp_error": error.code()})),
                            );
                        }
                    }
                }
                Command::TimeConfigure(configuration) => {
                    let save = store.save_time_configuration(&configuration);
                    if save.is_ok() {
                        if let Ok(mut state) = broker.lock() {
                            state.set_time_configuration(configuration.clone());
                        }
                    }
                    if let Err(error) = save {
                        finish_error(
                            &broker,
                            queued.job_id,
                            StructuredError::job(
                                "time_config_store_failed",
                                "time configuration could not be persisted",
                            )
                            .with_details(json!({"esp_error": error.code()})),
                        );
                    } else if station_connected {
                        match restart_sntp(&mut sntp, &configuration, &pending_sync, &broker) {
                            Ok(()) => finish(
                                &broker,
                                queued.job_id,
                                true,
                                json!({
                                    "ntp_server": configuration.ntp_server(),
                                    "utc_offset_minutes": configuration.utc_offset_minutes(),
                                    "sync_state": "syncing",
                                }),
                            ),
                            Err(error) => {
                                if let Ok(mut state) = broker.lock() {
                                    state.set_time_error("sntp_start_failed");
                                }
                                finish_error(
                                    &broker,
                                    queued.job_id,
                                    StructuredError::job(
                                        "sntp_start_failed",
                                        "SNTP synchronization could not be started",
                                    )
                                    .with_details(json!({"esp_error": error.code()})),
                                );
                            }
                        }
                    } else {
                        if let Ok(mut state) = broker.lock() {
                            state.set_time_waiting_for_network();
                        }
                        finish(
                            &broker,
                            queued.job_id,
                            true,
                            json!({
                                "ntp_server": configuration.ntp_server(),
                                "utc_offset_minutes": configuration.utc_offset_minutes(),
                                "sync_state": "waiting_network",
                            }),
                        );
                    }
                }
                Command::TimeSync => {
                    if !station_connected {
                        finish_error(
                            &broker,
                            queued.job_id,
                            StructuredError::job(
                                "wifi_not_connected",
                                "SNTP synchronization requires a station connection",
                            ),
                        );
                    } else {
                        let configuration = broker
                            .lock()
                            .map(|state| state.time_configuration())
                            .unwrap_or_default();
                        match restart_sntp(&mut sntp, &configuration, &pending_sync, &broker) {
                            Ok(()) => finish(
                                &broker,
                                queued.job_id,
                                true,
                                json!({"sync_state": "syncing"}),
                            ),
                            Err(error) => {
                                if let Ok(mut state) = broker.lock() {
                                    state.set_time_error("sntp_start_failed");
                                }
                                finish_error(
                                    &broker,
                                    queued.job_id,
                                    StructuredError::job(
                                        "sntp_start_failed",
                                        "SNTP synchronization could not be started",
                                    )
                                    .with_details(json!({"esp_error": error.code()})),
                                );
                            }
                        }
                    }
                }
                _ => unreachable!("network worker only dequeues network commands"),
            }
        }
        if station_connected && !wifi.is_connected()? {
            station_connected = false;
            sntp = None;
            next_reconnect = Instant::now();
            if let Ok(mut state) = broker.lock() {
                state.network.sta_ip = None;
                state.network.mode = "connecting";
            }
        }
        if !station_connected && Instant::now() >= next_reconnect {
            for profile in store.load_profiles()?.iter() {
                let credentials =
                    Credentials::new(profile.ssid(), profile.password(), next_generation)?;
                disconnect_sta(&mut wifi, &wifi_observation)?;
                wifi.set_configuration(&WifiConfiguration::Mixed(
                    client_configuration(&credentials)?,
                    ap.clone(),
                ))?;
                if matches!(
                    try_connect(&mut wifi, &credentials, &wifi_observation)?,
                    WifiConnectionOutcome::Connected
                ) {
                    store.promote_candidate(&credentials)?;
                    let address = wifi.sta_netif().get_ip_info()?.ip.to_string();
                    if let Ok(mut state) = broker.lock() {
                        state.set_network_connected(credentials.ssid.clone(), address);
                        state.set_saved_wifi(store.load_profiles()?.names());
                    }
                    wifi.set_configuration(&WifiConfiguration::Client(client_configuration(
                        &credentials,
                    )?))?;
                    station_connected = true;
                    let configuration = broker
                        .lock()
                        .map(|s| s.time_configuration())
                        .unwrap_or_default();
                    let _ = restart_sntp(&mut sntp, &configuration, &pending_sync, &broker);
                    break;
                }
            }
            next_reconnect = Instant::now() + Duration::from_secs(60);
        }
        thread::sleep(NETWORK_POLL);
    }
}

#[path = "server_web.rs"]
mod web;

fn start_http_server(
    broker: Arc<Mutex<Broker>>,
    storage: Arc<AssetStorage>,
    admin_store: Arc<Mutex<CredentialStore>>,
    main_screens: Arc<MainScreenController>,
    display: DisplayBroker,
) -> Result<EspHttpServer<'static>, esp_idf_svc::io::EspIOError> {
    let mut server = EspHttpServer::new(&HttpConfiguration {
        stack_size: HTTP_STACK_SIZE,
        max_sessions: 4,
        max_open_sockets: 4,
        session_timeout: Duration::from_secs(5),
        uri_match_wildcard: true,
        ..Default::default()
    })?;
    server.fn_handler("/", Method::Get, |request| {
        let mut response = request.into_response(
            200,
            Some("OK"),
            &[
                ("Content-Type", "text/html; charset=utf-8"),
                ("Content-Encoding", "gzip"),
                ("Cache-Control", "no-store"),
            ],
        )?;
        response.connection().write_all(INDEX)
    })?;
    web::register(&mut server, Arc::clone(&broker), Arc::clone(&storage))?;

    let admin_broker = Arc::clone(&broker);
    let admin_screens = Arc::clone(&main_screens);
    server.fn_handler("/admin/v1/token", Method::Post, move |mut request| {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct TokenUpdate {
            token: String,
        }

        if !request_same_origin(&request) {
            return write_json_response(request, 403, b"{\"error\":{\"code\":\"cross_origin\"}}");
        }
        if request
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            .is_none_or(|value| !value.trim().eq_ignore_ascii_case("application/json"))
        {
            return write_json_response(request, 415, b"{\"error\":{\"code\":\"content_type\"}}");
        }
        let supplied_token = request.header("X-Note4c-Token").map(str::to_owned);
        let length = content_length(&request);
        if length == 0 || length > ADMIN_TOKEN_MAX_LEN + 32 {
            return write_json_response(
                request,
                400,
                b"{\"error\":{\"code\":\"invalid_admin_token\"}}",
            );
        }
        let body = match read_request_body(&mut request, length) {
            Ok(body) => body,
            Err(_) => {
                return write_json_response(
                    request,
                    400,
                    b"{\"error\":{\"code\":\"invalid_admin_token\"}}",
                )
            }
        };
        let update: TokenUpdate = match serde_json::from_slice(&body) {
            Ok(update) => update,
            Err(_) => {
                return write_json_response(
                    request,
                    400,
                    b"{\"error\":{\"code\":\"invalid_admin_token\"}}",
                )
            }
        };
        if validate_admin_token(&update.token).is_err() {
            return write_json_response(
                request,
                400,
                b"{\"error\":{\"code\":\"invalid_admin_token\"}}",
            );
        }

        let mut broker = match admin_broker.lock() {
            Ok(broker) => broker,
            Err(_) => {
                return write_json_response(request, 500, b"{\"error\":{\"code\":\"internal\"}}")
            }
        };
        if !broker.is_authorized(supplied_token.as_deref()) {
            return write_json_response(request, 401, b"{\"error\":{\"code\":\"unauthorized\"}}");
        }
        let store = match admin_store.lock() {
            Ok(store) => store,
            Err(_) => {
                return write_json_response(request, 500, b"{\"error\":{\"code\":\"internal\"}}")
            }
        };
        if store.save_admin_custom_token(&update.token).is_err()
            || broker.replace_admin_token(update.token.clone()).is_err()
        {
            return write_json_response(
                request,
                500,
                b"{\"error\":{\"code\":\"admin_token_store_failed\"}}",
            );
        }
        // Keep the Broker lock until the panel's canonical QR state is
        // updated. It is the serialization point for token rotations, so a
        // later authorized request cannot overtake this screen update and
        // then be overwritten by an older token.
        drop(store);
        admin_screens.update_admin_token(update.token);
        drop(broker);
        write_json_response(request, 200, b"{\"updated\":true}")
    })?;

    let package_broker = Arc::clone(&broker);
    let package_storage = Arc::clone(&storage);
    server.fn_handler("/packages/v1", Method::Post, move |mut request| {
        let content_type_ok = request
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(),
                "application/vnd.note4c.widget+zip" | "application/zip" | "application/octet-stream"));
        if !content_type_ok {
            return write_json_response(request, 415, b"{\"error\":{\"code\":\"n4w_content_type\",\"message\":\"binary .pnw content type required\"}}");
        }
        if !request_same_origin(&request) {
            return write_json_response(request, 403, b"{\"error\":{\"code\":\"cross_origin\",\"message\":\"cross-origin request denied\"}}");
        }
        let token = request.header("X-Note4c-Token").map(str::to_owned);
        if !broker_authorized(&package_broker, token.as_deref()) {
            return write_json_response(request, 401, b"{\"error\":{\"code\":\"unauthorized\",\"message\":\"unauthorized\"}}");
        }
        if package_storage.status() != StorageStatus::Ready {
            return write_json_response(request, 409, b"{\"error\":{\"code\":\"storage_not_initialized\",\"message\":\"initialize assets storage first\"}}");
        }
        let content_length = content_length(&request);
        if content_length == 0 || content_length > panelet_widget_package::MAX_PACKAGE_BYTES {
            return write_json_response(request, 413, b"{\"error\":{\"code\":\"n4w_size\",\"message\":\"invalid .pnw content length\"}}");
        }
        let body = match read_request_body(&mut request, content_length) {
            Ok(body) => body,
            Err(_) => return write_json_response(request, 400, b"{\"error\":{\"code\":\"n4w_truncated\",\"message\":\"truncated .pnw body\"}}"),
        };
        let candidate = match panelet_widget_package::decode_pnw(&body) {
            Ok(candidate) => candidate,
            Err(error) => {
                let body = serde_json::to_vec(&json!({"error":{"code":"n4w_invalid","message":error.to_string()}})).unwrap_or_default();
                return write_json_response(request, 400, &body);
            }
        };
        let result = package_broker
            .lock()
            .map(|mut broker| broker.enqueue_widget_candidate_job(candidate));
        match result {
            Ok(Ok(job_id)) => {
                let body = serde_json::to_vec(&json!({"job_id":job_id,"state":"queued"}))
                    .unwrap_or_default();
                write_json_response(request, 202, &body)
            }
            Ok(Err(failure)) => {
                let error = failure.error;
                let body = serde_json::to_vec(&json!({"error":{
                    "kind":error.kind.as_str(),
                    "code":error.code,
                    "message":error.message,
                    "details":error.details,
                }}))
                .unwrap_or_default();
                write_json_response(request, failure.http_status, &body)
            }
            Err(_) => write_json_response(request, 500, b"{\"error\":{\"kind\":\"job\",\"code\":\"internal\",\"message\":\"broker unavailable\"}}"),
        }
    })?;

    let initialize_broker = Arc::clone(&broker);
    let initialize_storage = Arc::clone(&storage);
    server.fn_handler("/storage/v1/initialize", Method::Post, move |mut request| {
        if !request_same_origin(&request) {
            return write_json_response(request, 403, b"{\"error\":{\"code\":\"cross_origin\",\"message\":\"cross-origin request denied\"}}");
        }
        let token = request.header("X-Note4c-Token").map(str::to_owned);
        if !broker_authorized(&initialize_broker, token.as_deref()) {
            return write_json_response(request, 401, b"{\"error\":{\"code\":\"unauthorized\",\"message\":\"unauthorized\"}}");
        }
        if initialize_storage.status() == StorageStatus::Ready {
            return write_json_response(request, 409, b"{\"error\":{\"code\":\"storage_already_initialized\",\"message\":\"online reformat is forbidden\"}}");
        }
        let length = content_length(&request);
        if length == 0 || length > 128 {
            return write_json_response(request, 400, b"{\"error\":{\"code\":\"confirmation_required\",\"message\":\"exact erase confirmation required\"}}");
        }
        let body = match read_request_body(&mut request, length) {
            Ok(body) => body,
            Err(_) => return write_json_response(request, 400, b"{\"error\":{\"code\":\"confirmation_required\",\"message\":\"exact erase confirmation required\"}}"),
        };
        let confirmed = serde_json::from_slice::<serde_json::Value>(&body).ok()
            .and_then(|value| value.get("confirm").and_then(|value| value.as_str()).map(str::to_owned))
            .is_some_and(|value| value == "ERASE ASSETS");
        if !confirmed {
            return write_json_response(request, 400, b"{\"error\":{\"code\":\"confirmation_required\",\"message\":\"confirm must equal ERASE ASSETS\"}}");
        }
        match initialize_storage.initialize() {
            Ok(()) => {
                if let Ok(mut broker) = initialize_broker.lock() {
                    broker.set_storage_ready(true);
                }
                write_json_response(request, 200, b"{\"initialized\":true,\"storage\":\"assets_spiffs\",\"persistent\":true}")
            },
            Err(error) => {
                let body = serde_json::to_vec(&json!({"error":{"code":"storage_initialize_failed","message":error.to_string()}})).unwrap_or_default();
                write_json_response(request, 500, &body)
            }
        }
    })?;

    let shared_write_broker = Arc::clone(&broker);
    let shared_write_storage = Arc::clone(&storage);
    server.fn_handler("/shared/v1", Method::Post, move |mut request| {
        if !request_same_origin(&request) {
            return write_json_response(request, 403, b"{\"error\":{\"code\":\"cross_origin\"}}");
        }
        let token = request.header("X-Note4c-Token").map(str::to_owned);
        if !broker_authorized(&shared_write_broker, token.as_deref()) {
            return write_json_response(request, 401, b"{\"error\":{\"code\":\"unauthorized\"}}");
        }
        let Some(path) = request.header("X-Note4c-Path").map(str::to_owned) else {
            return write_json_response(request, 400, b"{\"error\":{\"code\":\"path_required\"}}");
        };
        let length = content_length(&request);
        if length == 0 || length > panelet_widget_store::SHARED_FILE_ITEM_MAX {
            return write_json_response(
                request,
                413,
                b"{\"error\":{\"code\":\"shared_file_size\"}}",
            );
        }
        let body = match read_request_body(&mut request, length) {
            Ok(body) if body.len() == length => body,
            _ => {
                return write_json_response(
                    request,
                    400,
                    b"{\"error\":{\"code\":\"shared_file_truncated\"}}",
                )
            }
        };
        match shared_write_storage.with_store(|store| store.shared_write_bytes(&path, &body)) {
            Ok(()) => {
                let response = serde_json::to_vec(
                    &json!({"stored":true,"path":path,"bytes":body.len(),"persistent":true}),
                )
                .unwrap_or_default();
                write_json_response(request, 200, &response)
            }
            Err(error) => {
                let response = serde_json::to_vec(
                    &json!({"error":{"code":"shared_write_failed","message":error.to_string()}}),
                )
                .unwrap_or_default();
                write_json_response(request, 400, &response)
            }
        }
    })?;

    let shared_read_broker = Arc::clone(&broker);
    let shared_read_storage = Arc::clone(&storage);
    server.fn_handler("/shared/v1", Method::Get, move |request| {
        if !request_same_origin(&request) {
            return write_json_response(request, 403, b"{\"error\":{\"code\":\"cross_origin\"}}");
        }
        let token = request.header("X-Note4c-Token").map(str::to_owned);
        if !broker_authorized(&shared_read_broker, token.as_deref()) {
            return write_json_response(request, 401, b"{\"error\":{\"code\":\"unauthorized\"}}");
        }
        if let Some(path) = request.header("X-Note4c-Path").map(str::to_owned) {
            match shared_read_storage.with_store(|store| store.shared_read_bytes(&path)) {
                Ok(Some(bytes)) => {
                    let mut response = request.into_response(
                        200,
                        None,
                        &[
                            ("Content-Type", "application/octet-stream"),
                            ("Cache-Control", "no-store"),
                            ("X-Content-Type-Options", "nosniff"),
                        ],
                    )?;
                    response.connection().write_all(&bytes)
                }
                Ok(None) => write_json_response(
                    request,
                    404,
                    b"{\"error\":{\"code\":\"shared_not_found\"}}",
                ),
                Err(error) => {
                    let response = serde_json::to_vec(
                        &json!({"error":{"code":"shared_read_failed","message":error.to_string()}}),
                    )
                    .unwrap_or_default();
                    write_json_response(request, 400, &response)
                }
            }
        } else {
            match shared_read_storage.with_store(|store| store.shared_list("")) {
                Ok(paths) => {
                    let response = serde_json::to_vec(&json!({"paths":paths,"persistent":true}))
                        .unwrap_or_default();
                    write_json_response(request, 200, &response)
                }
                Err(error) => {
                    let response = serde_json::to_vec(
                        &json!({"error":{"code":"shared_list_failed","message":error.to_string()}}),
                    )
                    .unwrap_or_default();
                    write_json_response(request, 400, &response)
                }
            }
        }
    })?;

    let shared_delete_broker = Arc::clone(&broker);
    let shared_delete_storage = Arc::clone(&storage);
    server.fn_handler("/shared/v1", Method::Delete, move |request| {
        if !request_same_origin(&request) {
            return write_json_response(request, 403, b"{\"error\":{\"code\":\"cross_origin\"}}");
        }
        let token = request.header("X-Note4c-Token").map(str::to_owned);
        if !broker_authorized(&shared_delete_broker, token.as_deref()) {
            return write_json_response(request, 401, b"{\"error\":{\"code\":\"unauthorized\"}}");
        }
        let Some(path) = request.header("X-Note4c-Path").map(str::to_owned) else {
            return write_json_response(request, 400, b"{\"error\":{\"code\":\"path_required\"}}");
        };
        match shared_delete_storage.with_store(|store| store.shared_delete(&path)) {
            Ok(()) => {
                let response =
                    serde_json::to_vec(&json!({"deleted":true,"path":path,"persistent":true}))
                        .unwrap_or_default();
                write_json_response(request, 200, &response)
            }
            Err(error) => {
                let response = serde_json::to_vec(
                    &json!({"error":{"code":"shared_delete_failed","message":error.to_string()}}),
                )
                .unwrap_or_default();
                write_json_response(request, 400, &response)
            }
        }
    })?;

    let rpc_broker = Arc::clone(&broker);
    server.fn_handler("/rpc/v1", Method::Post, move |mut request| {
        let content_type_ok = request
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
        if !content_type_ok {
            let mut response = request.into_response(
                415,
                Some("Unsupported Media Type"),
                &[
                    ("Content-Type", "application/json"),
                    ("Cache-Control", "no-store"),
                ],
            )?;
            return response.connection().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32600,\"message\":\"application/json required\"}}");
        }
        let same_origin = match (request.header("Origin"), request.header("Host")) {
            (None, _) => true,
            (Some(origin), Some(host)) => origin == format!("http://{host}"),
            (Some(_), None) => false,
        };
        if !same_origin {
            let mut response = request.into_response(
                403,
                Some("Forbidden"),
                &[
                    ("Content-Type", "application/json"),
                    ("Cache-Control", "no-store"),
                ],
            )?;
            return response.connection().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32001,\"message\":\"cross-origin request denied\"}}");
        }
        let content_length = request
            .header("Content-Length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        if content_length == 0 || content_length > RPC_BODY_MAX {
            let mut response = request.into_response(
                413,
                Some("Payload Too Large"),
                &[
                    ("Content-Type", "application/json"),
                    ("Cache-Control", "no-store"),
                ],
            )?;
            return response
                .connection()
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32600,\"message\":\"invalid content length\"}}");
        }
        let token = request.header("X-Note4c-Token").map(str::to_owned);
        let authorized = rpc_broker
            .lock()
            .map(|broker| broker.is_authorized(token.as_deref()))
            .unwrap_or(false);
        if !authorized {
            let mut response = request.into_response(
                401,
                Some("Unauthorized"),
                &[
                    ("Content-Type", "application/json"),
                    ("Cache-Control", "no-store"),
                ],
            )?;
            return response.connection().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32001,\"message\":\"unauthorized\"}}");
        }
        let mut body = Vec::new();
        if body.try_reserve_exact(content_length).is_err() {
            let mut response = request.into_response(
                503,
                Some("Service Unavailable"),
                &[
                    ("Content-Type", "application/json"),
                    ("Cache-Control", "no-store"),
                ],
            )?;
            return response.connection().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"request buffer unavailable\"}}");
        }
        body.resize(content_length, 0);
        let mut offset = 0;
        while offset < body.len() {
            let read = request.read(&mut body[offset..])?;
            if read == 0 {
                break;
            }
            offset += read;
        }
        if offset != body.len() {
            body.truncate(offset);
        }
        let reply = rpc_broker
            .lock()
            .map(|mut broker| {sync_presentation(&mut broker,&display);broker.dispatch(&body, token.as_deref())})
            .unwrap_or_else(|_| panelet_management::RpcReply {
                http_status: 500,
                body: "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"internal error\"}}".into(),
            });
        let mut response = request.into_response(
            reply.http_status,
            None,
            &[
                ("Content-Type", "application/json"),
                ("Cache-Control", "no-store"),
                ("X-Content-Type-Options", "nosniff"),
            ],
        )?;
        response
            .connection()
            .write_all(reply.body.as_bytes())
    })?;
    Ok(server)
}

fn request_same_origin<C: ServerConnection>(request: &ServerRequest<C>) -> bool {
    match (request.header("Origin"), request.header("Host")) {
        (None, _) => true,
        (Some(origin), Some(host)) => origin == format!("http://{host}"),
        (Some(_), None) => false,
    }
}

fn content_length<C: ServerConnection>(request: &ServerRequest<C>) -> usize {
    request
        .header("Content-Length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

fn broker_authorized(broker: &Arc<Mutex<Broker>>, token: Option<&str>) -> bool {
    broker
        .lock()
        .map(|broker| broker.is_authorized(token))
        .unwrap_or(false)
}

fn read_request_body<C: ServerConnection>(
    request: &mut ServerRequest<C>,
    length: usize,
) -> Result<Vec<u8>, C::Error> {
    let mut body = Vec::new();
    if body.try_reserve_exact(length).is_err() {
        // The concrete HTTP error type has no portable constructor. Returning
        // an empty body lets the caller report a bounded decode/truncation
        // failure without attempting another allocation.
        return Ok(body);
    }
    body.resize(length, 0);
    let mut offset = 0;
    while offset < length {
        let read = request.read(&mut body[offset..])?;
        if read == 0 {
            break;
        }
        offset += read;
    }
    body.truncate(offset);
    Ok(body)
}

fn write_json_response<C: ServerConnection>(
    request: ServerRequest<C>,
    status: u16,
    body: &[u8],
) -> Result<(), C::Error> {
    let mut response = request.into_response(
        status,
        None,
        &[
            ("Content-Type", "application/json"),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
        ],
    )?;
    response.connection().write_all(body)
}

fn ap_configuration(
    broker: &Arc<Mutex<Broker>>,
    password: &str,
) -> Result<AccessPointConfiguration, EspError> {
    let ssid = broker
        .lock()
        .map(|state| state.network.ap_ssid.clone())
        .unwrap_or_else(|_| "NOTE4C-ERROR".into());
    Ok(AccessPointConfiguration {
        ssid: ssid.as_str().try_into().map_err(|_| {
            EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_ARG }>()
        })?,
        ssid_hidden: false,
        channel: 6,
        auth_method: AuthMethod::WPA2Personal,
        password: password.try_into().map_err(|_| {
            EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_ARG }>()
        })?,
        max_connections: 4,
        ..Default::default()
    })
}

fn client_configuration(credentials: &Credentials) -> Result<ClientConfiguration, EspError> {
    let secured = !credentials.password.is_empty();
    Ok(ClientConfiguration {
        ssid: credentials.ssid.as_str().try_into().map_err(|_| {
            EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_ARG }>()
        })?,
        password: credentials.password.as_str().try_into().map_err(|_| {
            EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_ARG }>()
        })?,
        auth_method: if secured {
            // This device must interoperate with a legacy WPA-Personal IoT
            // network whose peer cannot complete WPA2 handshakes. ESP-IDF
            // treats this field as the minimum accepted scan auth mode, so a
            // WPA threshold still admits WPA2/WPA3 Personal APs.
            AuthMethod::WPA
        } else {
            AuthMethod::None
        },
        // Advertise optional Protected Management Frame support so WPA2 APs
        // keep working while WPA3/PMF-capable personal networks can negotiate
        // it. Requiring PMF remains an AP/user policy decision.
        pmf_cfg: if secured {
            PmfConfiguration::Capable { required: false }
        } else {
            PmfConfiguration::NotCapable
        },
        ..Default::default()
    })
}

impl WifiConnectionFailure {
    fn from_disconnect(disconnect: WifiDisconnectObservation) -> Self {
        let code = classify_wifi_disconnect_reason(disconnect.reason);
        let message = match code {
            "wifi_ap_not_found" => "Wi-Fi access point was not found",
            "wifi_authentication_timeout" => "Wi-Fi authentication response timed out",
            "wifi_authentication_failed" => "Wi-Fi authentication or key exchange failed",
            "wifi_security_incompatible" => "Wi-Fi access point security is incompatible",
            "wifi_association_failed" => "Wi-Fi association failed",
            "wifi_signal_too_weak" => "Wi-Fi signal did not meet the connection threshold",
            "wifi_beacon_timeout" => "Wi-Fi beacon timed out",
            _ => "Wi-Fi connection failed",
        };
        Self {
            code,
            message,
            disconnect: Some(disconnect),
        }
    }

    fn timeout(associated: bool) -> Self {
        if associated {
            Self {
                code: "wifi_dhcp_timeout",
                message: "Wi-Fi associated but did not obtain an IP address",
                disconnect: None,
            }
        } else {
            Self {
                code: "wifi_connection_timeout",
                message: "Wi-Fi connection attempt timed out without a disconnect reason",
                disconnect: None,
            }
        }
    }

    fn structured_error(self) -> StructuredError {
        let details = match self.disconnect {
            Some(disconnect) => {
                json!({"wifi_reason": disconnect.reason, "rssi": disconnect.rssi})
            }
            None => json!({"timeout_ms": CONNECT_TIMEOUT.as_millis()}),
        };
        StructuredError::job(self.code, self.message).with_details(details)
    }
}

fn try_connect(
    wifi: &mut EspWifi<'static>,
    credentials: &Credentials,
    observation: &Mutex<WifiAttemptObservation>,
) -> Result<WifiConnectionOutcome, EspError> {
    if let Ok(mut observation) = observation.lock() {
        observation.expected_ssid = Some(credentials.ssid.clone());
        observation.associated = false;
        observation.disconnect = None;
    }

    let outcome = (|| {
        wifi.connect()?;
        let started = Instant::now();
        loop {
            let (associated, disconnect) = observation
                .lock()
                .map(|observation| (observation.associated, observation.disconnect))
                .unwrap_or((false, None));
            // A matching disconnect wins over IP state. This prevents a stale
            // address from a previous connection from looking like success.
            if let Some(disconnect) = disconnect {
                return Ok(WifiConnectionOutcome::Rejected(
                    WifiConnectionFailure::from_disconnect(disconnect),
                ));
            }
            // `associated` can only be set by a fresh StaConnected event whose
            // SSID matches this attempt. Require both that event and a usable
            // IPv4 address before committing credentials.
            if associated && wifi.driver().is_sta_connected()? && wifi.sta_netif().is_up()? {
                return Ok(WifiConnectionOutcome::Connected);
            }
            if started.elapsed() >= CONNECT_TIMEOUT {
                return Ok(WifiConnectionOutcome::Rejected(
                    WifiConnectionFailure::timeout(associated),
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    })();

    clear_wifi_attempt_observation(observation);
    let requires_cancel = match &outcome {
        Ok(WifiConnectionOutcome::Connected) => false,
        // A matching disconnect event is already terminal and was delivered
        // after the driver's status/netif handlers. Avoid letting an
        // unnecessary cleanup error overwrite this precise reason.
        Ok(WifiConnectionOutcome::Rejected(failure)) => failure.disconnect.is_none(),
        Err(_) => true,
    };
    if requires_cancel {
        // `esp_wifi_connect` is asynchronous. An expired or indeterminate
        // attempt must be explicitly cancelled or it could connect after the
        // job has failed and its candidate credentials were invalidated.
        disconnect_sta(wifi, observation)?;
    }
    if let Ok(WifiConnectionOutcome::Rejected(failure)) = &outcome {
        if let Some(disconnect) = failure.disconnect {
            log::warn!(
                "STA connection failed: code={} reason={} rssi={}",
                failure.code,
                disconnect.reason,
                disconnect.rssi
            );
        } else {
            log::warn!("STA connection failed: code={}", failure.code);
        }
    }
    outcome
}

fn clear_wifi_attempt_observation(observation: &Mutex<WifiAttemptObservation>) {
    if let Ok(mut observation) = observation.lock() {
        observation.expected_ssid = None;
        observation.associated = false;
        observation.disconnect = None;
    }
}

fn disconnect_sta(
    wifi: &mut EspWifi<'static>,
    observation: &Mutex<WifiAttemptObservation>,
) -> Result<(), EspError> {
    // Disarm diagnostics before the deliberate disconnect so its event cannot
    // be attributed to the next candidate attempt.
    clear_wifi_attempt_observation(observation);
    match wifi.disconnect() {
        Ok(()) => {}
        Err(error) if error.code() == esp_idf_svc::sys::ESP_ERR_WIFI_NOT_CONNECT => {}
        Err(error) => return Err(error),
    }

    let started = Instant::now();
    loop {
        if !wifi.driver().is_sta_connected()? && !wifi.sta_netif().is_up()? {
            return Ok(());
        }
        if started.elapsed() >= DISCONNECT_TIMEOUT {
            return Err(EspError::from_infallible::<
                { esp_idf_svc::sys::ESP_ERR_TIMEOUT },
            >());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn connected(
    broker: &Arc<Mutex<Broker>>,
    main_screens: &MainScreenController,
    wifi: &EspWifi<'static>,
    credentials: &Credentials,
) {
    if let Ok(info) = wifi.sta_netif().get_ip_info() {
        let address = info.ip.to_string();
        if let Ok(mut broker) = broker.lock() {
            broker.set_network_connected(credentials.ssid.clone(), address.clone());
        }
        main_screens.show_provisioned(credentials.ssid.clone(), address);
    }
}

fn mark_provisioning(
    broker: &Arc<Mutex<Broker>>,
    main_screens: &MainScreenController,
    detail: &'static str,
) {
    if let Ok(mut broker) = broker.lock() {
        broker.set_network_provisioning(Some(detail.into()));
    }
    main_screens.show_provisioning();
}

fn finish(broker: &Arc<Mutex<Broker>>, job_id: u32, succeeded: bool, result: serde_json::Value) {
    if let Ok(mut broker) = broker.lock() {
        broker.finish_job(job_id, succeeded, result);
    }
}

fn finish_error(broker: &Arc<Mutex<Broker>>, job_id: u32, error: StructuredError) {
    if let Ok(mut broker) = broker.lock() {
        broker.finish_job_with_error(job_id, error);
    }
}

#[derive(Clone)]
struct Credentials {
    ssid: String,
    password: String,
    generation: u32,
}

impl Credentials {
    fn new(ssid: &str, password: &str, generation: u32) -> Result<Self, EspError> {
        if ssid.is_empty()
            || ssid.len() > 32
            || !(password.is_empty() || (8..=63).contains(&password.len()))
        {
            return Err(EspError::from_infallible::<
                { esp_idf_svc::sys::ESP_ERR_INVALID_ARG },
            >());
        }
        Ok(Self {
            ssid: ssid.into(),
            password: password.into(),
            generation,
        })
    }
}

fn profile_store_error() -> EspError {
    EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_STATE }>()
}

struct CredentialStore(EspDefaultNvs);

impl CredentialStore {
    fn new(partition: EspDefaultNvsPartition) -> Result<Self, EspError> {
        Ok(Self(EspNvs::new(partition, NVS_NAMESPACE, true)?))
    }

    fn load_profiles(&self) -> Result<WifiProfiles, EspError> {
        if let Some(length) = self.0.blob_len("profiles")? {
            if length > WIFI_PROFILES_MAX_BYTES {
                return Err(profile_store_error());
            }
            let mut bytes = vec![0; length];
            let bytes = self
                .0
                .get_blob("profiles", &mut bytes)?
                .ok_or_else(profile_store_error)?;
            return WifiProfiles::decode(bytes).map_err(|_| profile_store_error());
        }
        let mut profiles = WifiProfiles::default();
        if let Some(old) = self.load_legacy_active()? {
            profiles
                .remember(
                    panelet_management::WifiCredentials::new(old.ssid, old.password)
                        .map_err(|_| profile_store_error())?,
                )
                .map_err(|_| profile_store_error())?;
        }
        Ok(profiles)
    }

    fn save_profiles(&self, profiles: &WifiProfiles) -> Result<(), EspError> {
        let bytes = profiles.encode().map_err(|_| profile_store_error())?;
        self.0.set_blob("profiles", &bytes)?;
        // The new blob is authoritative before removing legacy plaintext keys.
        self.0.remove("ssid")?;
        self.0.remove("pass")?;
        self.0.remove("valid")?;
        Ok(())
    }

    fn load_active(&self) -> Result<Option<Credentials>, EspError> {
        self.load_profiles()?
            .iter()
            .next()
            .map(|profile| Credentials::new(profile.ssid(), profile.password(), 1))
            .transpose()
    }

    fn load_legacy_active(&self) -> Result<Option<Credentials>, EspError> {
        if self.0.get_u8("valid")? != Some(1) || self.0.get_u8("schema")? != Some(CREDENTIAL_SCHEMA)
        {
            return Ok(None);
        }
        let generation = self.0.get_u32("gen")?.unwrap_or(0);
        let mut ssid_buffer = [0; 33];
        let mut password_buffer = [0; 64];
        let Some(ssid) = self.0.get_str("ssid", &mut ssid_buffer)? else {
            return Ok(None);
        };
        let Some(password) = self.0.get_str("pass", &mut password_buffer)? else {
            return Ok(None);
        };
        let credentials = Credentials {
            ssid: ssid.into(),
            password: password.into(),
            generation,
        };
        if self.0.get_u32("check")? != Some(checksum(&credentials)) {
            return Ok(None);
        }
        Ok(Some(credentials))
    }

    fn save_candidate(&self, credentials: &Credentials) -> Result<(), EspError> {
        self.0.set_u8("c_valid", 0)?;
        self.0.set_u8("c_schema", CREDENTIAL_SCHEMA)?;
        self.0.set_u32("c_gen", credentials.generation)?;
        self.0.set_str("c_ssid", &credentials.ssid)?;
        self.0.set_str("c_pass", &credentials.password)?;
        self.0.set_u32("c_check", checksum(credentials))?;
        self.0.set_u8("c_valid", 1)
    }

    fn promote_candidate(&self, credentials: &Credentials) -> Result<(), EspError> {
        let mut profiles = self.load_profiles()?;
        profiles
            .remember(
                panelet_management::WifiCredentials::new(
                    credentials.ssid.clone(),
                    credentials.password.clone(),
                )
                .map_err(|_| profile_store_error())?,
            )
            .map_err(|_| profile_store_error())?;
        self.save_profiles(&profiles)?;
        self.clear_candidate()
    }

    fn clear_candidate(&self) -> Result<(), EspError> {
        self.0.set_u8("c_valid", 0)
    }

    fn load_time_configuration(&self) -> Result<Option<TimeConfiguration>, EspError> {
        if self.0.get_u8("t_valid")? != Some(1)
            || self.0.get_u8("t_ver")? != Some(TIME_CONFIGURATION_SCHEMA)
        {
            return Ok(None);
        }
        let mut server_buffer = [0_u8; NTP_SERVER_MAX_LEN + 1];
        let Some(server) = self.0.get_str("ntp", &mut server_buffer)? else {
            return Ok(None);
        };
        let Some(offset) = self.0.get_i16("tz_min")? else {
            return Ok(None);
        };
        Ok(TimeConfiguration::new(server.to_owned(), offset).ok())
    }

    fn save_time_configuration(&self, configuration: &TimeConfiguration) -> Result<(), EspError> {
        self.0.set_u8("t_valid", 0)?;
        self.0.set_u8("t_ver", TIME_CONFIGURATION_SCHEMA)?;
        self.0.set_str("ntp", configuration.ntp_server())?;
        self.0
            .set_i16("tz_min", configuration.utc_offset_minutes())?;
        self.0.set_u8("t_valid", 1)
    }

    fn rtc_owned(&self) -> Result<bool, EspError> {
        Ok(self.0.get_u8("rtc_ver")? == Some(RTC_OWNERSHIP_SCHEMA))
    }

    fn set_rtc_owned(&self) -> Result<(), EspError> {
        self.0.set_u8("rtc_ver", RTC_OWNERSHIP_SCHEMA)
    }

    fn load_admin_custom_token(&self) -> Result<Option<String>, EspError> {
        if self.0.get_u8("a_valid")? != Some(1) || self.0.get_u8("a_ver")? != Some(1) {
            return Ok(None);
        }
        let mut buffer = [0; 33];
        Ok(self
            .0
            .get_str("admin_custom", &mut buffer)?
            .filter(|token| validate_admin_token(token).is_ok())
            .map(str::to_owned))
    }

    fn save_admin_custom_token(&self, token: &str) -> Result<(), EspError> {
        if validate_admin_token(token).is_err() {
            return Err(EspError::from_infallible::<
                { esp_idf_svc::sys::ESP_ERR_INVALID_ARG },
            >());
        }
        self.0.set_u8("a_valid", 0)?;
        self.0.set_u8("a_ver", 1)?;
        self.0.set_str("admin_custom", token)?;
        self.0.set_u8("a_valid", 1)
    }
}

fn checksum(credentials: &Credentials) -> u32 {
    let mut value = 0x811c_9dc5_u32;
    for byte in CREDENTIAL_SCHEMA
        .to_le_bytes()
        .into_iter()
        .chain(credentials.generation.to_le_bytes())
        .chain(credentials.ssid.bytes())
        .chain(credentials.password.bytes())
    {
        value = (value ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    value
}

fn factory_identity() -> Result<[u8; 6], EspError> {
    let mut identity = [0_u8; 6];
    unsafe {
        EspError::convert(esp_idf_svc::sys::esp_efuse_mac_get_default(
            identity.as_mut_ptr(),
        ))?;
    }
    Ok(identity)
}

fn ap_ssid(factory_identity: &[u8; 6]) -> String {
    format!(
        "NOTE4C-{:02X}{:02X}",
        factory_identity[4], factory_identity[5]
    )
}
