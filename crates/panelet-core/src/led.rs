// SPDX-License-Identifier: MPL-2.0

use crate::TestOutcome;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LedMode {
    Idle,
    Pattern {
        on_ticks: u32,
        off_ticks: u32,
        pulses_per_round: u8,
        rounds: u8,
        round_gap_ticks: u32,
        tick: u32,
    },
    Running,
    Armed {
        tick: u32,
    },
}

/// Non-blocking, one-tick-at-a-time LED feedback state machine.
///
/// The bring-up runner calls [`Self::tick`] every 20 ms, so button scanning is
/// never delayed by a pulse pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LedController {
    mode: LedMode,
}

impl LedController {
    pub const fn new(_safe_mode: bool) -> Self {
        Self {
            mode: LedMode::Idle,
        }
    }

    pub fn show_selection(&mut self, menu_number: u8) {
        self.mode = LedMode::Pattern {
            on_ticks: 4,
            off_ticks: 4,
            pulses_per_round: menu_number.max(1),
            rounds: 1,
            round_gap_ticks: 0,
            tick: 0,
        };
    }

    pub fn show_running(&mut self) {
        self.mode = LedMode::Running;
    }

    pub fn show_armed(&mut self) {
        self.mode = LedMode::Armed { tick: 0 };
    }

    pub fn show_finished(&mut self, outcome: TestOutcome) {
        let (on_ticks, off_ticks, pulses_per_round, rounds, round_gap_ticks) = match outcome {
            TestOutcome::Pass | TestOutcome::Observed => (4, 4, 2, 1, 0),
            TestOutcome::Inconclusive => (10, 8, 3, 1, 0),
            TestOutcome::Skipped | TestOutcome::Aborted => (25, 12, 1, 1, 0),
            TestOutcome::Fail => (25, 12, 3, 2, 25),
        };
        self.mode = LedMode::Pattern {
            on_ticks,
            off_ticks,
            pulses_per_round,
            rounds,
            round_gap_ticks,
            tick: 0,
        };
    }

    /// Advance one 20 ms application tick and return the desired logical LED state.
    pub fn tick(&mut self) -> bool {
        let result = match self.mode {
            LedMode::Idle => false,
            LedMode::Running => true,
            LedMode::Armed { tick } => {
                self.mode = LedMode::Armed {
                    tick: tick.wrapping_add(1),
                };
                tick % 50 < 25
            }
            LedMode::Pattern {
                on_ticks,
                off_ticks,
                pulses_per_round,
                rounds,
                round_gap_ticks,
                tick,
            } => {
                let pulse_span = on_ticks + off_ticks;
                let round_span = pulse_span * u32::from(pulses_per_round);
                let total = round_span * u32::from(rounds)
                    + round_gap_ticks * u32::from(rounds.saturating_sub(1));
                if tick >= total {
                    self.mode = LedMode::Idle;
                    false
                } else {
                    let block_span = round_span + round_gap_ticks;
                    let within_block = tick % block_span;
                    let on = within_block < round_span && within_block % pulse_span < on_ticks;
                    self.mode = LedMode::Pattern {
                        on_ticks,
                        off_ticks,
                        pulses_per_round,
                        rounds,
                        round_gap_ticks,
                        tick: tick + 1,
                    };
                    on
                }
            }
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(controller: &mut LedController, count: usize) -> std::vec::Vec<bool> {
        (0..count).map(|_| controller.tick()).collect()
    }

    #[test]
    fn selection_pulses_match_one_based_menu_number() {
        let mut led = LedController::new(false);
        led.show_selection(3);
        let states = collect(&mut led, 24);
        assert_eq!(states.iter().filter(|state| **state).count(), 12);
        assert!(states[0..4].iter().all(|state| *state));
        assert!(states[4..8].iter().all(|state| !*state));
    }

    #[test]
    fn running_and_armed_are_distinct_non_blocking_states() {
        let mut led = LedController::new(false);
        led.show_running();
        assert!(collect(&mut led, 60).iter().all(|state| *state));

        led.show_armed();
        let states = collect(&mut led, 50);
        assert!(states[0..25].iter().all(|state| *state));
        assert!(states[25..50].iter().all(|state| !*state));
    }

    #[test]
    fn completion_patterns_have_required_shape() {
        let mut led = LedController::new(false);
        led.show_finished(TestOutcome::Pass);
        assert_eq!(collect(&mut led, 16).iter().filter(|s| **s).count(), 8);

        led.show_finished(TestOutcome::Inconclusive);
        assert_eq!(collect(&mut led, 54).iter().filter(|s| **s).count(), 30);

        led.show_finished(TestOutcome::Skipped);
        assert_eq!(collect(&mut led, 37).iter().filter(|s| **s).count(), 25);

        led.show_finished(TestOutcome::Fail);
        let fail = collect(&mut led, 247);
        assert_eq!(fail.iter().filter(|s| **s).count(), 150);
    }

    #[test]
    fn idle_led_is_off_and_completion_feedback_always_ends() {
        for safe_mode in [false, true] {
            let mut led = LedController::new(safe_mode);
            assert!(collect(&mut led, 100).iter().all(|state| !state));
            for outcome in [TestOutcome::Pass, TestOutcome::Fail] {
                led.show_finished(outcome);
                collect(&mut led, 260);
                assert!(collect(&mut led, 100).iter().all(|state| !state));
            }
        }
    }
}
