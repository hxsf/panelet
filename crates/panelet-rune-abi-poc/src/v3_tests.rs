// SPDX-License-Identifier: MPL-2.0

//! Contract tests execute real Rune bytecode through the ABI 3 session driver.
use crate::display::{DisplayBroker, SubmissionState};
use crate::lifecycle::{
    ComponentEvent, ComponentSession, InputEvent, BOOT, CLICK, DOWN, LONG_PRESS, UP,
};
use crate::{
    context_with_os, mvp_context, HostFuture, HostIo, HostResult, HttpRequest, HttpResponse,
    LocalTimeReading, OsBindings, Std, StdConfig,
};
use rune::{Source, Sources, Vm};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[derive(Default)]
struct Io {
    calls: AtomicUsize,
    cancelled: AtomicBool,
    requests: Mutex<Vec<HttpRequest>>,
    response: Mutex<String>,
    kv: Mutex<BTreeMap<String, String>>,
    logs: Mutex<Vec<String>>,
}
impl HostIo for Io {
    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    fn http_fetch(&self, request: HttpRequest) -> HostFuture<HttpResponse> {
        // Count adapter entry itself, not only future polling: catches eager IO.
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request);
        let response = self.response.lock().unwrap().clone();
        Box::pin(async move { Ok(HttpResponse::new(200, response)) })
    }
    fn storage_get(&self, key: String) -> HostFuture<Option<String>> {
        let value = self.kv.lock().unwrap().get(&key).cloned();
        Box::pin(async move { Ok(value) })
    }
    fn storage_set(&self, key: String, value: String) -> HostFuture<()> {
        self.kv.lock().unwrap().insert(key, value);
        Box::pin(async { Ok(()) })
    }
    fn resource_get(&self, path: String) -> HostFuture<Option<String>> {
        let text = match path.as_str() {
            "album.json" => {
                include_str!("../../../examples/widgets/photo-album/resources/album.json")
            }
            "captions/morning.txt" => "MORNING RIDGE",
            "captions/market.txt" => "MARKET STREET",
            "captions/harbor.txt" => "NIGHT HARBOR",
            _ => return Box::pin(async { Ok(None) }),
        }
        .to_string();
        Box::pin(async move { Ok(Some(text)) })
    }
    fn log(&self, _level: String, message: String) -> HostFuture<()> {
        self.logs.lock().unwrap().push(message);
        Box::pin(async { Ok(()) })
    }
}
fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..100_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        std::thread::yield_now();
    }
    panic!("test future failed to complete");
}
fn vm(source: &str, bindings: &OsBindings) -> Vm {
    let context = context_with_os(bindings).unwrap();
    let mut sources = Sources::new();
    sources.insert(Source::memory(source).unwrap()).unwrap();
    let mut diagnostics = rune::Diagnostics::new();
    let unit = rune::prepare(&mut sources)
        .with_context(&context)
        .with_diagnostics(&mut diagnostics)
        .build()
        .unwrap_or_else(|e| panic!("build: {e:?}; {diagnostics:?}"));
    Vm::new(Arc::new(context.runtime().unwrap()), Arc::new(unit))
}
fn session(source: &str) -> (ComponentSession, DisplayBroker) {
    let display = DisplayBroker::new();
    let bindings = OsBindings::new(display.clone(), display.epoch());
    let session = ComponentSession::new(vm(source, &bindings), bindings, 65_536, 100_000).unwrap();
    (session, display)
}
fn dispatch(s: &mut ComponentSession, io: &Arc<Io>, event: ComponentEvent) -> HostResult<()> {
    dispatch_config(s, io, event, StdConfig::unrestricted_for_tests())
}
fn dispatch_config(
    s: &mut ComponentSession,
    io: &Arc<Io>,
    event: ComponentEvent,
    config: StdConfig,
) -> HostResult<()> {
    block_on(s.dispatch(Std::from_arc(io.clone(), config), event))
}
fn input(keycode: i64, kind: i64) -> ComponentEvent {
    ComponentEvent::Input(InputEvent {
        keycode,
        kind,
        held_ms: 1234,
    })
}
fn pixels(display: &DisplayBroker) -> Vec<u8> {
    let frame = display.poll(10_000).expect("submitted frame");
    let bytes = frame.frame.pixels().to_vec();
    display.finish(frame, Ok(())).unwrap();
    bytes
}
fn color(frame: &[u8], x: usize, y: usize) -> u8 {
    let p = y * 400 + x;
    (frame[p / 4] >> (6 - 2 * (p % 4))) & 3
}
fn component(body: &str) -> String {
    format!("struct State {{}} #[widget::component] impl State {{ pub fn new() {{ State {{}} }} {body} }}")
}

