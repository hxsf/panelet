// SPDX-License-Identifier: MPL-2.0

//! Real ABI 3 rendering with request-local, memory-only host services.
//! Used by the desktop editor and browser WASM preview. No hardware,
//! filesystem or network operation is performed by a widget preview.
use panelet_management::{
    compile_host_permissions, prepare_widget_candidate, HostPattern, WidgetCandidate,
    WidgetSettings,
};
use panelet_rune_abi_poc::clock::Instant;
use panelet_rune_abi_poc::display::DisplayBroker;
use panelet_rune_abi_poc::lifecycle::{
    ComponentEvent, ComponentSession, InputEvent, BOOT, CLICK, DOWN, LONG_PRESS, UP,
};
use panelet_rune_abi_poc::{
    context_with_os, HostError, HostFuture, HostIo, HostResult, HttpRequest, HttpResponse,
    LocalTimeReading, OsBindings, Std, StdConfig, HTTP_BODY_MAX_BYTES, HTTP_URL_MAX_BYTES,
};
use panelet_widget_store::{
    normalize_path, KV_KEYS_MAX, KV_TOTAL_MAX, PRIVATE_FILE_COUNT_MAX, PRIVATE_FILE_ITEM_MAX,
    PRIVATE_TOTAL_MAX, SHARED_FILE_COUNT_MAX, SHARED_FILE_ITEM_MAX, SHARED_TOTAL_MAX,
};
use rune::{Source, Sources, Vm};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

const COMPILE_MEMORY: usize = 1024 * 1024;
const EXECUTION_MEMORY: usize = 64 * 1024;
const INSTRUCTIONS: usize = 100_000;
const MAX_EVENTS: usize = 64;
const MAX_LOGS: usize = 64;
const MAX_FIXTURES: usize = 16;

fn empty_object() -> Value {
    json!({})
}
fn default_offset() -> i32 {
    480
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    package: WidgetCandidate,
    #[serde(default = "empty_object")]
    config: Value,
    #[serde(default)]
    host_permissions: Option<Vec<String>>,
    #[serde(default)]
    environment: Environment,
    #[serde(default)]
    events: Vec<Event>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Environment {
    #[serde(default)]
    unix_ms: Option<i64>,
    #[serde(default = "default_offset")]
    utc_offset_minutes: i32,
    #[serde(default)]
    http: Vec<Fixture>,
    #[serde(default)]
    shared_files: BTreeMap<String, String>,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            unix_ms: None,
            utc_offset_minutes: default_offset(),
            http: Vec::new(),
            shared_files: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    url: String,
    status: u16,
    body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    kind: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    advance_ms: u64,
}
impl Event {
    fn component(&self) -> HostResult<ComponentEvent> {
        if self.kind == "timer" {
            if self.key.is_some() {
                return Err(HostError::invalid_argument("timer does not take a key"));
            }
            return Ok(ComponentEvent::Timer);
        }
        let kind = match self.kind.as_str() {
            "click" => CLICK,
            "long_press" => LONG_PRESS,
            _ => {
                return Err(HostError::invalid_argument(
                    "event kind must be click, long_press or timer",
                ))
            }
        };
        let keycode = match self.key.as_deref() {
            Some("up") => UP,
            Some("down") => DOWN,
            Some("boot") => BOOT,
            _ => {
                return Err(HostError::invalid_argument(
                    "input key must be up, down or boot",
                ))
            }
        };
        Ok(ComponentEvent::Input(InputEvent {
            keycode,
            kind,
            held_ms: if kind == LONG_PRESS { 1_000 } else { 50 },
        }))
    }
}

#[derive(Default)]
struct MemoryFiles(Mutex<BTreeMap<String, String>>);
impl MemoryFiles {
    fn read(&self, path: &str) -> HostResult<Option<String>> {
        let path = normalize_path(path)
            .map_err(|_| HostError::invalid_argument("invalid storage path"))?;
        Ok(self.0.lock().unwrap().get(&path).cloned())
    }
    fn write(
        &self,
        path: String,
        text: String,
        count: usize,
        item: usize,
        total: usize,
    ) -> HostResult<()> {
        let path = normalize_path(&path)
            .map_err(|_| HostError::invalid_argument("invalid storage path"))?;
        let mut files = self.0.lock().unwrap();
        let old = files.get(&path).map_or(0, String::len);
        let size: usize = files.values().map(String::len).sum();
        if text.len() > item
            || (!files.contains_key(&path) && files.len() >= count)
            || size - old + text.len() > total
        {
            return Err(HostError::quota("preview storage quota exceeded"));
        }
        files.insert(path, text);
        Ok(())
    }
    fn delete(&self, path: &str) -> HostResult<()> {
        let path = normalize_path(path)
            .map_err(|_| HostError::invalid_argument("invalid storage path"))?;
        self.0.lock().unwrap().remove(&path);
        Ok(())
    }
    fn list(&self, prefix: &str) -> HostResult<Vec<String>> {
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            normalize_path(prefix)
                .map_err(|_| HostError::invalid_argument("invalid storage prefix"))?
        };
        Ok(self
            .0
            .lock()
            .unwrap()
            .keys()
            .filter(|path| path.starts_with(&prefix))
            .cloned()
            .collect())
    }
}

