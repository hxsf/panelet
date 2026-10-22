// SPDX-License-Identifier: MPL-2.0

#![no_std]

#[cfg(test)]
extern crate std;

pub mod input;
mod led;
mod lunar;
mod rtc;

pub use led::LedController;
pub use lunar::{solar_to_lunar, LunarConversionError, LunarDate};
pub use rtc::{
    civil_to_unix_seconds, parse_pcf8563, pcf8563_utc_to_unix_seconds, rtc_progress,
    unix_seconds_to_civil, unix_seconds_to_pcf8563_utc, CivilTime, RtcField, RtcParseError,
    RtcProgress, RtcSample, TimeConversionError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button {
    Up,
    Down,
    Confirm,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ButtonLevels {
    pub up: bool,
    pub down: bool,
    pub confirm: bool,
}

impl ButtonLevels {
    pub const fn pressed(self, button: Button) -> bool {
        match button {
            Button::Up => self.up,
            Button::Down => self.down,
            Button::Confirm => self.confirm,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ButtonEventKind {
    Pressed,
    Released,
    LongPressed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ButtonEvent {
    pub button: Button,
    pub kind: ButtonEventKind,
}

/// A sample-count based debouncer with a one-shot long-press event.
///
/// The caller owns the clock: with a 20 ms sampling interval, the bring-up
/// firmware uses 3 stable samples (60 ms) and 50 held samples (1 s).
#[derive(Clone, Copy, Debug)]
pub struct Debouncer {
    stable: bool,
    candidate: bool,
    candidate_samples: u8,
    stable_samples_required: u8,
    held_samples: u16,
    long_press_samples: u16,
    long_press_reported: bool,
}

impl Debouncer {
    pub const fn new(stable_samples_required: u8, long_press_samples: u16) -> Self {
        Self {
            stable: false,
            candidate: false,
            candidate_samples: 0,
            stable_samples_required,
            held_samples: 0,
            long_press_samples,
            long_press_reported: false,
        }
    }

    pub fn update(&mut self, raw_pressed: bool) -> Option<ButtonEventKind> {
        if raw_pressed == self.candidate {
            self.candidate_samples = self.candidate_samples.saturating_add(1);
        } else {
            self.candidate = raw_pressed;
            self.candidate_samples = 1;
        }

        if self.candidate != self.stable
            && self.candidate_samples >= self.stable_samples_required.max(1)
        {
            self.stable = self.candidate;
            self.held_samples = 0;
            self.long_press_reported = false;
            return Some(if self.stable {
                ButtonEventKind::Pressed
            } else {
                ButtonEventKind::Released
            });
        }

        if self.stable {
            self.held_samples = self.held_samples.saturating_add(1);
            if !self.long_press_reported
                && self.long_press_samples > 0
                && self.held_samples >= self.long_press_samples
            {
                self.long_press_reported = true;
                return Some(ButtonEventKind::LongPressed);
            }
        }

        None
    }

    pub const fn is_pressed(&self) -> bool {
        self.stable
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestId {
    SystemInfo,
    ButtonsAndLed,
    EpdQuick,
    EpdPartialExperimental,
    I2cScan,
    RtcRead,
    Battery,
    StatusScreen,
}

impl TestId {
    pub const ALL: [Self; 8] = [
        Self::SystemInfo,
        Self::ButtonsAndLed,
        Self::EpdQuick,
        Self::EpdPartialExperimental,
        Self::I2cScan,
        Self::RtcRead,
        Self::Battery,
        Self::StatusScreen,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::SystemInfo => "SYS",
            Self::ButtonsAndLed => "KEYS_LED",
            Self::EpdQuick => "EPD_QUICK",
            Self::EpdPartialExperimental => "EPD_PARTIAL_EXP",
            Self::I2cScan => "I2C_SCAN",
            Self::RtcRead => "RTC_READ",
            Self::Battery => "BATTERY",
            Self::StatusScreen => "STATUS_SCREEN",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::SystemInfo => 0,
            Self::ButtonsAndLed => 1,
            Self::EpdQuick => 2,
            Self::EpdPartialExperimental => 3,
            Self::I2cScan => 4,
            Self::RtcRead => 5,
            Self::Battery => 6,
            Self::StatusScreen => 7,
        }
    }

    pub const fn menu_number(self) -> u8 {
        self.index() as u8 + 1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestOutcome {
    Pass,
    Fail,
    Observed,
    Inconclusive,
    Skipped,
    Aborted,
}

/// Volatile per-boot test outcomes. No persistence backend is exposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionResults {
    outcomes: [Option<TestOutcome>; TestId::ALL.len()],
}

impl SessionResults {
    pub const fn new() -> Self {
        Self {
            outcomes: [None; TestId::ALL.len()],
        }
    }

    pub fn record(&mut self, test: TestId, outcome: TestOutcome) {
        self.outcomes[test.index()] = Some(outcome);
    }

    pub const fn get(&self, test: TestId) -> Option<TestOutcome> {
        self.outcomes[test.index()]
    }
}

impl Default for SessionResults {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestStatus {
    Idle,
    ArmedWaitingRelease(TestId),
    ArmedReady(TestId),
    Running(TestId),
    Finished { test: TestId, outcome: TestOutcome },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    None,
    SelectionChanged(TestId),
    ArmStarted(TestId),
    ArmReady(TestId),
    ArmCancelled { test: TestId, selected: TestId },
    Run(TestId),
    AbortRequested(TestId),
}

#[derive(Clone, Copy, Debug)]
pub struct TestMenu {
    selected: usize,
    status: TestStatus,
}

impl Default for TestMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl TestMenu {
    pub const fn new() -> Self {
        Self {
            selected: 0,
            status: TestStatus::Idle,
        }
    }

    pub const fn selected(&self) -> TestId {
        TestId::ALL[self.selected]
    }

    pub const fn status(&self) -> TestStatus {
        self.status
    }

    pub fn handle(&mut self, event: ButtonEvent) -> MenuAction {
        match (event.button, event.kind, self.status) {
            (
                button @ (Button::Up | Button::Down),
                ButtonEventKind::Pressed,
                TestStatus::ArmedWaitingRelease(test) | TestStatus::ArmedReady(test),
            ) => {
                self.status = TestStatus::Idle;
                self.move_selection(button);
                MenuAction::ArmCancelled {
                    test,
                    selected: self.selected(),
                }
            }
            (Button::Up, ButtonEventKind::Pressed, TestStatus::Idle) => {
                self.move_selection(Button::Up);
                MenuAction::SelectionChanged(self.selected())
            }
            (Button::Down, ButtonEventKind::Pressed, TestStatus::Idle) => {
                self.move_selection(Button::Down);
                MenuAction::SelectionChanged(self.selected())
            }
            (Button::Confirm, ButtonEventKind::Pressed, TestStatus::Idle)
                if self.selected() == TestId::EpdPartialExperimental =>
            {
                MenuAction::None
            }
            (Button::Confirm, ButtonEventKind::LongPressed, TestStatus::Idle)
                if self.selected() == TestId::EpdPartialExperimental =>
            {
                let test = self.selected();
                self.status = TestStatus::ArmedWaitingRelease(test);
                MenuAction::ArmStarted(test)
            }
            (Button::Confirm, ButtonEventKind::Released, TestStatus::ArmedWaitingRelease(test)) => {
                self.status = TestStatus::ArmedReady(test);
                MenuAction::ArmReady(test)
            }
            (Button::Confirm, ButtonEventKind::LongPressed, TestStatus::ArmedReady(test)) => {
                self.status = TestStatus::Running(test);
                MenuAction::Run(test)
            }
            (Button::Confirm, ButtonEventKind::Pressed, TestStatus::Idle)
                if matches!(self.selected(), TestId::EpdQuick | TestId::StatusScreen) =>
            {
                MenuAction::None
            }
            (Button::Confirm, ButtonEventKind::LongPressed, TestStatus::Idle)
                if matches!(self.selected(), TestId::EpdQuick | TestId::StatusScreen) =>
            {
                let test = self.selected();
                self.status = TestStatus::Running(test);
                MenuAction::Run(test)
            }
            (Button::Confirm, ButtonEventKind::Pressed, TestStatus::Idle) => {
                let test = self.selected();
                self.status = TestStatus::Running(test);
                MenuAction::Run(test)
            }
            (Button::Confirm, ButtonEventKind::LongPressed, TestStatus::Running(test)) => {
                MenuAction::AbortRequested(test)
            }
            _ => MenuAction::None,
        }
    }

    fn move_selection(&mut self, button: Button) {
        self.selected = match button {
            Button::Up if self.selected == 0 => TestId::ALL.len() - 1,
            Button::Up => self.selected - 1,
            Button::Down => (self.selected + 1) % TestId::ALL.len(),
            Button::Confirm => self.selected,
        };
    }

    pub fn finish(&mut self, outcome: TestOutcome) {
        if let TestStatus::Running(test) = self.status {
            self.status = TestStatus::Finished { test, outcome };
        }
    }

    pub fn return_to_idle(&mut self) {
        self.status = TestStatus::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_rejects_short_bounce() {
        let mut debounce = Debouncer::new(3, 50);

        assert_eq!(debounce.update(true), None);
        assert_eq!(debounce.update(false), None);
        assert_eq!(debounce.update(true), None);
        assert_eq!(debounce.update(true), None);
        assert_eq!(debounce.update(true), Some(ButtonEventKind::Pressed));
        assert!(debounce.is_pressed());
    }

    #[test]
    fn long_press_is_reported_once() {
        let mut debounce = Debouncer::new(1, 2);

        assert_eq!(debounce.update(true), Some(ButtonEventKind::Pressed));
        assert_eq!(debounce.update(true), None);
        assert_eq!(debounce.update(true), Some(ButtonEventKind::LongPressed));
        assert_eq!(debounce.update(true), None);
        assert_eq!(debounce.update(false), Some(ButtonEventKind::Released));
    }

    #[test]
    fn menu_wraps_and_runs_selected_test() {
        let mut menu = TestMenu::new();
        let up = ButtonEvent {
            button: Button::Up,
            kind: ButtonEventKind::Pressed,
        };
        let confirm = ButtonEvent {
            button: Button::Confirm,
            kind: ButtonEventKind::Pressed,
        };

        assert_eq!(
            menu.handle(up),
            MenuAction::SelectionChanged(TestId::StatusScreen)
        );
        assert_eq!(
            menu.handle(up),
            MenuAction::SelectionChanged(TestId::Battery)
        );
        assert_eq!(
            menu.handle(up),
            MenuAction::SelectionChanged(TestId::RtcRead)
        );
        assert_eq!(menu.handle(confirm), MenuAction::Run(TestId::RtcRead));
        assert_eq!(menu.status(), TestStatus::Running(TestId::RtcRead));

        menu.finish(TestOutcome::Observed);
        assert_eq!(
            menu.status(),
            TestStatus::Finished {
                test: TestId::RtcRead,
                outcome: TestOutcome::Observed,
            }
        );
    }

    #[test]
    fn status_screen_is_last_and_requires_long_confirm() {
        let mut menu = TestMenu::new();
        let up = ButtonEvent {
            button: Button::Up,
            kind: ButtonEventKind::Pressed,
        };
        assert_eq!(
            menu.handle(up),
            MenuAction::SelectionChanged(TestId::StatusScreen)
        );
        assert_eq!(TestId::StatusScreen.menu_number(), 8);
        assert_eq!(
            menu.handle(ButtonEvent {
                button: Button::Confirm,
                kind: ButtonEventKind::Pressed,
            }),
            MenuAction::None
        );
        assert_eq!(
            menu.handle(ButtonEvent {
                button: Button::Confirm,
                kind: ButtonEventKind::LongPressed,
            }),
            MenuAction::Run(TestId::StatusScreen)
        );
    }

    #[test]
    fn session_results_are_ram_only_indexed_snapshots() {
        let mut results = SessionResults::new();
        assert_eq!(results.get(TestId::Battery), None);
        results.record(TestId::Battery, TestOutcome::Observed);
        results.record(TestId::RtcRead, TestOutcome::Inconclusive);
        assert_eq!(results.get(TestId::Battery), Some(TestOutcome::Observed));
        assert_eq!(
            results.get(TestId::RtcRead),
            Some(TestOutcome::Inconclusive)
        );
    }

    #[test]
    fn epd_quick_is_explicitly_reachable_from_the_menu() {
        let mut menu = TestMenu::new();
        let down = ButtonEvent {
            button: Button::Down,
            kind: ButtonEventKind::Pressed,
        };
        let confirm_pressed = ButtonEvent {
            button: Button::Confirm,
            kind: ButtonEventKind::Pressed,
        };
        let confirm_held = ButtonEvent {
            button: Button::Confirm,
            kind: ButtonEventKind::LongPressed,
        };

        assert_eq!(
            menu.handle(down),
            MenuAction::SelectionChanged(TestId::ButtonsAndLed)
        );
        assert_eq!(
            menu.handle(down),
            MenuAction::SelectionChanged(TestId::EpdQuick)
        );
        assert_eq!(menu.selected().name(), "EPD_QUICK");
        assert_eq!(menu.handle(confirm_pressed), MenuAction::None);
        assert_eq!(menu.status(), TestStatus::Idle);
        assert_eq!(menu.handle(confirm_held), MenuAction::Run(TestId::EpdQuick));
        assert_eq!(menu.status(), TestStatus::Running(TestId::EpdQuick));
    }

    #[test]
    fn experimental_partial_requires_two_holds_with_release_between() {
        let mut menu = TestMenu::new();
        let down = ButtonEvent {
            button: Button::Down,
            kind: ButtonEventKind::Pressed,
        };
        let confirm_held = ButtonEvent {
            button: Button::Confirm,
            kind: ButtonEventKind::LongPressed,
        };
        let confirm_pressed = ButtonEvent {
            button: Button::Confirm,
            kind: ButtonEventKind::Pressed,
        };
        let confirm_released = ButtonEvent {
            button: Button::Confirm,
            kind: ButtonEventKind::Released,
        };

        assert!(matches!(
            menu.handle(down),
            MenuAction::SelectionChanged(TestId::ButtonsAndLed)
        ));
        assert!(matches!(
            menu.handle(down),
            MenuAction::SelectionChanged(TestId::EpdQuick)
        ));
        assert!(matches!(
            menu.handle(down),
            MenuAction::SelectionChanged(TestId::EpdPartialExperimental)
        ));
        assert_eq!(menu.handle(confirm_pressed), MenuAction::None);
        assert_eq!(menu.status(), TestStatus::Idle);
        assert_eq!(
            menu.handle(confirm_held),
            MenuAction::ArmStarted(TestId::EpdPartialExperimental)
        );
        assert_eq!(
            menu.status(),
            TestStatus::ArmedWaitingRelease(TestId::EpdPartialExperimental)
        );
        assert_eq!(menu.handle(confirm_held), MenuAction::None);
        assert_eq!(
            menu.handle(confirm_released),
            MenuAction::ArmReady(TestId::EpdPartialExperimental)
        );
        assert_eq!(
            menu.status(),
            TestStatus::ArmedReady(TestId::EpdPartialExperimental)
        );
        assert_eq!(
            menu.handle(confirm_held),
            MenuAction::Run(TestId::EpdPartialExperimental)
        );
        assert_eq!(
            menu.status(),
            TestStatus::Running(TestId::EpdPartialExperimental)
        );
    }

    #[test]
    fn navigation_cancels_experimental_partial_arming() {
        let mut menu = TestMenu::new();
        let down = ButtonEvent {
            button: Button::Down,
            kind: ButtonEventKind::Pressed,
        };
        let up = ButtonEvent {
            button: Button::Up,
            kind: ButtonEventKind::Pressed,
        };
        let confirm_held = ButtonEvent {
            button: Button::Confirm,
            kind: ButtonEventKind::LongPressed,
        };

        menu.handle(down);
        menu.handle(down);
        menu.handle(down);
        menu.handle(confirm_held);
        assert_eq!(
            menu.handle(up),
            MenuAction::ArmCancelled {
                test: TestId::EpdPartialExperimental,
                selected: TestId::EpdQuick,
            }
        );
        assert_eq!(menu.status(), TestStatus::Idle);
        assert_eq!(menu.selected(), TestId::EpdQuick);

        menu.handle(down);
        menu.handle(confirm_held);
        menu.handle(ButtonEvent {
            button: Button::Confirm,
            kind: ButtonEventKind::Released,
        });
        assert_eq!(
            menu.handle(up),
            MenuAction::ArmCancelled {
                test: TestId::EpdPartialExperimental,
                selected: TestId::EpdQuick,
            }
        );
        assert_eq!(menu.status(), TestStatus::Idle);
    }
}
