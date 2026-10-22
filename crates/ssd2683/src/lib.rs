// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

//! Synchronous, allocation-free SSD2683 driver core for the NOTE4C panel.
//!
//! The default API exposes only the full-screen sequence supported by the
//! archived NOTE4C reference firmware. The `experimental-partial-refresh`
//! feature adds a narrowly bounded PTLW protocol probe which still transmits a
//! complete 30,000-byte DTM frame. It does not expose short-window DTM,
//! MTP/LUT programming, raw commands, or command scanning.
//!
//! The [`PanelColor`] encoding is panel-specific evidence from that reference
//! firmware. The SSD2683 proposal calls these values Gray0 through Gray3 and
//! does not itself assign them to physical pigments.

#[cfg(test)]
extern crate std;

mod command;
mod driver;
mod framebuffer;
mod interface;
#[cfg(feature = "experimental-partial-refresh")]
mod partial;

pub use command::Command;
pub use driver::{
    BusyWait, BusyWaitError, CleanupReport, DriverError, DriverState, DriverStatus,
    OperationConfig, RefreshFailure, SpiRateError, SpiRates, Ssd2683,
};
pub use framebuffer::{
    Framebuffer, FramebufferError, PanelColor, FRAMEBUFFER_LEN, HEIGHT, PIXELS_PER_BYTE, ROW_BYTES,
    WIDTH,
};
pub use interface::{DisplayInterface, EmbeddedHalError, EmbeddedHalInterface};
#[cfg(feature = "experimental-partial-refresh")]
pub use partial::{PartialWindow, PartialWindowError};
