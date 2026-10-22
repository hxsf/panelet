// SPDX-License-Identifier: MPL-2.0

mod board;
mod input;
#[cfg(feature = "rune-footprint-poc")]
mod rune_footprint;
#[cfg(feature = "rune-runtime-only-footprint-poc")]
mod rune_runtime_only_footprint;
mod server;
mod storage;

#[cfg(all(
    feature = "rune-footprint-poc",
    feature = "rune-runtime-only-footprint-poc"
))]
compile_error!(
    "the compiler+VM and runtime-only Rune footprint probes are separate comparable variants; enable exactly one"
);

use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use board::Board;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use panelet_core::{
    parse_pcf8563, pcf8563_utc_to_unix_seconds, rtc_progress, unix_seconds_to_pcf8563_utc, Button,
    ButtonEvent, ButtonEventKind, LedController, MenuAction, RtcProgress, SessionResults, TestId,
    TestMenu, TestOutcome,
};
use panelet_rune_abi_poc::{BatteryReading, HostError, HostResult, RtcReading};
use panelet_ui::draw_status_dashboard;
use panelet_ui::draw_text;
use qrcodegen::{QrCode, QrCodeEcc};
use ssd2683::{
    Framebuffer, FramebufferError, PanelColor, PartialWindow, FRAMEBUFFER_LEN,
    HEIGHT as EPD_HEIGHT, WIDTH as EPD_WIDTH,
};

const SAMPLE_PERIOD_MS: u32 = 20;
const DEBOUNCE_SAMPLES: u8 = 3;
const BOOT_PROBE_SAMPLES: usize = 10;
const POWER_BUTTON_RELEASE_TIMEOUT_SAMPLES: usize = 100;
const PARTIAL_OBSERVATION_MS: u32 = 10_000;
const PARTIAL_FAILURE_RECOVERY_DELAY_MS: u32 = 2_000;
const BATTERY_STATUS_SAMPLE_COUNT: u16 = 100;

fn main() {
    // Required by esp-idf-sys before any wrapped ESP-IDF functionality is used.
    esp_idf_svc::sys::link_patches();

    board::assert_power_hold_early().expect("failed to assert GPIO17 power hold");
    let peripherals = Peripherals::take().expect("ESP-IDF peripherals already taken");
    let modem = peripherals.modem;
    let mut board = Board::take(
        peripherals.pins,
        peripherals.spi3,
        peripherals.i2c0,
        peripherals.adc1,
    )
    .expect("failed to establish safe NOTE4C GPIO state");

    esp_idf_svc::log::EspLogger::initialize_default();
    log::info!(
        "{{\"event\":\"boot\",\"firmware\":\"{}\",\"idf_build_expected\":\"v5.5.3\",\"psram_configured\":{}}}",
        env!("CARGO_PKG_VERSION"),
        cfg!(esp_idf_spiram)
    );

    let hold_release = board.hold_release_health();
    let initial_safe_outputs = board.enforce_safe_outputs();
    let availability = board.peripheral_availability();
    log::info!(
        "{{\"event\":\"board_safety\",\"hold_release_healthy\":{},\"hold_release_failed_gpio_mask\":{},\"safe_outputs\":{},\"details\":\"{:?}\"}}",
        hold_release.is_healthy(),
        hold_release.failed_gpio_mask,
        initial_safe_outputs.is_clean(),
        initial_safe_outputs
    );
    log::info!(
        "{{\"event\":\"peripheral_availability\",\"i2c\":{},\"i2c_init_error\":\"{:?}\",\"battery_adc\":{},\"battery_adc_init_error\":\"{:?}\",\"gpio1_standby\":{},\"gpio1_init_error\":\"{:?}\",\"gpio2_charge\":{},\"gpio2_init_error\":\"{:?}\"}}",
        availability.i2c,
        availability.i2c_init_error,
        availability.battery_adc,
        availability.battery_adc_init_error,
        availability.power_standby_status,
        availability.power_standby_init_error,
        availability.charge_status,
        availability.charge_status_init_error
    );

    #[cfg(feature = "rune-footprint-poc")]
    rune_footprint::run();

    #[cfg(feature = "rune-runtime-only-footprint-poc")]
    rune_runtime_only_footprint::run();

    let server = match server::ServerHandle::start(modem) {
        Ok(handle) => Some(handle),
        Err(error) => {
            log::error!(
                "{{\"event\":\"server_start_failed\",\"error_code\":{}}}",
                error.code()
            );
            None
        }
    };

    let safe_mode_button = stable_boot_hold(&board, Button::Up);
    let safe_mode = safe_mode_button
        || !hold_release.is_healthy()
        || !initial_safe_outputs.is_clean()
        || !board.epd_available();
    if safe_mode_button {
        wait_for_safe_mode_button_release(&board);
    }
    let power_button_released = wait_for_power_button_release(&board);
    if !power_button_released {
        log::warn!(
            "{{\"event\":\"button_stuck\",\"button\":\"DOWN_POWER\",\"timeout_ms\":{}}}",
            POWER_BUTTON_RELEASE_TIMEOUT_SAMPLES as u32 * SAMPLE_PERIOD_MS
        );
    }

    log::info!(
        "{{\"event\":\"mode\",\"safe\":{},\"epd\":\"off\",\"nfc\":\"off\",\"audio\":\"off\"}}",
        safe_mode
    );
    log::info!(
        "controls: normal mode routes buttons to the foreground widget; use management to enter diagnostic mode; diagnostic UP=previous, DOWN=next, BOOT=run; EPD_QUICK/STATUS_SCREEN require one ~1 s BOOT hold; EPD_PARTIAL_EXP requires two separate ~1 s BOOT holds with a release between them"
    );
    log::warn!(
        "tests run synchronously; button cancellation is unavailable after a test starts; ROM recovery remains BOOT+RESET"
    );

    if let Some(handle) = server.as_ref() {
        handle.set_safe_mode(safe_mode);
    }
    input::start(
        board
            .take_input_pins()
            .expect("button inputs are owned until startup completes"),
    )
    .expect("independent input sampler must start");
    if server.is_none() {
        input::transition(1, input::InputMode::Diagnostic);
    }
    run_server_loop(&mut board, safe_mode, server);
}

fn wait_for_safe_mode_button_release(board: &Board) {
    let mut released_samples = 0u8;
    let mut held_samples = 0u32;
    loop {
        if board.buttons().up {
            released_samples = 0;
            held_samples = held_samples.saturating_add(1);
            if held_samples % POWER_BUTTON_RELEASE_TIMEOUT_SAMPLES as u32 == 0 {
                log::warn!(
                    "{{\"event\":\"safe_mode_wait\",\"button\":\"UP\",\"held_ms\":{},\"rails\":\"off\"}}",
                    held_samples * SAMPLE_PERIOD_MS
                );
            }
        } else {
            released_samples = released_samples.saturating_add(1);
            if released_samples >= DEBOUNCE_SAMPLES {
                return;
            }
        }
        FreeRtos::delay_ms(SAMPLE_PERIOD_MS);
    }
}

fn stable_boot_hold(board: &Board, button: Button) -> bool {
    let mut consecutive = 0u8;
    for _ in 0..BOOT_PROBE_SAMPLES {
        if board.buttons().pressed(button) {
            consecutive = consecutive.saturating_add(1);
            if consecutive >= DEBOUNCE_SAMPLES {
                return true;
            }
        } else {
            consecutive = 0;
        }
        FreeRtos::delay_ms(SAMPLE_PERIOD_MS);
    }
    false
}