struct PreviewIo {
    resources: BTreeMap<String, String>,
    fixtures: Vec<Fixture>,
    permissions: Vec<HostPattern>,
    kv: Mutex<BTreeMap<String, String>>,
    private: MemoryFiles,
    shared: MemoryFiles,
    logs: Mutex<Vec<String>>,
}
fn ready<T: Send + 'static>(result: HostResult<T>) -> HostFuture<T> {
    Box::pin(std::future::ready(result))
}
impl HostIo for PreviewIo {
    fn http_fetch(&self, request: HttpRequest) -> HostFuture<HttpResponse> {
        // The ABI checks capabilities and unsafe URL classes before this call.
        // Match the same manifest/settings host patterns without resolving DNS:
        // responses come exclusively from an exact URL fixture.
        let allowed = self
            .permissions
            .iter()
            .any(|pattern| pattern.matches_url(&request.url).unwrap_or(false));
        if !allowed {
            return ready(Err(HostError::new(
                "host_permission_denied",
                "http",
                "URL does not match preview host permissions",
                false,
            )));
        }
        let result = match self
            .fixtures
            .iter()
            .find(|fixture| fixture.url == request.url)
        {
            Some(fixture) if fixture.body.len() > request.max_body_bytes => {
                Err(HostError::quota("HTTP fixture exceeds request body limit"))
            }
            Some(fixture) => Ok(HttpResponse::new(fixture.status, fixture.body.clone())),
            None => Err(HostError::new(
                "preview_http_fixture_missing",
                "http",
                "No matching HTTP fixture; preview does not access the network",
                false,
            )),
        };
        ready(result)
    }
    fn storage_has(&self, key: String) -> HostFuture<bool> {
        ready(Ok(self.kv.lock().unwrap().contains_key(&key)))
    }
    fn storage_get(&self, key: String) -> HostFuture<Option<String>> {
        ready(Ok(self.kv.lock().unwrap().get(&key).cloned()))
    }
    fn storage_set(&self, key: String, value: String) -> HostFuture<()> {
        let mut kv = self.kv.lock().unwrap();
        let old = kv.get(&key).map_or(0, |value| key.len() + value.len());
        let size: usize = kv.iter().map(|(key, value)| key.len() + value.len()).sum();
        if (!kv.contains_key(&key) && kv.len() >= KV_KEYS_MAX)
            || size - old + key.len() + value.len() > KV_TOTAL_MAX
        {
            return ready(Err(HostError::quota("preview KV quota exceeded")));
        }
        kv.insert(key, value);
        ready(Ok(()))
    }
    fn storage_delete(&self, key: String) -> HostFuture<()> {
        self.kv.lock().unwrap().remove(&key);
        ready(Ok(()))
    }
    fn resource_get(&self, path: String) -> HostFuture<Option<String>> {
        ready(Ok(self.resources.get(&path).cloned()))
    }
    fn resource_list(&self) -> HostFuture<Vec<String>> {
        ready(Ok(self.resources.keys().cloned().collect()))
    }
    fn fs_private_exists(&self, path: String) -> HostFuture<bool> {
        ready(self.private.read(&path).map(|value| value.is_some()))
    }
    fn fs_private_read_text(&self, path: String) -> HostFuture<Option<String>> {
        ready(self.private.read(&path))
    }
    fn fs_private_write_text(&self, path: String, text: String) -> HostFuture<()> {
        ready(self.private.write(
            path,
            text,
            PRIVATE_FILE_COUNT_MAX,
            PRIVATE_FILE_ITEM_MAX,
            PRIVATE_TOTAL_MAX,
        ))
    }
    fn fs_private_delete(&self, path: String) -> HostFuture<()> {
        ready(self.private.delete(&path))
    }
    fn fs_private_list(&self, prefix: String) -> HostFuture<Vec<String>> {
        ready(self.private.list(&prefix))
    }
    fn fs_shared_exists(&self, path: String) -> HostFuture<bool> {
        ready(self.shared.read(&path).map(|value| value.is_some()))
    }
    fn fs_shared_read_text(&self, path: String) -> HostFuture<Option<String>> {
        ready(self.shared.read(&path))
    }
    fn fs_shared_write_text(&self, path: String, text: String) -> HostFuture<()> {
        ready(self.shared.write(
            path,
            text,
            SHARED_FILE_COUNT_MAX,
            SHARED_FILE_ITEM_MAX,
            SHARED_TOTAL_MAX,
        ))
    }
    fn fs_shared_delete(&self, path: String) -> HostFuture<()> {
        ready(self.shared.delete(&path))
    }
    fn fs_shared_list(&self, prefix: String) -> HostFuture<Vec<String>> {
        ready(self.shared.list(&prefix))
    }
    fn log(&self, level: String, message: String) -> HostFuture<()> {
        let mut logs = self.logs.lock().unwrap();
        if logs.len() < MAX_LOGS {
            logs.push(format!("[{level}] {message}"));
        }
        ready(Ok(()))
    }
    // Sleep futures intentionally complete immediately; virtual time advances
    // only through event.advance_ms. Hardware services retain fail-closed defaults.
}

