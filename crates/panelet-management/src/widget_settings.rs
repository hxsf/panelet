// SPDX-License-Identifier: MPL-2.0

//! Per-installation configuration and a bounded monotonic polling schedule.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;

use crate::{compile_host_permissions, HOST_PERMISSION_CAPACITY};

pub const WIDGET_SETTINGS_MAX: usize = 2048;
pub const POLL_INTERVAL_MIN: u32 = 60;
pub const POLL_INTERVAL_MAX: u32 = 86_400;

#[derive(Clone, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WidgetSettings {
    /// Zero disables automatic invocation. Only the active widget is polled.
    #[serde(default)]
    pub interval_seconds: u32,
    /// None uses manifest permissions; Some replaces them by explicit admin grant.
    #[serde(default)]
    pub host_permissions: Option<Vec<String>>,
    #[serde(default = "empty_values")]
    pub values: Value,
}

fn empty_values() -> Value {
    json!({})
}

impl Default for WidgetSettings {
    fn default() -> Self {
        Self {
            interval_seconds: 0,
            host_permissions: None,
            values: empty_values(),
        }
    }
}

// Configuration may contain API credentials. Never include values or URLs in
// derived command/broker debug output. Explicit authenticated reads are separate.
impl fmt::Debug for WidgetSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetSettings")
            .field("interval_seconds", &self.interval_seconds)
            .field("configuration", &"[redacted]")
            .finish()
    }
}

impl WidgetSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.interval_seconds != 0
            && !(POLL_INTERVAL_MIN..=POLL_INTERVAL_MAX).contains(&self.interval_seconds)
        {
            return Err("interval must be zero or 60..86400 seconds");
        }
        if !self.values.is_object() {
            return Err("configuration values must be an object");
        }
        if self.encoded_len() > WIDGET_SETTINGS_MAX {
            return Err("configuration exceeds 2048 bytes");
        }
        if let Some(permissions) = &self.host_permissions {
            if permissions.len() > HOST_PERMISSION_CAPACITY
                || compile_host_permissions(permissions).is_err()
            {
                return Err("invalid configuration host permissions");
            }
        }
        Ok(())
    }

    pub fn encoded_len(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len())
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PollSchedule {
    next_due_ms: Option<u64>,
    failures: u8,
}

impl PollSchedule {
    pub fn due(&self, now_ms: u64, interval: u32) -> bool {
        interval != 0 && self.next_due_ms.is_none_or(|due| now_ms >= due)
    }

    pub fn finish(&mut self, now_ms: u64, interval: u32, success: bool) {
        self.failures = if success {
            0
        } else {
            self.failures.saturating_add(1).min(4)
        };
        let delay = u64::from(interval)
            .saturating_mul(1 << self.failures)
            .min(u64::from(POLL_INTERVAL_MAX));
        self.next_due_ms = Some(now_ms.saturating_add(delay * 1000));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_are_bounded_and_debug_redacts_credentials() {
        let mut settings = WidgetSettings::default();
        settings.values = json!({"bearer_token":"secret", "url":"https://private.test/feed"});
        settings.validate().unwrap();
        assert!(!format!("{settings:?}").contains("secret"));
        assert!(!format!("{settings:?}").contains("private.test"));
        settings.interval_seconds = 59;
        assert!(settings.validate().is_err());
        settings.interval_seconds = 60;
        settings.host_permissions = Some(vec!["https://*/feed".into()]);
        assert!(settings.validate().is_err());
        settings.host_permissions = Some(vec!["https://example.test/feed".into()]);
        settings.validate().unwrap();
        settings.values = json!({"x":"x".repeat(WIDGET_SETTINGS_MAX)});
        assert!(settings.validate().is_err());
    }

    #[test]
    fn polling_coalesces_missed_deadlines_and_backs_off_from_completion() {
        let mut schedule = PollSchedule::default();
        assert!(!schedule.due(0, 0));
        assert!(schedule.due(0, 60));
        schedule.finish(25_000, 60, true);
        assert!(!schedule.due(84_999, 60));
        assert!(schedule.due(85_000, 60));
        schedule.finish(90_000, 60, false);
        assert!(!schedule.due(209_999, 60));
        assert!(schedule.due(210_000, 60));
        schedule.finish(1_000_000, 60, true);
        assert!(!schedule.due(1_059_999, 60));
        assert!(schedule.due(1_060_000, 60));
    }
}