fn wait_for_power_button_release(board: &Board) -> bool {
    let mut released_samples = 0u8;
    for _ in 0..POWER_BUTTON_RELEASE_TIMEOUT_SAMPLES {
        if board.buttons().down {
            released_samples = 0;
        } else {
            released_samples = released_samples.saturating_add(1);
            if released_samples >= DEBOUNCE_SAMPLES {
                return true;
            }
        }
        FreeRtos::delay_ms(SAMPLE_PERIOD_MS);
    }
    false
}

fn run_server_loop(board: &mut Board, safe_mode: bool, server: Option<server::ServerHandle>) -> ! {
    let mut menu = TestMenu::new();
    let mut led = LedController::new(safe_mode);
    let mut session_results = SessionResults::new();
    let mut partial_attempted_this_boot = false;
    let mut diagnostic_epoch = None;
    announce_selection(&menu, safe_mode);
    led.show_selection(menu.selected().menu_number());

    loop {
        FreeRtos::delay_ms(SAMPLE_PERIOD_MS);

        if let Some(handle) = server.as_ref() {
            handle.update_heap();
            if let Some(screen) = handle.take_main_screen() {
                service_main_screen(handle, screen, board, safe_mode);
            }
            if let Some(presentation) = handle.take_presentation() {
                handle.set_display_busy(true);
                let result = if safe_mode || !board.epd_available() {
                    Err("display unavailable".to_owned())
                } else {
                    match <&[u8; FRAMEBUFFER_LEN]>::try_from(presentation.frame.pixels()) {
                        Ok(bytes) => {
                            let report = match presentation.refresh {
                                panelet_rune_abi_poc::display::RefreshKind::Full => {
                                    log::info!("screen refresh strategy=full submission={}", presentation.submission_id);
                                    board.run_epd_full_refresh(bytes)
                                }
                                panelet_rune_abi_poc::display::RefreshKind::Partial(window) => {
                                    log::info!("screen refresh strategy=partial submission={} window={:?}", presentation.submission_id, window);
                                    board.run_epd_window_refresh(bytes, window)
                                }
                            };
                            if report_is_successful(report) {
                                Ok(())
                            } else {
                                Err("display refresh or cleanup failed".to_owned())
                            }
                        }
                        Err(_) => Err("invalid framebuffer length".to_owned()),
                    }
                };
                handle.set_display_busy(false);
                handle.finish_presentation(presentation, result);
            }
            if let Some(request) = handle.take_runtime_hardware() {
                service_runtime_hardware(request, board);
            }
            if let Some(queued) = handle.take_main_command() {
                execute_server_command(
                    handle,
                    queued,
                    board,
                    safe_mode,
                    &mut session_results,
                    &mut partial_attempted_this_boot,
                );
            }
        }

        while let Some(input) = input::pop(input::InputMode::Diagnostic) {
            if !safe_mode
                && server
                    .as_ref()
                    .is_some_and(|handle| !handle.diagnostic_mode())
            {
                continue;
            }
            let input::InputPayload::Diagnostic(event) = input.payload else {
                continue;
            };
            if diagnostic_epoch != Some(input.epoch) {
                menu = TestMenu::new();
                diagnostic_epoch = Some(input.epoch);
                led = LedController::new(safe_mode);
                let _ = board.set_led(false);
                announce_selection(&menu, safe_mode);
            }
            log_button(event);
            match menu.handle(event) {
                MenuAction::None => {}
                MenuAction::SelectionChanged(test) => {
                    led.show_selection(test.menu_number());
                    announce_selection(&menu, safe_mode);
                }
                MenuAction::ArmStarted(test) => {
                    led.show_armed();
                    log::warn!(
                        "{{\"event\":\"experimental_arm\",\"test\":\"{}\",\"step\":1,\"required_next\":\"release_BOOT_then_hold_again\",\"navigation_cancels\":true}}",
                        test.name()
                    );
                }
                MenuAction::ArmReady(test) => {
                    led.show_armed();
                    log::warn!(
                        "{{\"event\":\"experimental_arm\",\"test\":\"{}\",\"step\":2,\"required_next\":\"hold_BOOT_again_for_1s\",\"navigation_cancels\":true}}",
                        test.name()
                    );
                }
                MenuAction::ArmCancelled { test, selected } => {
                    led.show_selection(selected.menu_number());
                    log::info!(
                        "{{\"event\":\"experimental_arm_cancelled\",\"test\":\"{}\",\"selected\":\"{}\"}}",
                        test.name(),
                        selected.name()
                    );
                    announce_selection(&menu, safe_mode);
                }
                MenuAction::Run(test) => {
                    if test == TestId::EpdPartialExperimental && partial_attempted_this_boot {
                        log::warn!(
                            "{{\"event\":\"test_refused\",\"test\":\"EPD_PARTIAL_EXP\",\"reason\":\"one_attempt_per_boot\"}}"
                        );
                        menu.finish(TestOutcome::Skipped);
                        session_results.record(test, TestOutcome::Skipped);
                        led.show_finished(TestOutcome::Skipped);
                        menu.return_to_idle();
                        announce_selection(&menu, safe_mode);
                        continue;
                    }
                    if test == TestId::EpdPartialExperimental {
                        // Consume the one-shot before any safety checks or I/O so
                        // no error path can accidentally permit another attempt.
                        partial_attempted_this_boot = true;
                    }
                    log::info!(
                        "{{\"event\":\"test_start\",\"test\":\"{}\",\"safe_mode\":{}}}",
                        test.name(),
                        safe_mode
                    );
                    led.show_running();
                    if let Err(error) = board.set_led(true) {
                        log::error!("LED running-state update failed: {error}");
                    }
                    let outcome = execute_test(board, test, safe_mode, &session_results);
                    menu.finish(outcome);
                    session_results.record(test, outcome);
                    led.show_finished(outcome);
                    log::info!(
                        "{{\"event\":\"test_finish\",\"test\":\"{}\",\"outcome\":\"{:?}\"}}",
                        test.name(),
                        outcome
                    );
                    menu.return_to_idle();
                    announce_selection(&menu, safe_mode);
                }
                MenuAction::AbortRequested(test) => {
                    // The synchronous runner cannot dispatch events while a test is
                    // running. Keep this branch explicit in case the menu core is later
                    // moved to another task, but do not promise live cancellation.
                    log::warn!(
                        "{{\"event\":\"abort_request\",\"test\":\"{}\",\"accepted\":false,\"reason\":\"synchronous_runner_has_no_live_cancel\"}}",
                        test.name()
                    );
                }
            }
        }
    }
}

fn service_main_screen(
    server: &server::ServerHandle,
    screen: server::MainScreen,
    board: &mut Board,
    safe_mode: bool,
) {
    if safe_mode || !board.epd_available() {
        log::warn!("server main screen skipped because the display is unavailable");
        return;
    }
    server.invalidate_screen();
    server.set_display_busy(true);
    let (name, outcome) = match screen {
        server::MainScreen::Provisioning(info) => {
            ("provisioning", run_server_provisioning_screen(board, &info))
        }
        server::MainScreen::Provisioned(info) => {
            ("provisioned", run_server_provisioned_screen(board, &info))
        }
    };
    server.set_display_busy(false);
    if matches!(outcome, TestOutcome::Observed | TestOutcome::Pass) {
        server.set_main_screen(name);
        server.enable_widget_polling(true);
    }
}