#[test]
fn macro_rejects_duplicate_bindings_invalid_signatures_and_removed_render() {
    for body in [
        "#[widget::input(click(boot, boot))] pub async fn f(self,e) { os::runtime::ok(()) }",
        "#[widget::input(click(boot))] pub async fn f(self,e) { os::runtime::ok(()) } #[widget::input(click(boot))] pub async fn g(self,e) { os::runtime::ok(()) }",
        "#[widget::mounted] pub fn f(self) { os::runtime::ok(()) }",
        "#[widget::mounted] pub async fn f(self,host) { os::runtime::ok(()) }",
        "#[widget::input(click(boot))] pub async fn f(self) { os::runtime::ok(()) }",
        "#[widget::input(double_click(boot))] pub async fn f(self,e) { os::runtime::ok(()) }",
        "#[widget::input(click(confirm))] pub async fn f(self,e) { os::runtime::ok(()) }",
        "#[widget::render] pub async fn f(self) { os::runtime::ok(()) }",
        "#[widget::mounted] #[widget::timer] pub async fn f(self) { os::runtime::ok(()) }",
    ] {
        let context=mvp_context().unwrap(); let mut sources=Sources::new();
        sources.insert(Source::memory(component(body)).unwrap()).unwrap();
        assert!(rune::prepare(&mut sources).with_context(&context).build().is_err(),"accepted {body}");
    }
}
#[test]
fn legacy_main_is_not_an_executable_component() {
    let d = DisplayBroker::new();
    let b = OsBindings::new(d.clone(), d.epoch());
    assert!(
        ComponentSession::new(vm("pub async fn main(std) { 0 }", &b), b, 65536, 100000).is_err()
    );
}
#[test]
fn constructor_has_no_host_context() {
    let source="struct State {} #[widget::component] impl State { pub fn new() { os::screen::get_context().unwrap(); State {} } }";
    let (mut s, d) = session(source);
    let io = Arc::new(Io::default());
    assert!(dispatch(&mut s, &io, ComponentEvent::Mount).is_err());
    assert!(s.is_poisoned());
    assert_eq!(d.status().state, SubmissionState::Idle);
}
#[test]
fn multibinding_dispatches_only_matched_events_with_readonly_metadata() {
    let source = component(
        r#"#[widget::input(click(boot), long_press(up, down))]
      pub async fn f(self,e) {
        let text=e.keycode.to_string(); text.push_str(":"); text.push_str(e.kind.to_string());
        text.push_str(":"); text.push_str(e.held_ms.to_string()); os::log::info(text).await?; os::runtime::ok(())
      }"#,
    );
    let (mut s, _) = session(&source);
    let io = Arc::new(Io::default());
    for e in [
        input(BOOT, CLICK),
        input(UP, LONG_PRESS),
        input(DOWN, LONG_PRESS),
        input(UP, CLICK),
    ] {
        dispatch(&mut s, &io, e).unwrap();
    }
    assert_eq!(
        *io.logs.lock().unwrap(),
        vec!["2:0:1234", "0:1:1234", "1:1:1234"]
    );
    let source = component(
        "#[widget::input(click(boot))] pub async fn f(self,e) { e.keycode=0; os::runtime::ok(()) }",
    );
    let (mut bad, _) = session(&source);
    assert!(dispatch(&mut bad, &io, input(BOOT, CLICK)).is_err());
}
#[test]
fn cold_future_is_not_started_and_retained_future_cannot_use_next_invocation() {
    let source = r#"struct State { future } #[widget::component] impl State {
      pub fn new() { State { future: [] } }
      #[widget::mounted] pub async fn m(self) { self.future.push(os::http::get("https://example.test/feed")); os::runtime::ok(()) }
      #[widget::timer] pub async fn t(self) { self.future.pop().unwrap().await?; os::runtime::ok(()) }
    }"#;
    let (mut s, _) = session(source);
    let io = Arc::new(Io::default());
    dispatch(&mut s, &io, ComponentEvent::Mount).unwrap();
    assert_eq!(io.calls.load(Ordering::SeqCst), 0);
    assert!(dispatch(&mut s, &io, ComponentEvent::Timer).is_err());
    assert_eq!(io.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn cancellation_prevents_adapter_io() {
    let source=component("#[widget::mounted] pub async fn m(self) { os::http::get(\"https://example.test/feed\").await?; os::runtime::ok(()) }");
    let (mut s, _) = session(&source);
    let io = Arc::new(Io::default());
    io.cancelled.store(true, Ordering::SeqCst);
    assert!(dispatch(&mut s, &io, ComponentEvent::Mount).is_err());
    assert_eq!(io.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn canvas_is_shared_in_invocation_inherits_committed_frame_and_drops_unflushed_changes() {
    let source = component(
        r#"
      #[widget::mounted] pub async fn m(self) {
        let a=os::screen::get_context()?;let b=os::screen::get_context()?;
        a.pixel([0,0],os::screen::RED)?;b.pixel([1,0],os::screen::YELLOW)?;
        let receipt=a.flush().await?;if receipt.submission_id==0 {return os::runtime::fail("receipt");}
        b.pixel([2,0],os::screen::BLACK)?;os::runtime::ok(())
      }
      #[widget::timer] pub async fn t(self) {
        let c=os::screen::get_context()?;c.pixel([3,0],os::screen::BLACK)?;c.flush().await?;os::runtime::ok(())
      }"#,
    );
    let (mut s, d) = session(&source);
    let io = Arc::new(Io::default());
    dispatch(&mut s, &io, ComponentEvent::Mount).unwrap();
    let first = pixels(&d);
    assert_eq!(
        (
            color(&first, 0, 0),
            color(&first, 1, 0),
            color(&first, 2, 0)
        ),
        (3, 2, 1)
    );
    dispatch(&mut s, &io, ComponentEvent::Timer).unwrap();
    let second = pixels(&d);
    assert_eq!(
        (
            color(&second, 0, 0),
            color(&second, 1, 0),
            color(&second, 2, 0),
            color(&second, 3, 0)
        ),
        (3, 2, 1, 0)
    );
}
#[test]
fn retained_canvas_is_revoked_and_unflushed_render_never_submits() {
    let source = r#"struct State { canvas } #[widget::component] impl State {
      pub fn new() { State {canvas:[]} }
      #[widget::mounted] pub async fn m(self) {let c=os::screen::get_context()?;c.clear(0)?;self.canvas.push(c);os::runtime::ok(())}
      #[widget::timer] pub async fn t(self) {self.canvas.pop().unwrap().flush().await?;os::runtime::ok(())}
    }"#;
    let (mut s, d) = session(source);
    let io = Arc::new(Io::default());
    dispatch(&mut s, &io, ComponentEvent::Mount).unwrap();
    assert!(d.poll(10000).is_none());
    assert!(dispatch(&mut s, &io, ComponentEvent::Timer).is_err());
    assert!(d.poll(10000).is_none());
}
#[test]
fn accepted_frame_survives_later_script_error_and_cleanup_cannot_submit() {
    let source=component("#[widget::mounted] pub async fn m(self) {let c=os::screen::get_context()?;c.clear(3)?;c.flush().await?;os::runtime::fail(\"after flush\")}");
    let (mut s, d) = session(&source);
    let io = Arc::new(Io::default());
    assert!(dispatch(&mut s, &io, ComponentEvent::Mount).is_err());
    assert_eq!(pixels(&d), vec![0xff; 30000]);
    let source=component("#[widget::before_destroy] pub async fn d(self) {let c=os::screen::get_context()?;c.flush().await?;os::runtime::ok(())}");
    let (mut s, d) = session(&source);
    dispatch(&mut s, &io, ComponentEvent::Mount).unwrap();
    assert!(dispatch(&mut s, &io, ComponentEvent::BeforeDestroy).is_err());
    assert!(d.poll(10000).is_none());
}
#[test]
fn actual_counter_keeps_state_and_distinguishes_buttons() {
    let (mut s, d) = session(include_str!("../../../examples/widgets/counter/main.rn"));
    let io = Arc::new(Io::default());
    dispatch(&mut s, &io, ComponentEvent::Mount).unwrap();
    let zero = pixels(&d);
    dispatch(&mut s, &io, input(BOOT, CLICK)).unwrap();
    let one = pixels(&d);
    assert_ne!(one, zero);
    dispatch(&mut s, &io, input(UP, LONG_PRESS)).unwrap();
    let eleven = pixels(&d);
    assert_ne!(eleven, one);
    dispatch(&mut s, &io, input(DOWN, LONG_PRESS)).unwrap();
    assert_eq!(pixels(&d), one);
}
#[test]
fn actual_todo_preserves_http_auth_and_submits_only_valid_snapshot() {
    let io = Arc::new(Io::default());
    *io.response.lock().unwrap() =
        include_str!("../../../examples/widgets/todo/feed.example.json").into();
    let (mut s, d) = session(include_str!("../../../examples/widgets/todo/main.rn"));
    let config=StdConfig {widget_config:r#"{"url":"https://example.test/feed","auth_type":"basic","username":"alice","password":"secret"}"#.into(),..StdConfig::unrestricted_for_tests()};
    dispatch_config(&mut s, &io, ComponentEvent::Mount, config.clone()).unwrap();
    let frame = pixels(&d);
    assert!(frame.iter().any(|v| *v != 0x55));
    assert_eq!(
        io.requests.lock().unwrap()[0]
            .headers
            .as_slice()
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("authorization"))
            .unwrap()
            .value,
        "Basic YWxpY2U6c2VjcmV0"
    );
    *io.response.lock().unwrap() = "{\"version\":1,\"items\":[{\"text\":4}]}".into();
    assert!(dispatch_config(&mut s, &io, input(BOOT, CLICK), config).is_err());
    assert!(d.poll(10000).is_none());
}
#[test]
fn actual_calendar_has_time_missing_and_current_month_paths() {
    let io = Arc::new(Io::default());
    let (mut s, d) = session(include_str!("../../../examples/widgets/calendar/main.rn"));
    dispatch(&mut s, &io, ComponentEvent::Mount).unwrap();
    let no_time = pixels(&d);
    let config = StdConfig {
        local_time: Some(LocalTimeReading {
            year: 2026,
            month: 9,
            day: 9,
            weekday: 3,
            hour: 12,
            minute: 0,
            second: 0,
            utc_offset_minutes: 480,
            unix_ms: 0,
        }),
        ..StdConfig::unrestricted_for_tests()
    };
    dispatch_config(&mut s, &io, ComponentEvent::Timer, config).unwrap();
    assert_ne!(pixels(&d), no_time);
    assert!(io
        .logs
        .lock()
        .unwrap()
        .iter()
        .any(|v| v == "CALENDAR SUBMITTED"));
}
#[test]
fn actual_album_advances_persistent_cursor_on_acceptance_not_physical_completion() {
    let io = Arc::new(Io::default());
    let (mut s, d) = session(include_str!(
        "../../../examples/widgets/photo-album/main.rn"
    ));
    dispatch(&mut s, &io, ComponentEvent::Mount).unwrap();
    assert_eq!(
        io.kv.lock().unwrap().get("album.current").unwrap(),
        "market"
    );
    let morning = pixels(&d);
    dispatch(&mut s, &io, input(BOOT, CLICK)).unwrap();
    let market = pixels(&d);
    assert_ne!(market, morning);
    dispatch(&mut s, &io, ComponentEvent::Timer).unwrap();
    let harbor = pixels(&d);
    assert_ne!(harbor, market);
    assert_eq!(
        io.kv.lock().unwrap().get("album.current").unwrap(),
        "morning"
    );
}