fn compile(source: &str, bindings: &OsBindings) -> HostResult<Vm> {
    let context = context_with_os(bindings)
        .map_err(|error| HostError::new("rune_context", "compile", error.to_string(), false))?;
    let runtime = context
        .runtime()
        .map_err(|error| HostError::new("rune_context", "compile", error.to_string(), false))?;
    let mut sources = Sources::new();
    sources
        .insert(
            Source::memory(source)
                .map_err(|_| HostError::quota("Rune source allocation failed"))?,
        )
        .map_err(|_| HostError::quota("Rune source allocation failed"))?;
    let mut diagnostics = rune::Diagnostics::without_warnings();
    let unit = rune::alloc::limit::with(COMPILE_MEMORY, || {
        rune::prepare(&mut sources)
            .with_context(&context)
            .with_diagnostics(&mut diagnostics)
            .build()
    })
    .call();
    let unit = unit.map_err(|_| {
        use rune::ast::Spanned;
        use rune::diagnostics::{Diagnostic, FatalDiagnosticKind};
        let mut messages = Vec::new();
        for diagnostic in diagnostics.diagnostics().iter().take(8) {
            if let Diagnostic::Fatal(error) = diagnostic {
                let message = if let FatalDiagnosticKind::CompileError(compile) = error.kind() {
                    let start = compile.span().start.into_usize();
                    let prefix = source.get(..start).unwrap_or("");
                    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
                    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
                    format!("main.rn:{line}:{column}: {error}")
                } else {
                    error.to_string()
                };
                messages.push(message);
            }
        }
        if messages.is_empty() {
            messages.push("Rune compilation failed or exceeded its 1 MiB budget".into());
        }
        HostError::new("rune_compile", "compile", messages.join("\n"), false)
    })?;
    Ok(Vm::new(Arc::new(runtime), Arc::new(unit)))
}

