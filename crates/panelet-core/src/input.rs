// SPDX-License-Identifier: MPL-2.0

//! Bounded input routing independent of display or script execution.
use crate::{Button, ButtonEvent, ButtonEventKind, ButtonLevels, Debouncer};

pub const INPUT_QUEUE_CAPACITY: usize = 32;
pub const SAMPLE_INTERVAL_MS: u64 = 20;
pub const LONG_PRESS_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputMode {
    Normal,
    Diagnostic,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputKind {
    Click,
    LongPress,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputEvent {
    pub button: Button,
    pub kind: InputKind,
    pub held_ms: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputPayload {
    Script(InputEvent),
    Diagnostic(ButtonEvent),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueuedInput {
    pub epoch: u64,
    pub payload: InputPayload,
}

#[derive(Clone, Copy)]
struct Key {
    debounce: Debouncer,
    pressed_at: Option<u64>,
    long_sent: bool,
    armed: bool,
    release_samples: u8,
}
impl Key {
    const fn new() -> Self {
        Self {
            debounce: Debouncer::new(3, 0),
            pressed_at: None,
            long_sent: false,
            armed: false,
            release_samples: 0,
        }
    }
    fn sample(&mut self, raw: bool, now_ms: u64) -> Option<(ButtonEventKind, u64)> {
        // Require a stable physical release on startup and every ownership change,
        // including presses not yet accepted by the debouncer at transition time.
        if !self.armed {
            self.release_samples = if raw {
                0
            } else {
                self.release_samples.saturating_add(1)
            };
            if self.release_samples >= 3 {
                self.armed = true;
            }
            return None;
        }
        match self.debounce.update(raw) {
            Some(ButtonEventKind::Pressed) => {
                self.pressed_at = Some(now_ms);
                self.long_sent = false;
                Some((ButtonEventKind::Pressed, 0))
            }
            Some(ButtonEventKind::Released) => {
                let held = now_ms.saturating_sub(self.pressed_at.take().unwrap_or(now_ms));
                Some((ButtonEventKind::Released, held))
            }
            _ => {
                if let Some(start) = self.pressed_at {
                    let held = now_ms.saturating_sub(start);
                    if !self.long_sent && held >= LONG_PRESS_MS {
                        self.long_sent = true;
                        return Some((ButtonEventKind::LongPressed, held));
                    }
                }
                None
            }
        }
    }
}

pub struct InputRouter {
    keys: [Key; 3],
    levels: ButtonLevels,
    epoch: u64,
    mode: InputMode,
    queue: [Option<QueuedInput>; INPUT_QUEUE_CAPACITY],
    head: usize,
    len: usize,
    overflow: u64,
    captured: u64,
}
impl Default for InputRouter {
    fn default() -> Self {
        Self::new()
    }
}
impl InputRouter {
    pub const fn new() -> Self {
        Self {
            keys: [Key::new(); 3],
            levels: ButtonLevels {
                up: false,
                down: false,
                confirm: false,
            },
            epoch: 0,
            mode: InputMode::Normal,
            queue: [None; INPUT_QUEUE_CAPACITY],
            head: 0,
            len: 0,
            overflow: 0,
            captured: 0,
        }
    }
    pub fn transition(&mut self, epoch: u64, mode: InputMode) {
        if self.epoch == epoch && self.mode == mode {
            return;
        }
        self.epoch = epoch;
        self.mode = mode;
        self.keys = [Key::new(); 3];
        self.queue.fill(None);
        self.head = 0;
        self.len = 0;
    }
    pub fn sample(&mut self, levels: ButtonLevels, now_ms: u64) {
        self.levels = levels;
        for (index, button) in [Button::Up, Button::Down, Button::Confirm]
            .into_iter()
            .enumerate()
        {
            let Some((kind, held_ms)) = self.keys[index].sample(levels.pressed(button), now_ms)
            else {
                continue;
            };
            let payload = match self.mode {
                InputMode::Diagnostic => InputPayload::Diagnostic(ButtonEvent { button, kind }),
                InputMode::Normal => {
                    let kind = match kind {
                        ButtonEventKind::LongPressed => InputKind::LongPress,
                        ButtonEventKind::Released if !self.keys[index].long_sent => {
                            InputKind::Click
                        }
                        _ => continue,
                    };
                    InputPayload::Script(InputEvent {
                        button,
                        kind,
                        held_ms,
                    })
                }
            };
            self.captured = self.captured.saturating_add(1);
            if self.len == INPUT_QUEUE_CAPACITY {
                self.overflow = self.overflow.saturating_add(1);
                continue;
            }
            self.queue[(self.head + self.len) % INPUT_QUEUE_CAPACITY] = Some(QueuedInput {
                epoch: self.epoch,
                payload,
            });
            self.len += 1;
        }
    }
    pub fn peek_mode(&self, mode: InputMode) -> Option<QueuedInput> {
        if self.mode != mode || self.len == 0 { return None; }
        self.queue[self.head]
    }
    pub fn pop_mode(&mut self, mode: InputMode) -> Option<QueuedInput> {
        if self.mode != mode { return None; }
        self.pop()
    }
    pub fn pop(&mut self) -> Option<QueuedInput> {
        if self.len == 0 {
            return None;
        }
        let event = self.queue[self.head].take();
        self.head = (self.head + 1) % INPUT_QUEUE_CAPACITY;
        self.len -= 1;
        event
    }
    pub const fn levels(&self) -> ButtonLevels {
        self.levels
    }
    pub const fn overflow(&self) -> u64 {
        self.overflow
    }
    pub const fn captured(&self) -> u64 {
        self.captured
    }
    pub const fn pending(&self) -> usize {
        self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn samples(router: &mut InputRouter, levels: ButtonLevels, count: usize, now: &mut u64) {
        for _ in 0..count {
            *now += SAMPLE_INTERVAL_MS;
            router.sample(levels, *now);
        }
    }
    fn ready() -> (InputRouter, u64) {
        let mut r = InputRouter::new();
        let mut t = 0;
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        (r, t)
    }
    fn levels(button: Button) -> ButtonLevels {
        ButtonLevels {
            up: button == Button::Up,
            down: button == Button::Down,
            confirm: button == Button::Confirm,
        }
    }
    #[test]
    fn three_keys_click_with_duration_after_debounce() {
        let (mut r, mut t) = ready();
        for button in [Button::Up, Button::Down, Button::Confirm] {
            samples(&mut r, levels(button), 2, &mut t);
            assert!(r.pop().is_none());
            samples(&mut r, levels(button), 3, &mut t);
            samples(&mut r, ButtonLevels::default(), 3, &mut t);
            assert_eq!(
                r.pop().unwrap().payload,
                InputPayload::Script(InputEvent {
                    button,
                    kind: InputKind::Click,
                    held_ms: 100
                })
            );
        }
    }
    #[test]
    fn long_press_once_and_release_never_clicks() {
        let (mut r, mut t) = ready();
        for button in [Button::Up, Button::Down, Button::Confirm] {
            samples(&mut r, levels(button), 53, &mut t);
            assert_eq!(
                r.pop().unwrap().payload,
                InputPayload::Script(InputEvent {
                    button,
                    kind: InputKind::LongPress,
                    held_ms: 1000
                })
            );
            samples(&mut r, levels(button), 70, &mut t);
            samples(&mut r, ButtonLevels::default(), 3, &mut t);
            assert!(r.pop().is_none());
        }
    }
    #[test]
    fn transition_drops_old_queue_and_held_button_requires_release() {
        let (mut r, mut t) = ready();
        samples(&mut r, levels(Button::Up), 3, &mut t);
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        samples(&mut r, levels(Button::Confirm), 1, &mut t);
        r.transition(2, InputMode::Normal);
        samples(&mut r, levels(Button::Confirm), 100, &mut t);
        assert!(r.pop().is_none());
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        samples(&mut r, levels(Button::Confirm), 3, &mut t);
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        assert_eq!(r.pop().unwrap().epoch, 2);
    }
    #[test]
    fn bounded_queue_preserves_fifo_and_drops_newest_without_consumer() {
        let (mut r, mut t) = ready();
        for i in 0..35 {
            samples(
                &mut r,
                levels(if i % 2 == 0 { Button::Up } else { Button::Down }),
                3,
                &mut t,
            );
            samples(&mut r, ButtonLevels::default(), 3, &mut t);
        }
        assert_eq!(r.pending(), 32);
        assert_eq!(r.overflow(), 3);
        assert_eq!(r.captured(), 35);
        for i in 0..32 {
            let InputPayload::Script(e) = r.pop().unwrap().payload else {
                panic!()
            };
            assert_eq!(e.button, if i % 2 == 0 { Button::Up } else { Button::Down });
        }
        assert!(r.pop().is_none());
        samples(&mut r, levels(Button::Confirm), 3, &mut t);
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        assert!(r.pop().is_some());
    }
    #[test]
    fn diagnostic_receives_raw_edges_and_hold_only_in_explicit_mode() {
        let (mut r, mut t) = ready();
        r.transition(1, InputMode::Diagnostic);
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        samples(&mut r, levels(Button::Confirm), 53, &mut t);
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        for kind in [
            ButtonEventKind::Pressed,
            ButtonEventKind::LongPressed,
            ButtonEventKind::Released,
        ] {
            assert_eq!(
                r.pop().unwrap().payload,
                InputPayload::Diagnostic(ButtonEvent {
                    button: Button::Confirm,
                    kind
                })
            );
        }
        assert!(r.pop().is_none());
    }
    #[test]
    fn bounce_and_idempotent_transition_do_not_drop_a_valid_click() {
        let (mut r, mut t) = ready();
        samples(&mut r, levels(Button::Up), 2, &mut t);
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        assert!(r.pop().is_none());
        samples(&mut r, levels(Button::Up), 3, &mut t);
        r.transition(0, InputMode::Normal);
        samples(&mut r, ButtonLevels::default(), 3, &mut t);
        assert!(r.pop().is_some());
    }
}