fn execute_server_command(
    server: &server::ServerHandle,
    queued: panelet_management::QueuedCommand,
    board: &mut Board,
    safe_mode: bool,
    session_results: &mut SessionResults,
    partial_attempted_this_boot: &mut bool,
) {
    use panelet_management::Command;
    use serde_json::json;

    let diagnostic = matches!(
        &queued.command,
        Command::TestRun { .. } | Command::DisplayText { .. }
    );
    if !server.command_is_current(&queued, diagnostic) {
        server.finish(
            queued.job_id,
            false,
            json!({"error":"stale diagnostic or display command"}),
        );
        return;
    }
    server.invalidate_screen();
    match queued.command {
        Command::DisplayText { text } => {
            server.set_display_busy(true);
            let outcome = run_epd_text(board, &text);
            if matches!(outcome, TestOutcome::Pass | TestOutcome::Observed) {
                server.set_main_screen("diagnostic");
            }
            server.set_display_busy(false);
            server.finish(
                queued.job_id,
                matches!(outcome, TestOutcome::Observed | TestOutcome::Pass),
                json!({"outcome": format!("{:?}", outcome)}),
            );
        }
        Command::DisplayStatus => {
            server.set_display_busy(true);
            let outcome = run_status_screen(board, safe_mode, session_results);
            if matches!(outcome, TestOutcome::Pass | TestOutcome::Observed) {
                server.set_main_screen("status");
            }
            server.set_display_busy(false);
            server.finish(
                queued.job_id,
                matches!(outcome, TestOutcome::Observed | TestOutcome::Pass),
                json!({"outcome": format!("{:?}", outcome)}),
            );
        }
        Command::TestRun { test } => {
            let Some(test_id) = test_id_from_rpc(&test) else {
                server.finish(queued.job_id, false, json!({"error":"unknown_test"}));
                return;
            };
            if test_id == TestId::EpdPartialExperimental && *partial_attempted_this_boot {
                server.finish(
                    queued.job_id,
                    false,
                    json!({"error":"one_attempt_per_boot"}),
                );
                return;
            }
            if test_id == TestId::EpdPartialExperimental {
                *partial_attempted_this_boot = true;
            }
            let display_test = matches!(
                test_id,
                TestId::EpdQuick | TestId::EpdPartialExperimental | TestId::StatusScreen
            );
            if display_test {
                server.set_display_busy(true);
            }
            let outcome = execute_test(board, test_id, safe_mode, session_results);
            if display_test {
                server.set_display_busy(false);
            }
            session_results.record(test_id, outcome);
            server.finish(
                queued.job_id,
                !matches!(outcome, TestOutcome::Fail | TestOutcome::Skipped),
                json!({"outcome": format!("{:?}", outcome)}),
            );
        }
        Command::SystemMode { .. }
        | Command::ScreenRetry
        | Command::WidgetActivate { .. }
        | Command::WidgetMount { .. }
        | Command::WidgetInput { .. }
        | Command::WidgetPoll { .. }
        | Command::WidgetConfigure { .. }
        | Command::WidgetUpload(_)
        | Command::WidgetDelete { .. } => {
            unreachable!("main worker cannot dequeue widget commands")
        }
        Command::WifiScan
        | Command::WifiConfigure(_)
        | Command::WifiForget { .. }
        | Command::TimeConfigure(_)
        | Command::TimeSync => {
            unreachable!("main worker cannot dequeue network commands")
        }
    }
}

fn service_runtime_hardware(
    request: server::RuntimeHardwareRequest,
    board: &mut Board,
) {
    match request {
        server::RuntimeHardwareRequest::Battery { cancelled, reply } => {
            if reject_cancelled_hardware(&cancelled, &reply) {
                return;
            }
            reply.complete(read_battery_for_widget(board));
        }
        server::RuntimeHardwareRequest::Rtc { cancelled, reply } => {
            if reject_cancelled_hardware(&cancelled, &reply) {
                return;
            }
            reply.complete(read_rtc_for_widget(board));
        }
        server::RuntimeHardwareRequest::RtcSetSystem { cancelled, reply } => {
            if reject_cancelled_hardware(&cancelled, &reply) {
                return;
            }
            reply.complete(write_system_time_to_rtc(board));
        }
    }
}

fn reject_cancelled_hardware<T>(
    cancelled: &std::sync::atomic::AtomicBool,
    reply: &server::Reply<T>,
) -> bool {
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        reply.complete(Err(HostError::cancelled()));
        true
    } else {
        false
    }
}


fn read_battery_for_widget(board: &mut Board) -> HostResult<BatteryReading> {
    let status = board.battery_status_levels().map_err(|error| {
        HostError::new(
            "battery_unavailable",
            "power",
            format!("{} is unavailable", error.component),
            true,
        )
    })?;
    let adc = board.battery_adc_samples().map_err(|error| {
        HostError::new(
            "battery_read_failed",
            "power",
            format!("{} read failed", error.component),
            true,
        )
    })?;
    Ok(BatteryReading {
        adc_raw: u32::from(adc.average_raw()),
        adc_pin_mv: u32::from(adc.average_calibrated_mv()),
        standby_pin_high: status.standby_high,
        charge_pin_low: !status.charge_high,
        // The divider ratio and cell chemistry have not been validated on
        // this board revision; expose raw measurements only for now.
        cell_mv: None,
        soc_percent: None,
    })
}

fn read_rtc_for_widget(board: &mut Board) -> HostResult<RtcReading> {
    let raw = board.rtc_read_once().map_err(|error| {
        HostError::new(
            "rtc_read_failed",
            "time",
            format!("RTC transaction failed at {:?}", error.phase),
            true,
        )
    })?;
    let sample = parse_pcf8563(raw).map_err(|error| {
        HostError::new(
            "rtc_invalid",
            "time",
            format!("RTC value is invalid: {error:?}"),
            false,
        )
    })?;
    Ok(RtcReading {
        year_00_99: sample.year,
        month: sample.month,
        day: sample.day,
        weekday: sample.weekday,
        hour: sample.hour,
        minute: sample.minute,
        second: sample.second,
        century_bit: sample.century_bit,
        voltage_low: sample.voltage_low,
        unix_ms: None,
        valid: !sample.voltage_low,
    })
}

