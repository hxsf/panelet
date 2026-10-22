// SPDX-License-Identifier: MPL-2.0

use core::fmt;

#[cfg(feature = "experimental-partial-refresh")]
use crate::PartialWindow;
use crate::{Command, DisplayInterface, FRAMEBUFFER_LEN};

pub const MAX_WRITE_HZ: u32 = 20_000_000;
pub const MAX_READ_HZ: u32 = 2_500_000;
const FRAME_WRITE_CHUNK: usize = 4_000;
const REFERENCE_VENDOR_COMPAT_COMMAND: u8 = 0xE9;
const REFERENCE_VENDOR_COMPAT_DATA: u8 = 0x01;
const DISPLAY_REFRESH_DATA: u8 = 0x00;
const POWER_OFF_DATA: u8 = 0x00;
const DEEP_SLEEP_CHECK: u8 = 0xA5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpiRates {
    write_hz: u32,
    read_hz: Option<u32>,
}

impl SpiRates {
    pub fn new(write_hz: u32, read_hz: Option<u32>) -> Result<Self, SpiRateError> {
        if write_hz == 0 || write_hz > MAX_WRITE_HZ {
            return Err(SpiRateError::InvalidWriteRate {
                requested: write_hz,
            });
        }
        if let Some(requested) = read_hz {
            if requested == 0 || requested > MAX_READ_HZ {
                return Err(SpiRateError::InvalidReadRate { requested });
            }
        }
        Ok(Self { write_hz, read_hz })
    }

    #[must_use]
    pub const fn write_hz(self) -> u32 {
        self.write_hz
    }