#[test]
fn os_configuration_belongs_to_each_vm_and_each_invocation() {
    let source = component(
        r#"
        #[widget::mounted] pub async fn m(self) { self.read().await }
        #[widget::timer] pub async fn t(self) { self.read().await }
        pub async fn read(self) {
            let config=os::config::get()?;
            os::log::info(config.get("owner")?.unwrap().string()?.unwrap()).await?;
            os::runtime::ok(())
        }"#,
    );
    let (mut a, _) = session(&source);
    let (mut b, _) = session(&source);
    let io = Arc::new(Io::default());
    for (first, owner) in [(true, "alpha"), (false, "beta"), (true, "alpha-new")] {
        let s = if first { &mut a } else { &mut b };
        let config = StdConfig {
            widget_config: format!("{{\"owner\":\"{owner}\"}}"),
            ..StdConfig::unrestricted_for_tests()
        };
        dispatch_config(s, &io, ComponentEvent::Timer, config).unwrap();
    }
    assert_eq!(*io.logs.lock().unwrap(), vec!["alpha", "beta", "alpha-new"]);
}

#[test]
fn retaining_many_revoked_canvas_handles_does_not_exhaust_four_frame_pool() {
    let source = r#"struct State { old, count } #[widget::component] impl State {
        pub fn new() { State {old:[],count:0} }
        #[widget::mounted] pub async fn m(self) { self.submit().await }
        #[widget::timer] pub async fn t(self) { self.submit().await }
        pub async fn submit(self) {
            let c=os::screen::get_context()?;
            c.pixel([self.count,0],os::screen::BLACK)?;
            c.flush().await?;
            self.old.push(c);self.count+=1;os::runtime::ok(())
        }
    }"#;
    let (mut s, d) = session(source);
    let io = Arc::new(Io::default());
    for count in 0..12 {
        dispatch(&mut s, &io, ComponentEvent::Timer).unwrap();
        let frame = pixels(&d);
        assert_eq!(color(&frame, count, 0), 0);
        assert_eq!(color(&frame, count + 1, 0), 1);
    }
}