fn run<T>(future: impl Future<Output = HostResult<T>>) -> HostResult<T> {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    let started = Instant::now();
    loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return result;
        }
        if started.elapsed() >= Duration::from_secs(2) {
            return Err(HostError::timeout("preview event deadline exceeded"));
        }
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        std::thread::yield_now();
    }
}

fn local_time(unix_ms: Option<i64>, offset: i16) -> HostResult<Option<LocalTimeReading>> {
    let Some(unix_ms) = unix_ms else {
        return Ok(None);
    };
    let seconds = unix_ms
        .div_euclid(1000)
        .checked_add(i64::from(offset) * 60)
        .ok_or_else(|| HostError::invalid_argument("preview time overflow"))?;
    let civil = panelet_core::unix_seconds_to_civil(seconds).map_err(|_| {
        HostError::invalid_argument("preview time is outside the supported calendar range")
    })?;
    Ok(Some(LocalTimeReading {
        year: civil.year,
        month: civil.month,
        day: civil.day,
        weekday: civil.weekday,
        hour: civil.hour,
        minute: civil.minute,
        second: civil.second,
        utc_offset_minutes: offset,
        unix_ms,
    }))
}

fn capture(display: &DisplayBroker, pixels: &mut Option<Vec<u8>>) -> HostResult<()> {
    if let Some(presentation) = display.poll(u64::MAX) {
        *pixels = Some(presentation.frame.pixels().to_vec());
        // Only this isolated virtual broker is completed, never the device's
        // physical display broker. Unchanged frames retain the previous pixels.
        display
            .finish(presentation, Ok(()))
            .map_err(|_| HostError::invalid_argument("preview frame completion failed"))?;
    }
    Ok(())
}

fn execute(
    mut request: Request,
    pixels: &mut Option<Vec<u8>>,
    output_logs: &mut Vec<String>,
) -> HostResult<()> {
    request.package = prepare_widget_candidate(request.package)
        .map_err(|error| HostError::new(error.code, "package", error.message, false))?;
    let encoded = panelet_widget_package::encode_pnw(&request.package);
    let package = panelet_widget_package::decode_pnw(&encoded)
        .map_err(|error| HostError::new("package_invalid", "package", error.to_string(), false))?;
    let settings = WidgetSettings {
        values: request.config,
        host_permissions: request.host_permissions,
        ..WidgetSettings::default()
    };
    settings.validate().map_err(HostError::invalid_argument)?;
    if request.events.len() > MAX_EVENTS {
        return Err(HostError::quota("preview accepts at most 64 events"));
    }
    if !(-720..=840).contains(&request.environment.utc_offset_minutes) {
        return Err(HostError::invalid_argument(
            "UTC offset must be -720..840 minutes",
        ));
    }
    for event in &request.events {
        event.component()?;
        if event.advance_ms > 31 * 86_400_000 {
            return Err(HostError::invalid_argument(
                "each event can advance at most 31 days",
            ));
        }
    }
    let permissions = settings
        .host_permissions
        .unwrap_or(package.manifest.host_permissions);
    let patterns = compile_host_permissions(&permissions)
        .map_err(|error| HostError::new(error.code, "package", error.message, false))?;
    if request.environment.http.len() > MAX_FIXTURES {
        return Err(HostError::quota("preview accepts at most 16 HTTP fixtures"));
    }
    for (index, fixture) in request.environment.http.iter().enumerate() {
        if fixture.url.len() > HTTP_URL_MAX_BYTES
            || !(100..=599).contains(&fixture.status)
            || fixture.body.len() > HTTP_BODY_MAX_BYTES
        {
            return Err(HostError::invalid_argument(
                "invalid HTTP fixture URL, status or body size",
            ));
        }
        panelet_management::ParsedUrl::parse(&fixture.url)
            .map_err(|error| HostError::invalid_argument(error.message))?;
        if request.environment.http[..index]
            .iter()
            .any(|other| other.url == fixture.url)
        {
            return Err(HostError::invalid_argument("duplicate HTTP fixture URL"));
        }
    }
    let shared = MemoryFiles::default();
    for (path, text) in request.environment.shared_files {
        if normalize_path(&path).as_ref().ok() != Some(&path) {
            return Err(HostError::invalid_argument(
                "shared fixture paths must be normalized",
            ));
        }
        shared.write(
            path,
            text,
            SHARED_FILE_COUNT_MAX,
            SHARED_FILE_ITEM_MAX,
            SHARED_TOTAL_MAX,
        )?;
    }
    let offset = request.environment.utc_offset_minutes as i16;
    let mut unix_ms = request.environment.unix_ms;
    let mut uptime_ms = 0u64;
    let io = Arc::new(PreviewIo {
        resources: package
            .resources
            .into_iter()
            .map(|resource| (resource.path, resource.text))
            .collect(),
        fixtures: request.environment.http,
        permissions: patterns,
        kv: Mutex::new(BTreeMap::new()),
        private: MemoryFiles::default(),
        shared,
        logs: Mutex::new(Vec::new()),
    });
    let display = DisplayBroker::new();
    let bindings = OsBindings::new(display.clone(), display.epoch());
    let vm = compile(&package.source, &bindings)?;
    let mut session = ComponentSession::new(vm, bindings, EXECUTION_MEMORY, INSTRUCTIONS)?;
    let result = (|| {
        for event in std::iter::once(None).chain(request.events.iter().map(Some)) {
            let event = if let Some(event) = event {
                uptime_ms = uptime_ms
                    .checked_add(event.advance_ms)
                    .ok_or_else(|| HostError::invalid_argument("preview uptime overflow"))?;
                unix_ms = unix_ms
                    .map(|time| {
                        time.checked_add(event.advance_ms as i64)
                            .ok_or_else(|| HostError::invalid_argument("preview clock overflow"))
                    })
                    .transpose()?;
                event.component()?
            } else {
                ComponentEvent::Mount
            };
            let config = StdConfig {
                widget_config: settings.values.to_string(),
                uptime_ms,
                unix_ms,
                local_time: local_time(unix_ms, offset)?,
                capabilities: package.manifest.capabilities.clone(),
                host_permissions: permissions.clone(),
                wall_time_ms: 2_000,
                ..StdConfig::default()
            };
            let result = run(session.dispatch(Std::from_arc(io.clone(), config), event));
            capture(&display, pixels)?;
            result?;
        }
        Ok(())
    })();
    *output_logs = io.logs.lock().unwrap().clone();
    result
}

