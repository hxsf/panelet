// SPDX-License-Identifier: MPL-2.0

/// SSD2683 commands used by the NOTE4C display sequences.
///
/// The undocumented vendor command `0xE9` is intentionally absent. It is only
/// available through `Ssd2683::apply_reference_vendor_compatibility` so callers
/// cannot accidentally present it as a documented controller command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Command {
    /// Turn the panel high-voltage supplies off (`POF`).
    PowerOff = 0x02,
    /// Turn the panel high-voltage supplies on (`PON`).
    PowerOn = 0x04,
    /// Enter deep sleep. The documented check byte is `0xA5` (`DSLP`).
    DeepSleep = 0x07,
    /// Start full-frame data transmission (`DTM`).
    DataTransmission = 0x10,
    /// Start a display refresh. The documented data byte is `0x00` (`DRF`).
    DisplayRefresh = 0x12,
    /// Set the partial window and PMODE (`PTLW`). Experimental only because the
    /// NOTE4C four-color panel has no validated partial-refresh waveform.
    #[cfg(feature = "experimental-partial-refresh")]
    PartialWindow = 0x83,
}

impl Command {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}
