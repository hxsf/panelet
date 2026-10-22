// SPDX-License-Identifier: MPL-2.0

use embedded_hal::{
    delay::DelayNs,
    digital::{InputPin, OutputPin},
    spi::SpiBus,
};

/// Minimal synchronous interface required by the controller state machine.
///
/// `busy_is_low` is named after the electrical level on purpose: the SSD2683
/// proposal and the NOTE4C reference firmware both treat BUSY_N low as busy.
/// Keeping this polarity in the trait makes the board assumption visible and
/// auditable until it is confirmed on physical NOTE4C hardware.
pub trait DisplayInterface {
    type Error;

    fn set_bus_idle(&mut self) -> Result<(), Self::Error>;
    fn set_reset(&mut self, high: bool) -> Result<(), Self::Error>;
    fn set_power(&mut self, enabled: bool) -> Result<(), Self::Error>;
    fn write_command(&mut self, command: u8) -> Result<(), Self::Error>;
    fn write_data(&mut self, data: &[u8]) -> Result<(), Self::Error>;
    fn busy_is_low(&mut self) -> Result<bool, Self::Error>;
    fn delay_ms(&mut self, milliseconds: u32);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedHalError<SpiError, DcError, CsError, ResetError, BusyError, PowerError> {
    Spi(SpiError),
    Dc(DcError),
    Cs(CsError),
    Reset(ResetError),
    Busy(BusyError),
    Power(PowerError),
}

/// `embedded-hal` 1.0 adapter using an explicit, software-controlled CS pin.
///
/// SPI mode and clock rates are configured by the board crate before this
/// adapter is constructed. [`crate::SpiRates`] records and validates those
/// external settings in the controller core.
pub struct EmbeddedHalInterface<SPI, DC, CS, RESET, BUSY, POWER, DELAY> {
    spi: SPI,
    dc: DC,
    cs: CS,
    reset: RESET,
    busy: BUSY,
    power: POWER,
    delay: DELAY,
}

impl<SPI, DC, CS, RESET, BUSY, POWER, DELAY>
    EmbeddedHalInterface<SPI, DC, CS, RESET, BUSY, POWER, DELAY>
{
    #[must_use]
    pub const fn new(
        spi: SPI,
        dc: DC,
        cs: CS,
        reset: RESET,
        busy: BUSY,
        power: POWER,
        delay: DELAY,
    ) -> Self {
        Self {
            spi,
            dc,
            cs,
            reset,
            busy,
            power,
            delay,
        }
    }

    #[must_use]
    pub fn release(self) -> (SPI, DC, CS, RESET, BUSY, POWER, DELAY) {
        (
            self.spi, self.dc, self.cs, self.reset, self.busy, self.power, self.delay,
        )
    }
}

impl<SPI, DC, CS, RESET, BUSY, POWER, DELAY> DisplayInterface
    for EmbeddedHalInterface<SPI, DC, CS, RESET, BUSY, POWER, DELAY>
where
    SPI: SpiBus<u8>,
    DC: OutputPin,
    CS: OutputPin,
    RESET: OutputPin,
    BUSY: InputPin,
    POWER: OutputPin,
    DELAY: DelayNs,
{
    type Error =
        EmbeddedHalError<SPI::Error, DC::Error, CS::Error, RESET::Error, BUSY::Error, POWER::Error>;

    fn set_bus_idle(&mut self) -> Result<(), Self::Error> {
        self.cs.set_high().map_err(EmbeddedHalError::Cs)?;
        self.dc.set_low().map_err(EmbeddedHalError::Dc)
    }

    fn set_reset(&mut self, high: bool) -> Result<(), Self::Error> {
        if high {
            self.reset.set_high().map_err(EmbeddedHalError::Reset)
        } else {
            self.reset.set_low().map_err(EmbeddedHalError::Reset)
        }
    }

    fn set_power(&mut self, enabled: bool) -> Result<(), Self::Error> {
        if enabled {
            self.power.set_high().map_err(EmbeddedHalError::Power)
        } else {
            self.power.set_low().map_err(EmbeddedHalError::Power)
        }
    }

    fn write_command(&mut self, command: u8) -> Result<(), Self::Error> {
        self.dc.set_low().map_err(EmbeddedHalError::Dc)?;
        self.cs.set_low().map_err(EmbeddedHalError::Cs)?;
        let write_result = self.spi.write(&[command]).map_err(EmbeddedHalError::Spi);
        let cs_result = self.cs.set_high().map_err(EmbeddedHalError::Cs);
        write_result.and(cs_result)
    }

    fn write_data(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        if data.is_empty() {
            return Ok(());
        }
        self.dc.set_high().map_err(EmbeddedHalError::Dc)?;
        self.cs.set_low().map_err(EmbeddedHalError::Cs)?;
        let write_result = self.spi.write(data).map_err(EmbeddedHalError::Spi);
        let cs_result = self.cs.set_high().map_err(EmbeddedHalError::Cs);
        write_result.and(cs_result)
    }

    fn busy_is_low(&mut self) -> Result<bool, Self::Error> {
        self.busy.is_low().map_err(EmbeddedHalError::Busy)
    }

    fn delay_ms(&mut self, milliseconds: u32) {
        self.delay.delay_ms(milliseconds);
    }
}