    #[must_use]
    pub const fn read_hz(self) -> Option<u32> {
        self.read_hz
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpiRateError {
    InvalidWriteRate { requested: u32 },
    InvalidReadRate { requested: u32 },
}

impl fmt::Display for SpiRateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWriteRate { requested } => write!(
                f,
                "SSD2683 write clock {requested} Hz is outside 1..={MAX_WRITE_HZ} Hz"
            ),
            Self::InvalidReadRate { requested } => write!(
                f,
                "SSD2683 read clock {requested} Hz is outside 1..={MAX_READ_HZ} Hz"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BusyWait {
    pub max_polls: u32,
    pub poll_interval_ms: u32,
}

impl BusyWait {
    pub const fn new(max_polls: u32, poll_interval_ms: u32) -> Result<Self, BusyWaitError> {
        if max_polls == 0 {
            return Err(BusyWaitError::ZeroPolls);
        }
        Ok(Self {
            max_polls,
            poll_interval_ms,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusyWaitError {
    ZeroPolls,
}

/// Timing policy for the bounded full-refresh sequence.
///
/// The reset hold of 20 ms and 50 ms post-release quiet window are conservative
/// interpretations of the SSD2683 proposal's reset/LUT-load timing. The quiet
/// window also avoids sampling BUSY_N high before the controller has had time to
/// assert it. The 10 ms PON-ready settle, 10 ms DRF-to-BUSY delay, 20 ms
/// post-POF delay and 120 s BUSY ceiling are reference-firmware policy, not
/// controller-guaranteed refresh durations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationConfig {
    pub reset_hold_after_power_ms: u32,
    pub post_reset_release_ms: u32,
    pub post_power_on_ready_ms: u32,
    pub post_refresh_command_ms: u32,
    pub post_power_off_ms: u32,
    pub busy_wait: BusyWait,
}

impl Default for OperationConfig {
    fn default() -> Self {
        Self {
            reset_hold_after_power_ms: 20,
            post_reset_release_ms: 50,
            post_power_on_ready_ms: 10,
            post_refresh_command_ms: 10,
            post_power_off_ms: 20,
            busy_wait: BusyWait {
                max_polls: 2_400,
                poll_interval_ms: 50,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverState {
    PoweredOff,
    ResetHeldLow,
    RailEnabled,
    Ready,
    FrameLoaded,
    PanelPowered,
    Refreshing,
    PanelPoweredOff,
    DeepSleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverStatus {
    pub state: DriverState,
    /// An I/O error or BUSY timeout occurred after the last known-good state.
    pub faulted: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub enum DriverError<E> {
    Interface(E),
    BusyTimeout {
        polls: u32,
    },
    InvalidState {
        expected: DriverState,
        actual: DriverState,
    },
}

impl<E: fmt::Display> fmt::Display for DriverError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interface(error) => write!(f, "SSD2683 interface error: {error}"),
            Self::BusyTimeout { polls } => {
                write!(f, "SSD2683 BUSY_N stayed low for {polls} polls")
            }
            Self::InvalidState { expected, actual } => {
                write!(f, "SSD2683 expected {expected:?}, found {actual:?}")
            }
        }
    }
}

/// Outcome of a best-effort failure cleanup.
///
/// All fields are independent because cleanup continues after an earlier
/// failure. `rail_off` is attempted even if graceful POF/DSLP failed.
#[derive(Debug, Eq, PartialEq)]
pub struct CleanupReport<E> {
    pub graceful_power_off_attempted: bool,
    pub graceful_power_off: Option<DriverError<E>>,
    pub deep_sleep_attempted: bool,
    pub deep_sleep: Option<DriverError<E>>,
    pub bus_idle: Option<E>,
    pub reset_low: Option<E>,
    pub rail_off: Option<E>,
}

impl<E> CleanupReport<E> {
    /// Whether every operation that was actually attempted succeeded.
    ///
    /// A `true` result does not imply graceful POF/DSLP was attempted. Inspect
    /// the two `*_attempted` fields; BUSY timeout cleanup deliberately skips
    /// controller commands and cuts the rail directly.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.graceful_power_off.is_none()
            && self.deep_sleep.is_none()
            && self.bus_idle.is_none()
            && self.reset_low.is_none()
            && self.rail_off.is_none()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct RefreshFailure<E> {
    pub cause: DriverError<E>,
    pub cleanup: CleanupReport<E>,
}

pub struct Ssd2683<I> {
    io: I,
    rates: SpiRates,
    status: DriverStatus,
}

impl<I: DisplayInterface> Ssd2683<I> {
    #[must_use]
    pub const fn new(io: I, rates: SpiRates) -> Self {
        Self {
            io,
            rates,
            status: DriverStatus {
                state: DriverState::PoweredOff,
                faulted: false,
            },
        }
    }

    #[must_use]
    pub const fn rates(&self) -> SpiRates {
        self.rates
    }

    #[must_use]
    pub const fn status(&self) -> DriverStatus {
        self.status
    }

    #[must_use]
    pub fn release(self) -> I {
        self.io
    }

    /// Hold reset low before enabling the EPD rail, observe a quiet window after
    /// reset release, then wait for BUSY_N high.
    ///
    /// The proposal and archived firmware describe BUSY_N as active-low. The
    /// polarity at the NOTE4C board boundary still requires hardware confirmation.
    pub fn initialize(&mut self, config: OperationConfig) -> Result<(), DriverError<I::Error>> {
        self.require_state(DriverState::PoweredOff)?;
        self.status.faulted = false;
        let result = self.io.set_bus_idle();
        self.interface(result)?;
        let result = self.io.set_reset(false);
        self.interface(result)?;
        self.status.state = DriverState::ResetHeldLow;
        let result = self.io.set_power(true);
        self.interface(result)?;
        self.status.state = DriverState::RailEnabled;
        self.io.delay_ms(config.reset_hold_after_power_ms);
        let result = self.io.set_reset(true);
        self.interface(result)?;
        self.io.delay_ms(config.post_reset_release_ms);
        self.wait_ready(config.busy_wait)?;
        self.status.state = DriverState::Ready;
        Ok(())
    }

    /// Apply the archived NOTE4C vendor sequence `0xE9, 0x01`.
    ///
    /// `0xE9` is not documented in the SSD2683 Rev 0.20 proposal. This method is
    /// deliberately separate and narrowly named; it is not a general raw-command API.
    pub fn apply_reference_vendor_compatibility(&mut self) -> Result<(), DriverError<I::Error>> {
        self.require_state(DriverState::Ready)?;
        self.command_with_data(
            REFERENCE_VENDOR_COMPAT_COMMAND,
            &[REFERENCE_VENDOR_COMPAT_DATA],
        )
    }

    pub fn load_full_frame(
        &mut self,
        frame: &[u8; FRAMEBUFFER_LEN],
        busy_wait: BusyWait,
    ) -> Result<(), DriverError<I::Error>> {
        self.require_state(DriverState::Ready)?;
        self.command(Command::DataTransmission)?;
        self.wait_ready(busy_wait)?;
        for chunk in frame.chunks(FRAME_WRITE_CHUNK) {
            let result = self.io.write_data(chunk);
            self.interface(result)?;
        }
        self.status.state = DriverState::FrameLoaded;
        Ok(())
    }

    pub fn refresh_loaded_frame(
        &mut self,
        config: OperationConfig,
    ) -> Result<(), DriverError<I::Error>> {
        self.require_state(DriverState::FrameLoaded)?;

        // PON may have reached the controller even if the SPI transaction later
        // reports an error (for example while releasing CS). Conservatively mark
        // high voltage as possible before sending it so failure cleanup issues POF.
        self.status.state = DriverState::PanelPowered;
        self.command(Command::PowerOn)?;
        self.wait_ready(config.busy_wait)?;
        self.io.delay_ms(config.post_power_on_ready_ms);

        // Apply the same conservative state rule to DRF. The reference firmware
        // waits 10 ms after the command/data pair before its first BUSY_N sample.
        self.status.state = DriverState::Refreshing;
        self.command_with_data(Command::DisplayRefresh.as_u8(), &[DISPLAY_REFRESH_DATA])?;
        self.io.delay_ms(config.post_refresh_command_ms);
        self.wait_ready(config.busy_wait)?;
        self.status.state = DriverState::PanelPowered;
        Ok(())
    }

    /// Gracefully turn high voltages off, enter deep sleep, assert reset, and
    /// finally cut the EPD rail.
    pub fn shutdown(&mut self, config: OperationConfig) -> Result<(), DriverError<I::Error>> {
        self.require_state(DriverState::PanelPowered)?;
        self.command_with_data(Command::PowerOff.as_u8(), &[POWER_OFF_DATA])?;
        self.status.state = DriverState::PanelPoweredOff;
        self.wait_ready(config.busy_wait)?;
        self.io.delay_ms(config.post_power_off_ms);
        self.command_with_data(Command::DeepSleep.as_u8(), &[DEEP_SLEEP_CHECK])?;
        self.status.state = DriverState::DeepSleep;
        let result = self.io.set_bus_idle();
        self.interface(result)?;
        let result = self.io.set_reset(false);
        self.interface(result)?;
        let result = self.io.set_power(false);
        self.interface(result)?;
        self.status = DriverStatus {
            state: DriverState::PoweredOff,
            faulted: false,
        };
        Ok(())
    }

    /// Complete one full refresh and guarantee a bounded cleanup attempt.
    ///
    /// On any failure the returned value includes the original cause and every
    /// cleanup failure. The rail-off operation is always attempted.
    pub fn run_full_refresh(
        &mut self,
        frame: &[u8; FRAMEBUFFER_LEN],
        config: OperationConfig,
        apply_vendor_compatibility: bool,
    ) -> Result<(), RefreshFailure<I::Error>> {
        let result = self.run_full_refresh_inner(frame, config, apply_vendor_compatibility);
        match result {
            Ok(()) => Ok(()),
            Err(cause) => {
                let cleanup = self.cleanup_after_error(&cause, config);
                Err(RefreshFailure { cause, cleanup })
            }
        }
    }

    /// Run the SSD2683 proposal's PTLW sequence as an experimental protocol probe.
    ///
    /// This still sends all 30,000 framebuffer bytes through DTM. PTLW only
    /// limits the subsequent refresh window; no short-window DTM, LUT or MTP
    /// operation is exposed. The archived NOTE4C four-color firmware forces full
    /// refresh and does not validate this sequence for the physical panel.
    #[cfg(feature = "experimental-partial-refresh")]
    pub fn run_experimental_partial_refresh(
        &mut self,
        frame: &[u8; FRAMEBUFFER_LEN],
        window: PartialWindow,
        config: OperationConfig,
        apply_vendor_compatibility: bool,
    ) -> Result<(), RefreshFailure<I::Error>> {
        let result = self.run_experimental_partial_refresh_inner(
            frame,
            window,
            config,
            apply_vendor_compatibility,
        );
        match result {
            Ok(()) => Ok(()),
            Err(cause) => {
                let cleanup = self.cleanup_after_error(&cause, config);
                Err(RefreshFailure { cause, cleanup })
            }
        }
    }

    /// Enable PTLW for a validated NOTE4C window.
    #[cfg(feature = "experimental-partial-refresh")]
    pub fn enable_experimental_partial_window(
        &mut self,
        window: PartialWindow,
    ) -> Result<(), DriverError<I::Error>> {
        self.require_state(DriverState::FrameLoaded)?;
        self.command_with_data(Command::PartialWindow.as_u8(), &window.encode(true))
    }

    /// Disable PTLW after the experimental refresh, before normal shutdown.
    #[cfg(feature = "experimental-partial-refresh")]
    pub fn disable_experimental_partial_window(
        &mut self,
        window: PartialWindow,
    ) -> Result<(), DriverError<I::Error>> {
        self.require_state(DriverState::PanelPowered)?;
        self.command_with_data(Command::PartialWindow.as_u8(), &window.encode(false))
    }

    /// Clean up according to the failure that ended the operation.
    ///
    /// The proposal states that commands are ignored while BUSY_N is low. A
    /// BUSY timeout therefore skips POF/DSLP and immediately attempts bus-idle,
    /// reset-low and rail-off independently, avoiding another full timeout.
    /// Interface errors remain ambiguous, so active-state cleanup conservatively
    /// attempts POF before cutting the rail.
    pub fn cleanup_after_error(
        &mut self,
        cause: &DriverError<I::Error>,
        config: OperationConfig,
    ) -> CleanupReport<I::Error> {
        self.cleanup_after_failure_inner(config, !matches!(cause, DriverError::BusyTimeout { .. }))
    }

    /// Best-effort bounded cleanup callable after any failed low-level step.
    ///
    /// If high voltage may be active, this first attempts documented POF and a
    /// bounded BUSY wait. It then attempts DSLP, bus-idle, reset-low, and rail-off
    /// independently, so one failed operation cannot suppress the final rail cut.
    /// Call [`Self::cleanup_after_error`] instead when the cause is known, so a
    /// BUSY timeout can skip commands the controller would ignore.
    pub fn cleanup_after_failure(&mut self, config: OperationConfig) -> CleanupReport<I::Error> {
        self.cleanup_after_failure_inner(config, true)
    }

    fn cleanup_after_failure_inner(
        &mut self,
        config: OperationConfig,
        controller_commands_allowed: bool,
    ) -> CleanupReport<I::Error> {
        let may_have_high_voltage = matches!(
            self.status.state,
            DriverState::PanelPowered | DriverState::Refreshing
        );

        let graceful_power_off_attempted = may_have_high_voltage && controller_commands_allowed;
        let graceful_power_off = if graceful_power_off_attempted {
            match self.command_with_data_untracked(Command::PowerOff.as_u8(), &[POWER_OFF_DATA]) {
                Ok(()) => {
                    self.status.state = DriverState::PanelPoweredOff;
                    self.wait_ready_untracked(config.busy_wait).err()
                }
                Err(error) => Some(error),
            }
        } else {
            None
        };

        let graceful_power_off_succeeded =
            graceful_power_off_attempted && graceful_power_off.is_none();
        let known_ready_without_high_voltage = !self.status.faulted
            && matches!(
                self.status.state,
                DriverState::Ready | DriverState::FrameLoaded | DriverState::PanelPoweredOff
            );
        let deep_sleep_attempted = controller_commands_allowed
            && (graceful_power_off_succeeded || known_ready_without_high_voltage);
        let deep_sleep = if deep_sleep_attempted {
            self.io.delay_ms(config.post_power_off_ms);
            match self.command_with_data_untracked(Command::DeepSleep.as_u8(), &[DEEP_SLEEP_CHECK])
            {
                Ok(()) => {
                    self.status.state = DriverState::DeepSleep;
                    None
                }
                Err(error) => Some(error),
            }
        } else {
            None
        };

        let bus_idle = self.io.set_bus_idle().err();
        let reset_low = self.io.set_reset(false).err();
        let rail_off = self.io.set_power(false).err();
        let rail_is_off = rail_off.is_none();

        let report = CleanupReport {
            graceful_power_off_attempted,
            graceful_power_off,
            deep_sleep_attempted,
            deep_sleep,
            bus_idle,
            reset_low,
            rail_off,
        };
        self.status = DriverStatus {
            state: if rail_is_off {
                DriverState::PoweredOff
            } else {
                self.status.state
            },
            faulted: !report.is_clean(),
        };
        report
    }

    /// Poll active-low BUSY_N no more than `max_polls` times.
    pub fn wait_ready(&mut self, busy_wait: BusyWait) -> Result<(), DriverError<I::Error>> {
        match self.wait_ready_untracked(busy_wait) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.status.faulted = true;
                Err(error)
            }
        }
    }

    fn run_full_refresh_inner(
        &mut self,
        frame: &[u8; FRAMEBUFFER_LEN],
        config: OperationConfig,
        apply_vendor_compatibility: bool,
    ) -> Result<(), DriverError<I::Error>> {
        self.initialize(config)?;
        if apply_vendor_compatibility {
            self.apply_reference_vendor_compatibility()?;
        }
        self.load_full_frame(frame, config.busy_wait)?;
        self.refresh_loaded_frame(config)?;
        self.shutdown(config)
    }

    #[cfg(feature = "experimental-partial-refresh")]
    fn run_experimental_partial_refresh_inner(
        &mut self,
        frame: &[u8; FRAMEBUFFER_LEN],
        window: PartialWindow,
        config: OperationConfig,
        apply_vendor_compatibility: bool,
    ) -> Result<(), DriverError<I::Error>> {
        self.initialize(config)?;
        if apply_vendor_compatibility {
            self.apply_reference_vendor_compatibility()?;
        }
        self.load_full_frame(frame, config.busy_wait)?;
        self.enable_experimental_partial_window(window)?;
        self.refresh_loaded_frame(config)?;
        self.disable_experimental_partial_window(window)?;
        self.shutdown(config)
    }

    fn wait_ready_untracked(&mut self, busy_wait: BusyWait) -> Result<(), DriverError<I::Error>> {
        if busy_wait.max_polls == 0 {
            return Err(DriverError::BusyTimeout { polls: 0 });
        }

        for poll in 1..=busy_wait.max_polls {
            let busy = self.io.busy_is_low().map_err(DriverError::Interface)?;
            if !busy {
                return Ok(());
            }
            if poll < busy_wait.max_polls {
                self.io.delay_ms(busy_wait.poll_interval_ms);
            }
        }
        Err(DriverError::BusyTimeout {
            polls: busy_wait.max_polls,
        })
    }

    fn command(&mut self, command: Command) -> Result<(), DriverError<I::Error>> {
        let result = self.io.write_command(command.as_u8());
        self.interface(result)
    }

    fn command_with_data(&mut self, command: u8, data: &[u8]) -> Result<(), DriverError<I::Error>> {
        match self.command_with_data_untracked(command, data) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.status.faulted = true;
                Err(error)
            }
        }
    }

    fn command_with_data_untracked(
        &mut self,
        command: u8,
        data: &[u8],
    ) -> Result<(), DriverError<I::Error>> {
        self.io
            .write_command(command)
            .map_err(DriverError::Interface)?;
        self.io.write_data(data).map_err(DriverError::Interface)
    }

    fn require_state(&self, expected: DriverState) -> Result<(), DriverError<I::Error>> {
        if self.status.state == expected {
            Ok(())
        } else {
            Err(DriverError::InvalidState {
                expected,
                actual: self.status.state,
            })
        }
    }

    fn interface<T>(&mut self, result: Result<T, I::Error>) -> Result<T, DriverError<I::Error>> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.status.faulted = true;
                Err(DriverError::Interface(error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Event {
        BusIdle,
        Reset(bool),
        Power(bool),
        Command(u8),
        Data { len: usize, first: u8, last: u8 },
        BusyRead,
        Delay(u32),
    }

    #[derive(Debug, Eq, PartialEq)]
    enum MockError {
        Command(u8),
    }

    struct MockInterface {
        events: Vec<Event>,
        busy_low: bool,
        busy_from_read: Option<usize>,
        busy_reads: usize,
        fail_command_once: Option<u8>,
    }

    impl MockInterface {
        fn ready() -> Self {
            Self {
                events: Vec::new(),
                busy_low: false,
                busy_from_read: None,
                busy_reads: 0,
                fail_command_once: None,
            }
        }

        fn busy() -> Self {
            Self {
                events: Vec::new(),
                busy_low: true,
                busy_from_read: None,
                busy_reads: 0,
                fail_command_once: None,
            }
        }

        fn busy_during_refresh() -> Self {
            Self {
                events: Vec::new(),
                busy_low: false,
                busy_from_read: Some(4),
                busy_reads: 0,
                fail_command_once: None,
            }
        }

        fn fail_command_once(command: u8) -> Self {
            Self {
                events: Vec::new(),
                busy_low: false,
                busy_from_read: None,
                busy_reads: 0,
                fail_command_once: Some(command),
            }
        }
    }

    impl DisplayInterface for MockInterface {
        type Error = MockError;

        fn set_bus_idle(&mut self) -> Result<(), Self::Error> {
            self.events.push(Event::BusIdle);
            Ok(())
        }

        fn set_reset(&mut self, high: bool) -> Result<(), Self::Error> {
            self.events.push(Event::Reset(high));
            Ok(())
        }

        fn set_power(&mut self, enabled: bool) -> Result<(), Self::Error> {
            self.events.push(Event::Power(enabled));
            Ok(())
        }

        fn write_command(&mut self, command: u8) -> Result<(), Self::Error> {
            self.events.push(Event::Command(command));
            if self.fail_command_once == Some(command) {
                self.fail_command_once = None;
                return Err(MockError::Command(command));
            }
            Ok(())
        }

        fn write_data(&mut self, data: &[u8]) -> Result<(), Self::Error> {
            self.events.push(Event::Data {
                len: data.len(),
                first: data[0],
                last: data[data.len() - 1],
            });
            Ok(())
        }

        fn busy_is_low(&mut self) -> Result<bool, Self::Error> {
            self.events.push(Event::BusyRead);
            self.busy_reads += 1;
            Ok(self.busy_low
                || self
                    .busy_from_read
                    .is_some_and(|first_busy_read| self.busy_reads >= first_busy_read))
        }

        fn delay_ms(&mut self, milliseconds: u32) {
            self.events.push(Event::Delay(milliseconds));
        }
    }

    fn test_rates() -> SpiRates {
        SpiRates::new(10_000_000, Some(2_000_000)).unwrap()
    }

    fn test_config() -> OperationConfig {
        OperationConfig {
            reset_hold_after_power_ms: 20,
            post_reset_release_ms: 50,
            post_power_on_ready_ms: 10,
            post_refresh_command_ms: 10,
            post_power_off_ms: 20,
            busy_wait: BusyWait::new(3, 1).unwrap(),
        }
    }

    #[test]
    fn rates_reject_proposal_violations() {
        assert_eq!(
            SpiRates::new(40_000_000, None),
            Err(SpiRateError::InvalidWriteRate {
                requested: 40_000_000
            })
        );
        assert_eq!(
            SpiRates::new(10_000_000, Some(8_000_000)),
            Err(SpiRateError::InvalidReadRate {
                requested: 8_000_000
            })
        );
        assert!(SpiRates::new(20_000_000, Some(2_500_000)).is_ok());
    }

    #[test]
    fn full_refresh_uses_only_expected_command_order() {
        let frame = [0x1B; FRAMEBUFFER_LEN];
        let mut driver = Ssd2683::new(MockInterface::ready(), test_rates());
        driver
            .run_full_refresh(&frame, test_config(), true)
            .unwrap();
        assert_eq!(driver.status().state, DriverState::PoweredOff);

        let io = driver.release();
        let commands: Vec<u8> = io
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Command(command) => Some(*command),
                _ => None,
            })
            .collect();
        assert_eq!(commands, [0xE9, 0x10, 0x04, 0x12, 0x02, 0x07]);
        assert!(io.events.starts_with(&[
            Event::BusIdle,
            Event::Reset(false),
            Event::Power(true),
            Event::Delay(20),
            Event::Reset(true),
            Event::Delay(50),
            Event::BusyRead,
        ]));

        let data_lengths: Vec<usize> = io
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Data { len, .. } => Some(*len),
                _ => None,
            })
            .collect();
        assert_eq!(
            data_lengths,
            [1, 4_000, 4_000, 4_000, 4_000, 4_000, 4_000, 4_000, 2_000, 1, 1, 1]
        );
        let pon_index = io
            .events
            .iter()
            .position(|event| *event == Event::Command(0x04))
            .unwrap();
        assert_eq!(
            &io.events[pon_index..pon_index + 7],
            [
                Event::Command(0x04),
                Event::BusyRead,
                Event::Delay(10),
                Event::Command(0x12),
                Event::Data {
                    len: 1,
                    first: 0,
                    last: 0,
                },
                Event::Delay(10),
                Event::BusyRead,
            ]
        );
        assert!(io
            .events
            .ends_with(&[Event::BusIdle, Event::Reset(false), Event::Power(false)]));
    }

    #[test]
    fn busy_wait_is_active_low_and_bounded() {
        let mut driver = Ssd2683::new(MockInterface::busy(), test_rates());
        let error = driver.wait_ready(BusyWait::new(3, 7).unwrap()).unwrap_err();
        assert_eq!(error, DriverError::BusyTimeout { polls: 3 });
        assert!(driver.status().faulted);

        let io = driver.release();
        assert_eq!(
            io.events,
            [
                Event::BusyRead,
                Event::Delay(7),
                Event::BusyRead,
                Event::Delay(7),
                Event::BusyRead
            ]
        );
    }

    #[test]
    fn cleanup_from_active_panel_is_graceful_then_cuts_rail() {
        let frame = [0x55; FRAMEBUFFER_LEN];
        let config = test_config();
        let mut driver = Ssd2683::new(MockInterface::ready(), test_rates());
        driver.initialize(config).unwrap();
        driver.load_full_frame(&frame, config.busy_wait).unwrap();
        driver.refresh_loaded_frame(config).unwrap();
        assert_eq!(driver.status().state, DriverState::PanelPowered);

        let report = driver.cleanup_after_failure(config);
        assert!(report.is_clean());
        assert_eq!(driver.status().state, DriverState::PoweredOff);

        let io = driver.release();
        let tail = &io.events[io.events.len() - 9..];
        assert_eq!(
            tail,
            [
                Event::Command(0x02),
                Event::Data {
                    len: 1,
                    first: 0,
                    last: 0,
                },
                Event::BusyRead,
                Event::Delay(20),
                Event::Command(0x07),
                Event::Data {
                    len: 1,
                    first: 0xA5,
                    last: 0xA5,
                },
                Event::BusIdle,
                Event::Reset(false),
                Event::Power(false)
            ]
        );
    }

    #[test]
    fn failed_refresh_returns_bounded_cleanup_report() {
        let frame = [0x00; FRAMEBUFFER_LEN];
        let config = test_config();
        let mut driver = Ssd2683::new(MockInterface::busy(), test_rates());
        let failure = driver.run_full_refresh(&frame, config, false).unwrap_err();
        assert_eq!(failure.cause, DriverError::BusyTimeout { polls: 3 });
        assert!(failure.cleanup.is_clean());
        assert_eq!(driver.status().state, DriverState::PoweredOff);
        let io = driver.release();
        assert!(io
            .events
            .ends_with(&[Event::BusIdle, Event::Reset(false), Event::Power(false)]));
    }

    #[test]
    fn cleanup_from_powered_off_still_drives_all_safety_lines() {
        let mut driver = Ssd2683::new(MockInterface::ready(), test_rates());
        let report = driver.cleanup_after_failure(test_config());
        assert!(report.is_clean());
        let io = driver.release();
        assert_eq!(
            io.events,
            [Event::BusIdle, Event::Reset(false), Event::Power(false)]
        );
    }

    #[test]
    fn pon_or_drf_spi_error_still_attempts_graceful_power_off() {
        let frame = [0x55; FRAMEBUFFER_LEN];
        let config = test_config();

        for failed_command in [0x04, 0x12] {
            let mut driver = Ssd2683::new(
                MockInterface::fail_command_once(failed_command),
                test_rates(),
            );
            let failure = driver.run_full_refresh(&frame, config, false).unwrap_err();
            assert_eq!(
                failure.cause,
                DriverError::Interface(MockError::Command(failed_command))
            );
            assert!(failure.cleanup.graceful_power_off.is_none());
            assert!(failure.cleanup.graceful_power_off_attempted);

            let io = driver.release();
            let commands: Vec<u8> = io
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::Command(command) => Some(*command),
                    _ => None,
                })
                .collect();
            let failure_index = commands
                .iter()
                .position(|command| *command == failed_command)
                .unwrap();
            assert_eq!(commands[failure_index + 1], 0x02);
            assert!(io.events.ends_with(&[
                Event::BusIdle,
                Event::Reset(false),
                Event::Power(false),
            ]));
        }
    }

    #[test]
    fn drf_busy_timeout_skips_commands_and_cuts_rail_immediately() {
        let frame = [0x55; FRAMEBUFFER_LEN];
        let config = test_config();
        let mut driver = Ssd2683::new(MockInterface::busy_during_refresh(), test_rates());
        let failure = driver.run_full_refresh(&frame, config, false).unwrap_err();

        assert_eq!(failure.cause, DriverError::BusyTimeout { polls: 3 });
        assert!(!failure.cleanup.graceful_power_off_attempted);
        assert!(!failure.cleanup.deep_sleep_attempted);
        assert!(failure.cleanup.is_clean());

        let io = driver.release();
        let commands: Vec<u8> = io
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Command(command) => Some(*command),
                _ => None,
            })
            .collect();
        assert_eq!(commands, [0x10, 0x04, 0x12]);
        assert!(io
            .events
            .ends_with(&[Event::BusIdle, Event::Reset(false), Event::Power(false),]));
    }