fn write_system_time_to_rtc(board: &mut Board) -> HostResult<()> {
    let unix_seconds = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| {
                HostError::new(
                    "system_time_invalid",
                    "time",
                    "system clock is earlier than the Unix epoch",
                    true,
                )
            })?
            .as_secs(),
    )
    .map_err(|_| {
        HostError::new(
            "system_time_invalid",
            "time",
            "system clock does not fit the supported range",
            false,
        )
    })?;
    let encoded = unix_seconds_to_pcf8563_utc(unix_seconds).map_err(|_| {
        HostError::new(
            "system_time_invalid",
            "time",
            "system clock is outside the RTC 2000-2099 ownership convention",
            false,
        )
    })?;
    let verified = board.rtc_write_utc(encoded).map_err(|error| {
        HostError::new(
            "rtc_write_failed",
            "time",
            format!("RTC write transaction failed at {:?}", error.phase),
            true,
        )
    })?;
    let verified = parse_pcf8563(verified).map_err(|_| {
        HostError::new(
            "rtc_verify_failed",
            "time",
            "RTC read-back contained an invalid calendar value",
            true,
        )
    })?;
    let verified_seconds = pcf8563_utc_to_unix_seconds(verified).map_err(|_| {
        HostError::new(
            "rtc_verify_failed",
            "time",
            "RTC read-back did not match the UTC ownership convention",
            true,
        )
    })?;
    if !(0..=2).contains(&(verified_seconds - unix_seconds)) {
        return Err(HostError::new(
            "rtc_verify_failed",
            "time",
            "RTC read-back differed from the system clock",
            true,
        ));
    }
    Ok(())
}

fn test_id_from_rpc(name: &str) -> Option<TestId> {
    match name {
        "system_info" => Some(TestId::SystemInfo),
        "buttons_led" => Some(TestId::ButtonsAndLed),
        "epd_quick" => Some(TestId::EpdQuick),
        "epd_partial" => Some(TestId::EpdPartialExperimental),
        "i2c_scan" => Some(TestId::I2cScan),
        "rtc_read" => Some(TestId::RtcRead),
        "battery" => Some(TestId::Battery),
        "status_screen" => Some(TestId::StatusScreen),
        _ => None,
    }
}

fn execute_test(
    board: &mut Board,
    test: TestId,
    safe_mode: bool,
    session_results: &SessionResults,
) -> TestOutcome {
    let safe_outputs = board.enforce_safe_outputs();

    if matches!(
        test,
        TestId::EpdQuick | TestId::EpdPartialExperimental | TestId::StatusScreen
    ) && (safe_mode || !board.epd_available())
    {
        log::warn!(
            "{{\"event\":\"test_refused\",\"test\":\"{}\",\"reason\":\"board_safety_gate\",\"safe_mode\":{},\"epd_available\":{},\"hold_release_failed_gpio_mask\":{},\"safe_outputs\":\"{:?}\"}}",
            test.name(),
            safe_mode,
            board.epd_available(),
            board.hold_release_health().failed_gpio_mask,
            safe_outputs
        );
        return TestOutcome::Skipped;
    }

    if safe_mode && matches!(test, TestId::I2cScan | TestId::RtcRead) {
        log::warn!(
            "{{\"event\":\"test_refused\",\"test\":\"{}\",\"reason\":\"safe_mode_keeps_shared_gpio42_avdd_rail_off\"}}",
            test.name()
        );
        return TestOutcome::Skipped;
    }

    if !safe_outputs.is_clean() {
        log::error!(
            "{{\"event\":\"safe_output_failure\",\"test\":\"{}\",\"details\":\"{:?}\"}}",
            test.name(),
            safe_outputs
        );
        return TestOutcome::Fail;
    }

    match test {
        TestId::SystemInfo => {
            let reset_reason = unsafe { esp_idf_svc::sys::esp_reset_reason() };
            let free_heap = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
            let minimum_free_heap = unsafe { esp_idf_svc::sys::esp_get_minimum_free_heap_size() };
            log::info!(
                "{{\"event\":\"system_info\",\"reset_reason\":{},\"free_heap_bytes\":{},\"minimum_free_heap_bytes\":{},\"button_gpio\":{:?},\"battery_gpio\":{{\"standby_h\":{},\"chrg_l\":{},\"adc1_ch3\":{}}},\"i2c0_gpio\":{{\"sda\":{},\"scl\":{}}},\"usb_gpio_reserved\":[{},{}]}}",
                reset_reason,
                free_heap,
                minimum_free_heap,
                board::pins::BUTTONS,
                board::pins::POWER_STANDBY_STATUS,
                board::pins::CHARGE_STATUS,
                board::pins::BATTERY_ADC,
                board::pins::I2C_SDA,
                board::pins::I2C_SCL,
                board::pins::USB_D_MINUS_RESERVED,
                board::pins::USB_D_PLUS_RESERVED
            );
            TestOutcome::Pass
        }
        TestId::ButtonsAndLed => {
            let levels = board.buttons();
            log::info!(
                "{{\"event\":\"button_snapshot\",\"up\":{},\"down\":{},\"confirm\":{}}}",
                levels.up,
                levels.down,
                levels.confirm
            );
            for _ in 0..2 {
                if board.set_led(true).is_err() {
                    return TestOutcome::Fail;
                }
                thread::sleep(Duration::from_millis(100));
                if board.set_led(false).is_err() {
                    return TestOutcome::Fail;
                }
                thread::sleep(Duration::from_millis(100));
            }
            // GPIO reads and writes succeeded, but LED visibility needs a human.
            TestOutcome::Observed
        }
        TestId::EpdQuick => run_epd_quick(board),
        TestId::EpdPartialExperimental => run_epd_partial_experiment(board),
        TestId::I2cScan => run_i2c_scan(board),
        TestId::RtcRead => run_rtc_read(board),
        TestId::Battery => run_battery(board),
        TestId::StatusScreen => run_status_screen(board, safe_mode, session_results),
    }
}

fn run_i2c_scan(board: &mut Board) -> TestOutcome {
    log::info!(
        "{{\"event\":\"i2c_scan_begin\",\"address_start\":8,\"address_end\":119,\"bus_hz\":400000,\"transaction_timeout_ms\":100,\"round_bound_ms\":12000,\"internal_pullups\":false,\"gpio42_settle_ms\":3,\"nfc_rail\":\"off\",\"register_writes\":false}}"
    );
    let report = match board.i2c_scan() {
        Ok(report) => report,
        Err(error) => {
            log::error!(
                "{{\"event\":\"i2c_scan_error\",\"details\":\"{:?}\"}}",
                error
            );
            return if matches!(error.phase, board::I2cFailurePhase::Unavailable) {
                TestOutcome::Skipped
            } else {
                TestOutcome::Fail
            };
        }
    };

    let mut found_count = 0_u16;
    for address in 0x08_u8..=0x77 {
        if report.found[address as usize] {
            found_count += 1;
            log::info!(
                "{{\"event\":\"i2c_address_found\",\"address_7bit\":{},\"address_hex\":\"0x{:02X}\"}}",
                address,
                address
            );
        }
    }
    let expected_es8311 = report.found[0x18];
    let expected_pcf8563 = report.found[0x51];
    let nfc_0x55_present = report.found[0x55];
    log::info!(
        "{{\"event\":\"i2c_scan_finish\",\"attempted\":{},\"found_count\":{},\"nack_or_esp_fail\":{},\"timeout\":{},\"other_error\":{},\"first_timeout_address\":\"{:?}\",\"first_other_error\":\"{:?}\",\"round_deadline_hit\":{},\"elapsed_ms\":{},\"expected_0x18\":{},\"expected_0x51\":{},\"nfc_0x55_present\":{},\"nfc_0x55_required\":false}}",
        report.attempted,
        found_count,
        report.nack_or_fail,
        report.timeout,
        report.other_error,
        report.first_timeout_address,
        report.first_other_error,
        report.round_deadline_hit,
        report.transaction_elapsed_ms,
        expected_es8311,
        expected_pcf8563,
        nfc_0x55_present
    );

    if report.attempted == 112
        && !report.round_deadline_hit
        && report.timeout == 0
        && report.other_error == 0
        && expected_es8311
        && expected_pcf8563
    {
        TestOutcome::Pass
    } else {
        TestOutcome::Fail
    }
}