/// Render one request. Pixels are the device's exact MSB-first packed 2 bpp
/// framebuffer: black=0, white=1, yellow=2, red=3, four pixels per byte.
pub fn render(request: Value) -> Value {
    let mut pixels = None;
    let mut logs = Vec::new();
    let result = serde_json::from_value::<Request>(request)
        .map_err(|error| HostError::invalid_argument(error.to_string()))
        .and_then(|request| execute(request, &mut pixels, &mut logs));
    json!({ "ok": result.is_ok(), "width": 400, "height": 300, "pixels": pixels, "logs": logs, "error": result.err().map(|error| json!({"code": error.code, "message": error.message})) })
}

/// Package operations share the device's validator and the native editor's
/// container codec; browser code does not implement a second package format.
#[cfg(all(
    target_arch = "wasm32",
    target_os = "unknown",
    feature = "browser-preview"
))]
pub fn encode_package(input: &[u8]) -> Value {
    let result = serde_json::from_slice::<WidgetCandidate>(input)
        .map_err(|error| HostError::invalid_argument(error.to_string()))
        .and_then(|candidate| {
            prepare_widget_candidate(candidate)
                .map_err(|error| HostError::new(error.code, "package", error.message, false))
        })
        .and_then(|candidate| {
            let bytes = panelet_widget_package::encode_pnw(&candidate);
            panelet_widget_package::decode_pnw(&bytes).map_err(|error| {
                HostError::new("package_invalid", "package", error.to_string(), false)
            })?;
            Ok(bytes)
        });
    match result {
        Ok(bytes) => json!({"ok": true, "bytes": bytes}),
        Err(error) => json!({"ok": false, "error": {"code": error.code, "message": error.message}}),
    }
}