    #[cfg(feature = "experimental-partial-refresh")]
    #[test]
    fn experimental_partial_refresh_sends_full_dtm_and_brackets_refresh_with_ptlw() {
        let frame = [0x55; FRAMEBUFFER_LEN];
        let window = PartialWindow::new(160, 239, 110, 189).unwrap();
        let mut driver = Ssd2683::new(MockInterface::ready(), test_rates());
        driver
            .run_experimental_partial_refresh(&frame, window, test_config(), true)
            .unwrap();

        let io = driver.release();
        let commands: Vec<u8> = io
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Command(command) => Some(*command),
                _ => None,
            })
            .collect();
        assert_eq!(commands, [0xE9, 0x10, 0x83, 0x04, 0x12, 0x83, 0x02, 0x07]);

        let data: Vec<(usize, u8, u8)> = io
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Data { len, first, last } => Some((*len, *first, *last)),
                _ => None,
            })
            .collect();
        assert_eq!(
            data,
            [
                (1, 0x01, 0x01),
                (4_000, 0x55, 0x55),
                (4_000, 0x55, 0x55),
                (4_000, 0x55, 0x55),
                (4_000, 0x55, 0x55),
                (4_000, 0x55, 0x55),
                (4_000, 0x55, 0x55),
                (4_000, 0x55, 0x55),
                (2_000, 0x55, 0x55),
                (9, 0x00, 0x01),
                (1, 0x00, 0x00),
                (9, 0x00, 0x00),
                (1, 0x00, 0x00),
                (1, 0xA5, 0xA5),
            ]
        );
    }
    #[cfg(feature = "experimental-partial-refresh")]
    #[test]
    fn partial_command_errors_and_busy_timeout_always_cut_power() {
        let frame = [0x1b; FRAMEBUFFER_LEN];
        let window = PartialWindow::new(0, 3, 299, 299).unwrap();
        for failed in [0x83, 0x04, 0x12, 0x02, 0x07] {
            let mut driver = Ssd2683::new(MockInterface::fail_command_once(failed), test_rates());
            assert!(driver
                .run_experimental_partial_refresh(&frame, window, test_config(), true)
                .is_err());
            assert!(driver.release().events.ends_with(&[
                Event::BusIdle,
                Event::Reset(false),
                Event::Power(false)
            ]));
        }
        let mut driver = Ssd2683::new(MockInterface::busy_during_refresh(), test_rates());
        assert!(driver
            .run_experimental_partial_refresh(&frame, window, test_config(), true)
            .is_err());
        let io = driver.release();
        let commands: Vec<_> = io
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Command(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert_eq!(commands, [0xe9, 0x10, 0x83, 0x04, 0x12]);
        assert!(io
            .events
            .ends_with(&[Event::BusIdle, Event::Reset(false), Event::Power(false)]));
    }
}