fn run_rtc_read(board: &mut Board) -> TestOutcome {
    log::info!(
        "{{\"event\":\"rtc_read_begin\",\"address_hex\":\"0x51\",\"register_hex\":\"0x02\",\"bytes_per_sample\":7,\"interval_requested_ms\":1100,\"write_operation\":\"register_pointer_only\",\"sets_time\":false}}"
    );
    let pair = match board.rtc_read_pair() {
        Ok(pair) => pair,
        Err(error) => {
            log::error!(
                "{{\"event\":\"rtc_read_error\",\"details\":\"{:?}\"}}",
                error
            );
            return if matches!(error.phase, board::I2cFailurePhase::Unavailable) {
                TestOutcome::Skipped
            } else {
                TestOutcome::Fail
            };
        }
    };
    let first = match parse_pcf8563(pair.first) {
        Ok(sample) => sample,
        Err(error) => {
            log::error!(
                "{{\"event\":\"rtc_parse_error\",\"sample\":1,\"raw\":{:?},\"error\":\"{:?}\"}}",
                pair.first,
                error
            );
            return TestOutcome::Fail;
        }
    };
    let second = match parse_pcf8563(pair.second) {
        Ok(sample) => sample,
        Err(error) => {
            log::error!(
                "{{\"event\":\"rtc_parse_error\",\"sample\":2,\"raw\":{:?},\"error\":\"{:?}\"}}",
                pair.second,
                error
            );
            return TestOutcome::Fail;
        }
    };
    let progress = rtc_progress(first, second, 3);
    log::info!(
        "{{\"event\":\"rtc_read_finish\",\"first\":{{\"year_00_99\":{},\"month\":{},\"day\":{},\"weekday\":{},\"hour\":{},\"minute\":{},\"second\":{},\"century_bit\":{},\"voltage_low\":{}}},\"second\":{{\"year_00_99\":{},\"month\":{},\"day\":{},\"weekday\":{},\"hour\":{},\"minute\":{},\"second\":{},\"century_bit\":{},\"voltage_low\":{}}},\"century_mapping_assumed\":false,\"interval_elapsed_ms\":{},\"progress\":\"{:?}\"}}",
        first.year,
        first.month,
        first.day,
        first.weekday,
        first.hour,
        first.minute,
        first.second,
        first.century_bit,
        first.voltage_low,
        second.year,
        second.month,
        second.day,
        second.weekday,
        second.hour,
        second.minute,
        second.second,
        second.century_bit,
        second.voltage_low,
        pair.interval_elapsed_ms,
        progress
    );

    if first.voltage_low || second.voltage_low {
        log::warn!("{{\"event\":\"rtc_untrusted\",\"reason\":\"pcf8563_voltage_low_flag_set\"}}");
        TestOutcome::Inconclusive
    } else if matches!(progress, RtcProgress::AdvancedBy(1..=3)) {
        TestOutcome::Pass
    } else {
        TestOutcome::Fail
    }
}

fn run_battery(board: &mut Board) -> TestOutcome {
    let first = match board.battery_status_levels() {
        Ok(levels) => levels,
        Err(error) => {
            log::warn!(
                "{{\"event\":\"battery_unavailable\",\"details\":\"{:?}\"}}",
                error
            );
            return TestOutcome::Skipped;
        }
    };
    let mut standby_high_count = u16::from(first.standby_high);
    let mut charge_high_count = u16::from(first.charge_high);
    let mut last = first;
    let started = Instant::now();
    for _ in 1..BATTERY_STATUS_SAMPLE_COUNT {
        FreeRtos::delay_ms(SAMPLE_PERIOD_MS);
        last = match board.battery_status_levels() {
            Ok(levels) => levels,
            Err(error) => {
                log::error!(
                    "{{\"event\":\"battery_status_read_error\",\"details\":\"{:?}\"}}",
                    error
                );
                return TestOutcome::Fail;
            }
        };
        standby_high_count += u16::from(last.standby_high);
        charge_high_count += u16::from(last.charge_high);
    }
    let status_elapsed_ms = started.elapsed().as_millis();
    let adc = match board.battery_adc_samples() {
        Ok(samples) => samples,
        Err(error) => {
            log::error!(
                "{{\"event\":\"battery_adc_error\",\"details\":\"{:?}\"}}",
                error
            );
            return if error.init_error.is_some() {
                TestOutcome::Skipped
            } else {
                TestOutcome::Fail
            };
        }
    };
    let pin_mv = adc.average_calibrated_mv();
    log::info!(
        "{{\"event\":\"battery_observation\",\"status_sample_count\":{},\"status_elapsed_ms\":{},\"gpio1_standby_h\":{{\"first_high\":{},\"last_high\":{},\"high_count\":{},\"high_duty_permille\":{},\"stability\":\"{}\"}},\"gpio2_chrg_l\":{{\"first_high\":{},\"last_high\":{},\"high_count\":{},\"high_duty_permille\":{},\"stability\":\"{}\"}},\"adc_gpio\":4,\"adc_unit_channel\":\"ADC1_CH3\",\"adc_resolution_bits\":12,\"adc_attenuation\":\"DB12\",\"adc_calibration\":\"Curve\",\"adc_raw_samples\":{:?},\"adc_calibrated_mv_samples\":{:?},\"adc_raw_average\":{},\"adc_pin_mv\":{},\"estimated_battery_mv_assuming_2_to_1\":{},\"divider_ratio_verified\":false,\"soc_percent_reported\":false}}",
        BATTERY_STATUS_SAMPLE_COUNT,
        status_elapsed_ms,
        first.standby_high,
        last.standby_high,
        standby_high_count,
        duty_permille(standby_high_count, BATTERY_STATUS_SAMPLE_COUNT),
        stable_level(standby_high_count, BATTERY_STATUS_SAMPLE_COUNT),
        first.charge_high,
        last.charge_high,
        charge_high_count,
        duty_permille(charge_high_count, BATTERY_STATUS_SAMPLE_COUNT),
        stable_level(charge_high_count, BATTERY_STATUS_SAMPLE_COUNT),
        adc.raw,
        adc.calibrated_mv,
        adc.average_raw(),
        pin_mv,
        u32::from(pin_mv) * 2
    );
    TestOutcome::Observed
}

fn duty_permille(high_count: u16, total: u16) -> u16 {
    ((u32::from(high_count) * 1_000) / u32::from(total)) as u16
}

fn stable_level(high_count: u16, total: u16) -> &'static str {
    if high_count == 0 {
        "stable_low"
    } else if high_count == total {
        "stable_high"
    } else {
        "mixed"
    }
}