#[cfg(all(
    target_arch = "wasm32",
    target_os = "unknown",
    feature = "browser-preview"
))]
pub fn decode_package(input: &[u8]) -> Value {
    match panelet_widget_package::decode_pnw(input) {
        Ok(package) => json!({"ok": true, "package": package}),
        Err(error) => {
            json!({"ok": false, "error": {"code": "package_invalid", "message": error.to_string()}})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_request(source: &str, capabilities: &[&str]) -> Value {
        json!({"package":{"manifest":{"id":"test.preview","name":"Preview","version":"1","abi":"note4c.rune/3","capabilities":capabilities,"host_permissions":[]},"source":source,"resources":[]}})
    }
    fn component(hooks: &str) -> String {
        format!("struct State {{}} #[widget::component] impl State {{ pub fn new() {{ State {{}} }} {hooks} }}")
    }
    fn counter() -> Value {
        source_request(
            include_str!("../../../../examples/widgets/counter/main.rn"),
            &["screen.present"],
        )
    }
    fn pixels(value: &Value) -> Vec<u8> {
        assert_eq!(value["ok"], true, "{value}");
        let pixels: Vec<u8> = serde_json::from_value(value["pixels"].clone()).unwrap();
        assert_eq!(pixels.len(), 30_000);
        pixels
    }

    #[test]
    fn counter_replays_inputs_retains_state_and_starts_fresh_per_request() {
        let first = pixels(&render(counter()));
        let mut request = counter();
        request["events"] = json!([{"kind":"click","key":"boot"}]);
        let one = pixels(&render(request.clone()));
        assert_ne!(one, first);
        request["events"] = json!([{"kind":"click","key":"boot"},{"kind":"long_press","key":"up"},{"kind":"long_press","key":"down"}]);
        assert_eq!(pixels(&render(request)), one);
        assert_eq!(pixels(&render(counter())), first);
    }

    #[test]
    fn pixels_use_exact_device_packing_and_only_flushed_changes() {
        let source = component("#[widget::mounted] pub async fn m(self) {let c=os::screen::get_context()?;c.pixel([0,0],0)?;c.pixel([2,0],2)?;c.pixel([3,0],3)?;c.flush().await?;c.pixel([4,0],0)?;os::runtime::ok(())}");
        let frame = pixels(&render(source_request(&source, &["screen.present"])));
        assert_eq!(frame[0], 0b00_01_10_11);
        assert_eq!(frame[1], 0x55);
        assert!(frame[1..].iter().all(|value| *value == 0x55));
    }

    #[test]
    fn error_keeps_last_accepted_frame_and_logs() {
        let source = component("#[widget::mounted] pub async fn m(self) {let c=os::screen::get_context()?;c.clear(3)?;c.flush().await?;os::log::info(\"submitted\").await?;os::runtime::fail(\"after flush\")}");
        let result = render(source_request(&source, &["screen.present", "log.write"]));
        assert_eq!(result["ok"], false);
        assert_eq!(result["pixels"], json!(vec![255u8; 30_000]));
        assert_eq!(result["logs"], json!(["[info] submitted"]));
        assert!(result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("after flush"));
    }

    #[test]
    fn compile_errors_have_source_location_and_no_frame() {
        let result = render(source_request("struct State {", &[]));
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "rune_compile");
        assert!(result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("main.rn:1:"));
        assert_eq!(result["pixels"], Value::Null);
    }

    #[test]
    fn capabilities_instruction_and_allocation_limits_apply() {
        let mut denied = counter();
        denied["package"]["manifest"]["capabilities"] = json!([]);
        let denied = render(denied);
        assert_eq!(denied["ok"], false);
        assert_eq!(denied["error"]["code"], "capability_denied");
        for body in [
            "loop {}",
            "let values = []; loop {values.push([0,1,2,3,4,5,6,7,8,9]);}",
        ] {
            let source = component(&format!(
                "#[widget::mounted] pub async fn m(self) {{ {body} os::runtime::ok(()) }}"
            ));
            let result = render(source_request(&source, &[]));
            assert_eq!(result["ok"], false);
            assert_eq!(result["error"]["code"], "invalid_argument", "{result}");
            assert_eq!(result["pixels"], Value::Null);
        }
    }

    #[test]
    fn package_config_and_event_limits_reject_before_execution() {
        let mut oversized = counter();
        oversized["package"]["source"] = json!(" ".repeat(5121));
        assert_eq!(render(oversized)["error"]["code"], "source_size");
        let mut config = counter();
        config["config"] = json!([]);
        assert_eq!(render(config)["ok"], false);
        let mut events = counter();
        events["events"] = json!(vec![json!({"kind":"timer"}); 65]);
        assert_eq!(render(events)["error"]["code"], "quota_exceeded");
        let mut event = counter();
        event["events"] = json!([{"kind":"click","key":"confirm"}]);
        assert_eq!(render(event)["ok"], false);
    }

    #[test]
    fn calendar_uses_supplied_local_time_and_virtual_timer_advance() {
        let mut request = source_request(
            include_str!("../../../../examples/widgets/calendar/main.rn"),
            &["time.read", "screen.present", "log.write"],
        );
        let missing = pixels(&render(request.clone()));
        request["environment"] = json!({"unix_ms": 1_789_012_800_000i64,"utc_offset_minutes":480});
        let current = pixels(&render(request.clone()));
        assert_ne!(current, missing);
        request["events"] = json!([{"kind":"timer","advance_ms":86_400_000u64}]);
        assert_ne!(pixels(&render(request)), current);
    }

    #[test]
    fn todo_uses_exact_http_fixture_and_explicit_host_permissions() {
        let mut request = source_request(
            include_str!("../../../../examples/widgets/todo/main.rn"),
            &["screen.present", "http.fetch"],
        );
        request["config"] = json!({"url":"https://example.test/feed","auth_type":"basic","username":"alice","password":"secret"});
        request["environment"] = json!({"http":[{"url":"https://example.test/feed","status":200,"body":include_str!("../../../../examples/widgets/todo/feed.example.json")}]});
        assert_eq!(
            render(request.clone())["error"]["code"],
            "host_permission_denied"
        );
        request["host_permissions"] = json!(["https://example.test/feed"]);
        assert!(pixels(&render(request.clone()))
            .iter()
            .any(|value| *value != 0x55));
        request["environment"]["http"] = json!([]);
        assert_eq!(
            render(request)["error"]["code"],
            "preview_http_fixture_missing"
        );
    }

    #[test]
    fn album_reads_package_resources_and_persists_kv_between_events() {
        let mut request = source_request(
            include_str!("../../../../examples/widgets/photo-album/main.rn"),
            &["screen.present", "storage.kv", "log.write"],
        );
        request["package"]["resources"] = json!([
            {"path":"album.json","text":include_str!("../../../../examples/widgets/photo-album/resources/album.json")},
            {"path":"captions/morning.txt","text":"MORNING RIDGE"},
            {"path":"captions/market.txt","text":"MARKET STREET"},
            {"path":"captions/harbor.txt","text":"NIGHT HARBOR"}
        ]);
        let first = pixels(&render(request.clone()));
        request["events"] = json!([{"kind":"click","key":"boot"}]);
        let second = pixels(&render(request.clone()));
        assert_ne!(first, second);
        request["events"] =
            json!([{"kind":"click","key":"boot"},{"kind":"timer"},{"kind":"timer"}]);
        assert_eq!(pixels(&render(request)), first);
    }

    #[test]
    fn shared_and_private_files_stay_in_memory_and_enforce_capabilities() {
        let source = component("#[widget::mounted] pub async fn m(self) {let text=os::fs::shared::read_text(\"input.txt\").await?.unwrap();os::fs::private::write_text(\"state.txt\",text).await?;os::log::info(os::fs::private::read_text(\"state.txt\").await?.unwrap()).await?;os::runtime::ok(())}");
        let mut request = source_request(&source, &["fs.shared.read", "fs.private", "log.write"]);
        request["environment"] = json!({"shared_files":{"input.txt":"request only"}});
        let result = render(request.clone());
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["logs"], json!(["[info] request only"]));
        request["package"]["manifest"]["capabilities"] = json!(["fs.private", "log.write"]);
        assert_eq!(render(request)["error"]["code"], "capability_denied");
    }
}