#[test]
fn hook_returning_old_render_boolean_poisoned_instead_of_implicit_redraw() {
    let source = component("#[widget::mounted] pub async fn m(self) { os::runtime::ok(true) }");
    let (mut s, d) = session(&source);
    let io = Arc::new(Io::default());
    assert!(dispatch(&mut s, &io, ComponentEvent::Mount).is_err());
    assert!(s.is_poisoned());
    assert!(d.poll(10_000).is_none());
}

#[test]
fn retained_runtime_combinators_reject_before_polling_script_children() {
    for wrapper in [
        "os::runtime::wait_all([write_later()])",
        "os::runtime::timeout(write_later(), 1000)",
    ] {
        let source = format!(
            r#"
            pub async fn write_later() {{
                os::storage::set("unexpected", "old future ran").await?;
                os::log::info("old future ran").await?;
                os::runtime::ok(())
            }}
            struct State {{ pending }}
            #[widget::component]
            impl State {{
                pub fn new() {{ State {{ pending: () }} }}
                #[widget::mounted]
                pub async fn mounted(self) {{
                    self.pending = {wrapper};
                    os::runtime::ok(())
                }}
                #[widget::input(click(boot))]
                pub async fn input(self, event) {{
                    let result = self.pending.await;
                    if result.is_ok() {{ return os::runtime::fail("old combinator executed"); }}
                    os::runtime::ok(())
                }}
            }}
        "#
        );
        let (mut s, _) = session(&source);
        let io = Arc::new(Io::default());
        dispatch(&mut s, &io, ComponentEvent::Mount).unwrap();
        assert!(io.kv.lock().unwrap().is_empty(), "eager work: {wrapper}");
        assert!(io.logs.lock().unwrap().is_empty(), "eager log: {wrapper}");
        dispatch(&mut s, &io, input(BOOT, CLICK)).unwrap();
        assert!(io.kv.lock().unwrap().is_empty(), "revoked work: {wrapper}");
        assert!(io.logs.lock().unwrap().is_empty(), "revoked log: {wrapper}");
    }
}

#[test]
fn initial_input_mount_and_input_share_the_dispatch_drawing_context() {
    let source = component(
        r#"
        #[widget::mounted]
        pub async fn mounted(self) {
            let canvas = os::screen::get_context()?;
            canvas.pixel([1, 1], os::screen::RED)?;
            canvas.flush().await?;
            os::runtime::ok(())
        }
        #[widget::input(click(boot))]
        pub async fn input(self, event) {
            let canvas = os::screen::get_context()?;
            canvas.pixel([2, 1], os::screen::BLACK)?;
            canvas.flush().await?;
            os::runtime::ok(())
        }
    "#,
    );
    let (mut s, display) = session(&source);
    let io = Arc::new(Io::default());
    dispatch(&mut s, &io, input(BOOT, CLICK)).unwrap();
    assert_eq!(display.status().submission_id, Some(2));
    let frame = pixels(&display);
    assert_eq!(color(&frame, 1, 1), 3);
    assert_eq!(color(&frame, 2, 1), 0);
    assert_eq!(color(&frame, 3, 1), 1);
    assert_eq!(display.pool_usage().0, 1);
}