fn run_status_screen(
    board: &mut Board,
    safe_mode: bool,
    session_results: &SessionResults,
) -> TestOutcome {
    let mut storage: Box<[u8; FRAMEBUFFER_LEN]> = vec![0_u8; FRAMEBUFFER_LEN]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("the vector length is fixed at FRAMEBUFFER_LEN"));
    let mut framebuffer = Framebuffer::new(storage.as_mut(), PanelColor::White);
    if let Err(error) = draw_status_dashboard(&mut framebuffer, safe_mode, session_results) {
        log::error!(
            "{{\"event\":\"status_screen_render_error\",\"error\":\"{}\"}}",
            error
        );
        return TestOutcome::Fail;
    }
    log::info!(
        "{{\"event\":\"status_screen_begin\",\"refresh\":\"full_once\",\"session_storage\":\"ram_only\",\"reset_clears_snapshot\":true,\"partial_refresh\":false,\"button_cancel_available\":false}}"
    );
    let started = Instant::now();
    let report = board.run_epd_full_refresh(framebuffer.as_bytes());
    log::info!(
        "{{\"event\":\"status_screen_finish\",\"elapsed_ms\":{},\"attempted\":{},\"driver_success\":{},\"driver_cleanup_clean\":{},\"forced_safe\":{},\"transport_only_observation\":true}}",
        started.elapsed().as_millis(),
        report.attempted,
        report.success,
        report.driver_cleanup_clean,
        report.forced_safe.is_clean()
    );
    if report_is_successful(report) {
        TestOutcome::Observed
    } else {
        TestOutcome::Fail
    }
}

fn run_server_provisioning_screen(
    board: &mut Board,
    setup: &server::ProvisioningInfo,
) -> TestOutcome {
    let mut storage: Box<[u8; FRAMEBUFFER_LEN]> = vec![0_u8; FRAMEBUFFER_LEN]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("the vector length is fixed at FRAMEBUFFER_LEN"));
    let mut frame = Framebuffer::new(storage.as_mut(), PanelColor::White);
    let management_url = format!("HTTP://{}/", setup.address);
    let wifi_payload = wifi_qr_payload(&setup.ap_ssid, &setup.ap_password);
    let render = (|| {
        draw_text(&mut frame, 10, 10, "PANELET SETUP", 3, PanelColor::Black)?;
        draw_text(&mut frame, 10, 45, "SSID", 1, PanelColor::Black)?;
        draw_text(&mut frame, 10, 58, &setup.ap_ssid, 1, PanelColor::Black)?;
        draw_text(&mut frame, 10, 82, "PASSWORD", 1, PanelColor::Black)?;
        draw_text(&mut frame, 10, 95, &setup.ap_password, 2, PanelColor::Black)?;
        draw_text(&mut frame, 10, 126, "MANAGEMENT", 1, PanelColor::Black)?;
        draw_text(&mut frame, 10, 139, &management_url, 1, PanelColor::Black)?;
        draw_text(
            &mut frame,
            10,
            165,
            "SCAN ADMIN QR FOR TOKEN",
            1,
            PanelColor::Black,
        )?;
        draw_text(
            &mut frame,
            10,
            221,
            "CONNECT THEN OPEN THE PAGE",
            1,
            PanelColor::Black,
        )?;
        draw_text(
            &mut frame,
            10,
            237,
            "HTTP IS LOCAL DEV ONLY",
            1,
            PanelColor::Red,
        )?;
        Ok::<(), panelet_ui::UiError>(())
    })();
    if let Err(error) = render {
        log::error!("server setup screen render failed: {error}");
        return TestOutcome::Fail;
    }
    if let Err(error) = draw_qr(&mut frame, 230, 38, 150, &wifi_payload) {
        log::error!("server setup QR render failed: {error}");
        return TestOutcome::Fail;
    }
    if let Err(error) = draw_text(&mut frame, 270, 172, "WIFI QR", 1, PanelColor::Black) {
        log::error!("server setup QR label render failed: {error}");
        return TestOutcome::Fail;
    }
    if let Err(error) = draw_qr(&mut frame, 290, 188, 100, &setup.admin_token) {
        log::error!("server setup admin QR render failed: {error}");
        return TestOutcome::Fail;
    }
    if let Err(error) = draw_text(&mut frame, 282, 290, "ADMIN QR", 1, PanelColor::Black) {
        log::error!("server setup admin QR label render failed: {error}");
        return TestOutcome::Fail;
    }
    let report = board.run_epd_full_refresh(frame.as_bytes());
    if report_is_successful(report) {
        TestOutcome::Observed
    } else {
        TestOutcome::Fail
    }
}

fn run_server_provisioned_screen(board: &mut Board, info: &server::ProvisionedInfo) -> TestOutcome {
    let mut storage: Box<[u8; FRAMEBUFFER_LEN]> = vec![0_u8; FRAMEBUFFER_LEN]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("the vector length is fixed at FRAMEBUFFER_LEN"));
    let mut frame = Framebuffer::new(storage.as_mut(), PanelColor::White);
    let ssid = display_safe_text(&info.ssid, 48);
    let management_url = format!("HTTP://{}/", info.address);
    let management_payload = format!("http://{}/", info.address);
    let render = (|| {
        draw_text(&mut frame, 10, 10, "PANELET READY", 3, PanelColor::Black)?;
        draw_text(&mut frame, 10, 48, "WIFI CONNECTED", 2, PanelColor::Black)?;
        draw_text(&mut frame, 10, 76, "SSID", 1, PanelColor::Black)?;
        draw_text(&mut frame, 10, 89, &ssid, 1, PanelColor::Black)?;
        draw_text(&mut frame, 10, 114, "MANAGEMENT", 1, PanelColor::Black)?;
        draw_text(&mut frame, 10, 127, &management_url, 1, PanelColor::Black)?;
        draw_text(
            &mut frame,
            10,
            158,
            "SCAN ADMIN QR FOR TOKEN",
            1,
            PanelColor::Black,
        )?;
        draw_text(
            &mut frame,
            10,
            205,
            "OPEN THE PAGE TO RUN A WIDGET",
            1,
            PanelColor::Black,
        )?;
        draw_text(
            &mut frame,
            10,
            226,
            "HTTP IS LOCAL DEV ONLY",
            1,
            PanelColor::Red,
        )?;
        Ok::<(), panelet_ui::UiError>(())
    })();
    if let Err(error) = render {
        log::error!("server provisioned screen render failed: {error}");
        return TestOutcome::Fail;
    }
    if let Err(error) = draw_qr(&mut frame, 230, 38, 150, &management_payload) {
        log::error!("server provisioned QR render failed: {error}");
        return TestOutcome::Fail;
    }
    if let Err(error) = draw_text(&mut frame, 246, 172, "MANAGEMENT QR", 1, PanelColor::Black) {
        log::error!("server provisioned QR label render failed: {error}");
        return TestOutcome::Fail;
    }
    if let Err(error) = draw_qr(&mut frame, 290, 188, 100, &info.admin_token) {
        log::error!("server provisioned admin QR render failed: {error}");
        return TestOutcome::Fail;
    }
    if let Err(error) = draw_text(&mut frame, 282, 290, "ADMIN QR", 1, PanelColor::Black) {
        log::error!("server provisioned admin QR label render failed: {error}");
        return TestOutcome::Fail;
    }
    let report = board.run_epd_full_refresh(frame.as_bytes());
    if report_is_successful(report) {
        TestOutcome::Observed
    } else {
        TestOutcome::Fail
    }
}

fn wifi_qr_payload(ssid: &str, password: &str) -> String {
    format!(
        "WIFI:T:WPA;S:{};P:{};;",
        wifi_qr_escape(ssid),
        wifi_qr_escape(password)
    )
}

fn wifi_qr_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | ';' | ',' | ':' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn draw_qr(
    frame: &mut Framebuffer<'_>,
    x: usize,
    y: usize,
    max_pixels: usize,
    payload: &str,
) -> Result<(), String> {
    const QUIET_ZONE: usize = 4;
    let qr = QrCode::encode_text(payload, QrCodeEcc::Medium)
        .map_err(|_| "payload is too large".to_owned())?;
    let modules = usize::try_from(qr.size()).map_err(|_| "invalid QR size".to_owned())?;
    let total_modules = modules + QUIET_ZONE * 2;
    let scale = max_pixels / total_modules;
    if scale == 0 || x + total_modules * scale > EPD_WIDTH || y + total_modules * scale > EPD_HEIGHT
    {
        return Err("QR placement exceeds framebuffer".to_owned());
    }
    for module_y in 0..modules {
        for module_x in 0..modules {
            if !qr.get_module(module_x as i32, module_y as i32) {
                continue;
            }
            let pixel_x = x + (module_x + QUIET_ZONE) * scale;
            let pixel_y = y + (module_y + QUIET_ZONE) * scale;
            for offset_y in 0..scale {
                for offset_x in 0..scale {
                    frame
                        .set_pixel(pixel_x + offset_x, pixel_y + offset_y, PanelColor::Black)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
    }
    Ok(())
}

fn display_safe_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .flat_map(char::to_uppercase)
        .map(|character| match character {
            'A'..='Z' | '0'..='9' | ' ' | '-' | '_' | '/' | '.' | ':' | '?' => character,
            _ => '?',
        })
        .take(max_chars)
        .collect()
}

fn run_epd_text(board: &mut Board, text: &str) -> TestOutcome {
    if !board.epd_available() {
        return TestOutcome::Skipped;
    }
    let mut storage: Box<[u8; FRAMEBUFFER_LEN]> = vec![0_u8; FRAMEBUFFER_LEN]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("the vector length is fixed at FRAMEBUFFER_LEN"));
    let mut frame = Framebuffer::new(storage.as_mut(), PanelColor::White);
    let mut y = 12;
    for logical_line in text.lines() {
        for chunk in logical_line.as_bytes().chunks(32) {
            if y + 14 > EPD_HEIGHT {
                break;
            }
            let chunk = core::str::from_utf8(chunk).unwrap_or("?");
            if let Err(error) = draw_text(&mut frame, 8, y, chunk, 2, PanelColor::Black) {
                log::error!("server display text render failed: {error}");
                return TestOutcome::Fail;
            }
            y += 18;
        }
    }
    let report = board.run_epd_full_refresh(frame.as_bytes());
    if report_is_successful(report) {
        TestOutcome::Observed
    } else {
        TestOutcome::Fail
    }
}

fn run_epd_quick(board: &mut Board) -> TestOutcome {
    // Build the 30 kB frame directly in an internal-heap allocation. This
    // avoids placing it on the 8 kB main task stack. build.rs rejects an
    // effective CONFIG_SPIRAM=y configuration for this recovery-safe image.
    let mut storage: Box<[u8; FRAMEBUFFER_LEN]> = vec![0_u8; FRAMEBUFFER_LEN]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("the vector length is fixed at FRAMEBUFFER_LEN"));
    let mut framebuffer = Framebuffer::new(storage.as_mut(), PanelColor::White);
    if let Err(error) = draw_epd_test_pattern(&mut framebuffer) {
        log::error!(
            "{{\"event\":\"epd_pattern_error\",\"error\":\"{}\"}}",
            error
        );
        return TestOutcome::Fail;
    }

    log::info!(
        "{{\"event\":\"epd_refresh_begin\",\"width\":400,\"height\":300,\"frame_bytes\":30000,\"spi_hz\":10000000,\"spi_mode\":0,\"spi_mode_evidence\":\"proposal_timing_inference\",\"busy_active_low\":\"provisional\",\"pattern\":\"left_to_right_black_white_yellow_red_asymmetric_corners\",\"button_cancel_available\":false}}"
    );
    let started = Instant::now();
    let report = board.run_epd_full_refresh(framebuffer.as_bytes());
    let elapsed_ms = started.elapsed().as_millis();
    log::info!(
        "{{\"event\":\"epd_refresh_finish\",\"elapsed_ms\":{},\"pon_drf_bounded_stage_elapsed_ms\":{},\"pon_drf_timing_available\":{},\"pon_drf_timing_scope\":\"commands_settle_and_bounded_busy_waits_not_pure_busy_low\",\"attempted\":{},\"driver_success\":{},\"driver_cleanup_clean\":{},\"forced_safe\":{},\"forced_safe_details\":\"{:?}\"}}",
        elapsed_ms,
        report.pon_drf_elapsed_ms.unwrap_or(0),
        report.pon_drf_elapsed_ms.is_some(),
        report.attempted,
        report.success,
        report.driver_cleanup_clean,
        report.forced_safe.is_clean(),
        report.forced_safe
    );

    if report.attempted
        && report.success
        && report.driver_cleanup_clean
        && report.forced_safe.is_clean()
    {
        // Transport/sequence completion is machine-observed; pigment mapping,
        // orientation, ghosting and refresh appearance still need a human.
        TestOutcome::Observed
    } else {
        TestOutcome::Fail
    }
}

fn run_epd_partial_experiment(board: &mut Board) -> TestOutcome {
    let window = PartialWindow::new(160, 239, 110, 189)
        .expect("the fixed center PTLW window is aligned and in bounds");
    let mut storage: Box<[u8; FRAMEBUFFER_LEN]> = vec![0_u8; FRAMEBUFFER_LEN]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("the vector length is fixed at FRAMEBUFFER_LEN"));
    let mut framebuffer = Framebuffer::new(storage.as_mut(), PanelColor::White);
    if let Err(error) = draw_epd_test_pattern(&mut framebuffer) {
        log::error!(
            "{{\"event\":\"epd_partial_pattern_error\",\"phase\":\"baseline\",\"error\":\"{}\"}}",
            error
        );
        return TestOutcome::Fail;
    }

    log::warn!(
        "{{\"event\":\"epd_partial_experiment_begin\",\"status\":\"unvalidated_protocol_probe\",\"window\":{{\"x_start\":160,\"x_end\":239,\"y_start\":110,\"y_end\":189,\"inclusive\":true,\"packed_bytes\":{}}},\"dtm_bytes_per_cycle\":30000,\"short_window_dtm\":false,\"button_cancel_available\":false}}",
        window.packed_len()
    );

    let baseline =
        run_logged_epd_phase(board, "baseline_full", framebuffer.as_bytes(), false, None);
    if !report_is_successful(baseline) {
        log::error!(
            "{{\"event\":\"epd_partial_experiment_finish\",\"outcome\":\"fail\",\"reason\":\"baseline_full_failed\",\"partial_attempted\":false,\"recovery_attempted\":false}}"
        );
        return TestOutcome::Fail;
    }

    log::info!(
        "{{\"event\":\"epd_observation\",\"phase\":\"baseline_full\",\"duration_ms\":{},\"purpose\":\"human_confirm_baseline_before_experimental_cycle\"}}",
        PARTIAL_OBSERVATION_MS
    );
    FreeRtos::delay_ms(PARTIAL_OBSERVATION_MS);

    if let Err(error) = draw_partial_center_pattern(&mut framebuffer, window) {
        log::error!(
            "{{\"event\":\"epd_partial_pattern_error\",\"phase\":\"modified_frame\",\"error\":\"{}\"}}",
            error
        );
        return TestOutcome::Fail;
    }

    let partial = run_logged_epd_phase(
        board,
        "partial_experimental",
        framebuffer.as_bytes(),
        true,
        Some(window),
    );
    if report_is_successful(partial) {
        log::info!(
            "{{\"event\":\"epd_observation\",\"phase\":\"partial_experimental\",\"duration_ms\":{},\"purpose\":\"human_inspect_window_and_ghosting\"}}",
            PARTIAL_OBSERVATION_MS
        );
        FreeRtos::delay_ms(PARTIAL_OBSERVATION_MS);
    } else if partial.forced_safe.is_clean() && board.epd_available() {
        log::warn!(
            "{{\"event\":\"epd_recovery_delay\",\"reason\":\"partial_failed_after_forced_safe\",\"duration_ms\":{}}}",
            PARTIAL_FAILURE_RECOVERY_DELAY_MS
        );
        FreeRtos::delay_ms(PARTIAL_FAILURE_RECOVERY_DELAY_MS);
    } else {
        log::error!(
            "{{\"event\":\"epd_recovery_skipped\",\"reason\":\"partial_failed_without_clean_forced_safe_or_epd_unavailable\",\"forced_safe\":{},\"epd_available\":{}}}",
            partial.forced_safe.is_clean(),
            board.epd_available()
        );
        return TestOutcome::Fail;
    }

    if let Err(error) = draw_epd_test_pattern(&mut framebuffer) {
        log::error!(
            "{{\"event\":\"epd_partial_pattern_error\",\"phase\":\"recovery_baseline\",\"error\":\"{}\"}}",
            error
        );
        return TestOutcome::Fail;
    }

    // Exactly one full-refresh restore follows the experimental cycle, whether
    // the PTLW refresh completed or entered its clean forced-safe failure path.
    let recovery =
        run_logged_epd_phase(board, "recovery_full", framebuffer.as_bytes(), false, None);
    let recovery_ok = report_is_successful(recovery);
    log::warn!(
        "{{\"event\":\"epd_partial_experiment_finish\",\"outcome\":\"{}\",\"protocol_transport_completed\":{},\"recovery_completed\":{},\"hardware_support_claimed\":false,\"human_verdict_required\":true}}",
        if report_is_successful(partial) && recovery_ok {
            "inconclusive"
        } else {
            "fail"
        },
        report_is_successful(partial),
        recovery_ok
    );

    if report_is_successful(partial) && recovery_ok {
        TestOutcome::Inconclusive
    } else {
        TestOutcome::Fail
    }
}

fn run_logged_epd_phase(
    board: &mut Board,
    phase: &'static str,
    frame: &[u8; FRAMEBUFFER_LEN],
    partial: bool,
    window: Option<PartialWindow>,
) -> board::EpdRunReport {
    log::info!(
        "{{\"event\":\"epd_phase_start\",\"phase\":\"{}\",\"mode\":\"{}\",\"dtm_bytes\":30000}}",
        phase,
        if partial {
            "experimental_partial"
        } else {
            "full"
        }
    );
    let started = Instant::now();
    let report = match window {
        Some(window) => board.run_epd_window_refresh(frame, window),
        None => board.run_epd_full_refresh(frame),
    };
    log::info!(
        "{{\"event\":\"epd_phase_finish\",\"phase\":\"{}\",\"total_elapsed_ms\":{},\"pon_drf_bounded_stage_elapsed_ms\":{},\"pon_drf_timing_available\":{},\"pon_drf_timing_scope\":\"commands_settle_and_bounded_busy_waits_not_pure_busy_low\",\"attempted\":{},\"driver_success\":{},\"driver_cleanup_clean\":{},\"forced_safe\":{},\"forced_safe_details\":\"{:?}\"}}",
        phase,
        started.elapsed().as_millis(),
        report.pon_drf_elapsed_ms.unwrap_or(0),
        report.pon_drf_elapsed_ms.is_some(),
        report.attempted,
        report.success,
        report.driver_cleanup_clean,
        report.forced_safe.is_clean(),
        report.forced_safe
    );
    report
}

fn report_is_successful(report: board::EpdRunReport) -> bool {
    report.attempted
        && report.success
        && report.driver_cleanup_clean
        && report.forced_safe.is_clean()
}

fn draw_partial_center_pattern(
    frame: &mut Framebuffer<'_>,
    window: PartialWindow,
) -> Result<(), FramebufferError> {
    for y in window.y_start() as usize..=window.y_end() as usize {
        for x in window.x_start() as usize..=window.x_end() as usize {
            let cell_x = (x - window.x_start() as usize) / 10;
            let cell_y = (y - window.y_start() as usize) / 10;
            let color = if (cell_x + cell_y) % 2 == 0 {
                PanelColor::Black
            } else {
                PanelColor::White
            };
            frame.set_pixel(x, y, color)?;
        }
    }
    Ok(())
}

fn draw_epd_test_pattern(frame: &mut Framebuffer<'_>) -> Result<(), FramebufferError> {
    frame.fill_vertical_bars([
        PanelColor::Black,
        PanelColor::White,
        PanelColor::Yellow,
        PanelColor::Red,
    ]);

    // Four unequal corner markers make rotation/mirroring obvious while also
    // cross-checking each physical pigment against a contrasting bar.
    fill_rect(frame, 8, 8, 72, 18, PanelColor::White)?;
    fill_rect(frame, 360, 8, 32, 36, PanelColor::Black)?;
    fill_rect(frame, 8, 260, 28, 32, PanelColor::Red)?;
    fill_rect(frame, 340, 278, 52, 14, PanelColor::Yellow)?;

    // A rising black diagonal is an additional non-symmetric orientation cue.
    for y in 44..256 {
        let x = 84 + y;
        for stroke in 0..3 {
            frame.set_pixel((x + stroke).min(EPD_WIDTH - 1), y, PanelColor::Black)?;
        }
    }
    Ok(())
}

fn fill_rect(
    frame: &mut Framebuffer<'_>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: PanelColor,
) -> Result<(), FramebufferError> {
    debug_assert!(x + width <= EPD_WIDTH);
    debug_assert!(y + height <= EPD_HEIGHT);
    for row in y..y + height {
        for column in x..x + width {
            frame.set_pixel(column, row, color)?;
        }
    }
    Ok(())
}

fn announce_selection(menu: &TestMenu, safe_mode: bool) {
    log::info!(
        "{{\"event\":\"menu\",\"selected\":\"{}\",\"safe_mode\":{}}}",
        menu.selected().name(),
        safe_mode
    );
}

fn log_button(event: ButtonEvent) {
    let button = match event.button {
        Button::Up => "UP",
        Button::Down => "DOWN_POWER",
        Button::Confirm => "BOOT_CONFIRM",
    };
    let action = match event.kind {
        ButtonEventKind::Pressed => "pressed",
        ButtonEventKind::Released => "released",
        ButtonEventKind::LongPressed => "long_pressed",
    };
    log::info!(
        "{{\"event\":\"button\",\"button\":\"{}\",\"action\":\"{}\"}}",
        button,
        action
    );
}
