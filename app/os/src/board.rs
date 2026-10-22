// SPDX-License-Identifier: MPL-2.0

use core::time::Duration as CoreDuration;

use esp_idf_svc::hal::adc::attenuation::DB_12;
use esp_idf_svc::hal::adc::oneshot::config::{AdcChannelConfig, Calibration, Resolution};
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::adc::ADC1;
use esp_idf_svc::hal::adc::{ADCCH3, ADCU1};
use esp_idf_svc::hal::delay::{FreeRtos, TickType};
use esp_idf_svc::hal::gpio::Pins;
use esp_idf_svc::hal::gpio::{AnyInputPin, Input, Output, PinDriver, Pull};
use esp_idf_svc::hal::i2c::I2C0;
use esp_idf_svc::hal::i2c::{APBTickType, I2cConfig, I2cDriver};
use esp_idf_svc::hal::spi::SPI3;
use esp_idf_svc::hal::spi::{
    config::{BitOrder, Config, DriverConfig, MODE_0},
    Dma, SpiBusDriver, SpiDriver,
};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::sys::{self, EspError};
use panelet_core::ButtonLevels;
use ssd2683::{
    EmbeddedHalInterface, OperationConfig, PartialWindow, SpiRates, Ssd2683, FRAMEBUFFER_LEN,
};
use std::time::Instant;

const EPD_SPI_WRITE_HZ: u32 = 10_000_000;
const EPD_SPI_MAX_TRANSFER: usize = 4_096;
const I2C_HZ: u32 = 400_000;
const I2C_HARDWARE_TIMEOUT_MS: u64 = 100;
const I2C_TRANSACTION_TIMEOUT_MS: u64 = 100;
const I2C_RAIL_SETTLE_MS: u32 = 3;
const I2C_SCAN_MAX_ELAPSED_MS: u128 = 12_000;
const RTC_SAMPLE_INTERVAL_MS: u32 = 1_100;
const BATTERY_ADC_SAMPLE_COUNT: usize = 10;

type BatteryAdc = AdcChannelDriver<'static, ADCCH3<ADCU1>, AdcDriver<'static, ADCU1>>;

/// The only module in the application that contains raw NOTE4C GPIO numbers.
pub mod pins {
    pub const BOOT_CONFIRM: i32 = 0;
    pub const POWER_STANDBY_STATUS: i32 = 1;
    pub const CHARGE_STATUS: i32 = 2;
    pub const STATUS_LED: i32 = 3;
    pub const BATTERY_ADC: i32 = 4;
    pub const EPD_POWER: i32 = 6;
    pub const EPD_BUSY: i32 = 8;
    pub const EPD_RESET: i32 = 9;
    pub const EPD_DC: i32 = 10;
    pub const EPD_CS: i32 = 11;
    pub const EPD_SCLK: i32 = 12;
    pub const EPD_MOSI: i32 = 13;
    pub const POWER_HOLD: i32 = 17;
    pub const POWER_BUTTON_DOWN: i32 = 18;
    pub const NFC_POWER: i32 = 21;
    pub const UP_BUTTON: i32 = 39;
    pub const AUDIO_RAIL: i32 = 42;
    pub const AUDIO_PA: i32 = 46;
    pub const I2C_SDA: i32 = 47;
    pub const I2C_SCL: i32 = 48;

    // Native USB Serial/JTAG. These are deliberately never acquired as GPIO.
    pub const USB_D_MINUS_RESERVED: i32 = 19;
    pub const USB_D_PLUS_RESERVED: i32 = 20;

    pub const BUTTONS: [i32; 3] = [UP_BUTTON, POWER_BUTTON_DOWN, BOOT_CONFIRM];
}

/// Assert the battery power latch before logging or other peripheral setup.
///
/// This deliberately uses the ESP-IDF GPIO API before `Peripherals::take()`.
/// GPIO ownership is transferred to `Board` immediately afterwards. Unlike
/// optional rails, this pin must actively drive high during construction; it
/// is therefore exposed high and immediately re-held before any HAL setup.
pub fn assert_power_hold_early() -> Result<(), EspError> {
    unsafe {
        let pin = pins::POWER_HOLD as _;
        EspError::convert(sys::gpio_set_level(pin, 1))?;
        EspError::convert(sys::gpio_set_direction(
            pin,
            sys::gpio_mode_t_GPIO_MODE_OUTPUT,
        ))?;
        EspError::convert(sys::gpio_hold_dis(pin))?;
        EspError::convert(sys::gpio_hold_en(pin))?;
    }
    Ok(())
}

/// Take ownership of a possibly held output without exposing the reset state.
///
/// The archived firmware enables pad hold on board power outputs. ESP-IDF 5.5
/// documents that configuration and output level must be established before
/// `gpio_hold_dis`, otherwise releasing hold can expose the default level. This
/// helper deliberately leaves the pad held. `Board::take` releases every pad
/// only after all fallible HAL/SPI construction has completed, so a partially
/// initialized `PinDriver` cannot expose its Drop-time reset state.
fn prepare_held_output(pin: i32, high: bool) -> Result<(), EspError> {
    unsafe {
        let pin = pin as _;
        // Freeze the inherited state first, including on a fresh boot where the
        // pad may not already be held. Configure the desired state behind that
        // hold, then release it so the safe state takes effect and immediately
        // re-hold it for the fallible HAL construction that follows.
        EspError::convert(sys::gpio_hold_en(pin))?;
        EspError::convert(sys::gpio_reset_pin(pin))?;
        EspError::convert(sys::gpio_set_level(pin, u32::from(high)))?;
        EspError::convert(sys::gpio_set_direction(
            pin,
            sys::gpio_mode_t_GPIO_MODE_OUTPUT,
        ))?;
        EspError::convert(sys::gpio_hold_dis(pin))?;
        EspError::convert(sys::gpio_hold_en(pin))?;
    }
    Ok(())
}

fn prepare_held_input(pin: i32) -> Result<(), EspError> {
    unsafe {
        let pin = pin as _;
        EspError::convert(sys::gpio_hold_en(pin))?;
        EspError::convert(sys::gpio_reset_pin(pin))?;
        EspError::convert(sys::gpio_set_direction(
            pin,
            sys::gpio_mode_t_GPIO_MODE_INPUT,
        ))?;
        EspError::convert(sys::gpio_hold_dis(pin))?;
        EspError::convert(sys::gpio_hold_en(pin))?;
    }
    Ok(())
}

fn retain_first_error(result: Result<(), EspError>, first: &mut Option<EspError>) {
    if let Err(error) = result {
        if first.is_none() {
            *first = Some(error);
        }
    }
}

fn prepare_all_safe_pads() -> Result<(), EspError> {
    let mut first_error = None;
    for (pin, high) in [
        (pins::POWER_HOLD, true),
        (pins::STATUS_LED, true),
        (pins::EPD_POWER, false),
        (pins::EPD_RESET, false),
        (pins::EPD_DC, false),
        (pins::EPD_CS, true),
        (pins::EPD_SCLK, false),
        (pins::EPD_MOSI, false),
        (pins::NFC_POWER, false),
        (pins::AUDIO_RAIL, false),
        (pins::AUDIO_PA, false),
    ] {
        retain_first_error(prepare_held_output(pin, high), &mut first_error);
    }
    retain_first_error(prepare_held_input(pins::EPD_BUSY), &mut first_error);

    if let Some(error) = first_error {
        Err(error)
    } else {
        Ok(())
    }
}

fn release_all_prepared_holds() -> HoldReleaseHealth {
    let mut failed_gpio_mask = 0_u64;

    // RESET is exposed low before the EPD rail is exposed low. All other
    // outputs are likewise already at their safe latch level behind the hold.
    for pin in [
        pins::EPD_RESET,
        pins::EPD_CS,
        pins::EPD_DC,
        pins::EPD_SCLK,
        pins::EPD_MOSI,
        pins::EPD_POWER,
        pins::NFC_POWER,
        pins::AUDIO_RAIL,
        pins::AUDIO_PA,
        pins::STATUS_LED,
        pins::POWER_HOLD,
        pins::EPD_BUSY,
    ] {
        let released = unsafe { EspError::convert(sys::gpio_hold_dis(pin as _)) }.is_ok();
        if !released {
            failed_gpio_mask |= 1_u64 << pin;
        }
    }

    HoldReleaseHealth { failed_gpio_mask }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HoldReleaseHealth {
    pub failed_gpio_mask: u64,
}

impl HoldReleaseHealth {
    pub const fn is_healthy(self) -> bool {
        self.failed_gpio_mask == 0
    }
}

/// Moved once into the input sampler after startup safety checks.
pub struct ButtonPins {
    up: PinDriver<'static, Input>,
    down: PinDriver<'static, Input>,
    confirm: PinDriver<'static, Input>,
}

impl ButtonPins {
    pub fn levels(&self) -> ButtonLevels {
        ButtonLevels {
            up: self.up.is_low(),
            down: self.down.is_low(),
            confirm: self.confirm.is_low(),
        }
    }
}

pub struct Board {
    display_cache: panelet_management::DisplayCache,
    power_hold: PinDriver<'static, Output>,
    epd_power: PinDriver<'static, Output>,
    epd_reset: PinDriver<'static, Output>,
    epd_dc: PinDriver<'static, Output>,
    epd_cs: PinDriver<'static, Output>,
    epd_busy: PinDriver<'static, Input>,
    epd_spi: SpiBusDriver<'static, SpiDriver<'static>>,
    nfc_power: PinDriver<'static, Output>,
    audio_rail: PinDriver<'static, Output>,
    audio_pa: PinDriver<'static, Output>,
    led: PinDriver<'static, Output>,
    button_pins: Option<ButtonPins>,
    power_standby_status: Option<PinDriver<'static, Input>>,
    charge_status: Option<PinDriver<'static, Input>>,
    i2c: Option<I2cDriver<'static>>,
    battery_adc: Option<BatteryAdc>,
    i2c_init_error: Option<i32>,
    battery_adc_init_error: Option<i32>,
    power_standby_init_error: Option<i32>,
    charge_status_init_error: Option<i32>,
    hold_release_health: HoldReleaseHealth,
    epd_inhibited: bool,
    i2c_inhibited: bool,
}

impl Board {
    pub fn take(
        pins: Pins,
        spi3: SPI3<'static>,
        i2c0: I2C0<'static>,
        adc1: ADC1<'static>,
    ) -> Result<Self, EspError> {
        // Every safety-relevant pad remains held until all fallible constructors
        // below have succeeded. Preparation attempts every pad before returning
        // the first error, leaving successfully prepared pads held safe.
        prepare_all_safe_pads()?;

        let mut power_hold = PinDriver::output(pins.gpio17)?;
        power_hold.set_high()?;

        // Configure high-energy and optional rails off before anything can use them.
        let mut epd_power = PinDriver::output(pins.gpio6)?;
        epd_power.set_low()?;
        let mut nfc_power = PinDriver::output(pins.gpio21)?;
        nfc_power.set_low()?;
        let mut audio_rail = PinDriver::output(pins.gpio42)?;
        audio_rail.set_low()?;
        let mut audio_pa = PinDriver::output(pins.gpio46)?;
        audio_pa.set_low()?;

        // Keep the display deselected and held in reset while its rail is off.
        let mut epd_cs = PinDriver::output(pins.gpio11)?;
        epd_cs.set_high()?;
        let mut epd_reset = PinDriver::output(pins.gpio9)?;
        epd_reset.set_low()?;
        let mut epd_dc = PinDriver::output(pins.gpio10)?;
        epd_dc.set_low()?;

        // BUSY_N is active-low according to the proposal text and archived
        // firmware. Keep the board pull neutral until hardware confirms it.
        let epd_busy = PinDriver::input(pins.gpio8, Pull::Floating)?;

        // The SSD2683 proposal's serial timing is interpreted as SPI mode 0.
        // This is a conservative 10 MHz write-only 4-wire bus (separate DC/CS),
        // below the proposal's 20 MHz write ceiling. No MISO pin is connected.
        let spi_driver = SpiDriver::new(
            spi3,
            pins.gpio12,
            pins.gpio13,
            None::<AnyInputPin<'static>>,
            &DriverConfig::new().dma(Dma::Auto(EPD_SPI_MAX_TRANSFER)),
        )?;
        let epd_spi = SpiBusDriver::new(
            spi_driver,
            &Config::new()
                .baudrate(Hertz(EPD_SPI_WRITE_HZ))
                .data_mode(MODE_0)
                .write_only(true)
                .bit_order(BitOrder::MsbFirst),
        )?;

        // The status LED sinks current and is therefore active-low.
        let mut led = PinDriver::output(pins.gpio3)?;
        led.set_high()?;

        // GPIO0 remains input-only so BOOT + RESET can always enter ROM download.
        let up = PinDriver::input(pins.gpio39, Pull::Up)?;
        let down = PinDriver::input(pins.gpio18, Pull::Up)?;
        let confirm = PinDriver::input(pins.gpio0, Pull::Up)?;

        // The charger/power status outputs are board signals whose external
        // electrical behavior must not be changed by internal pulls.
        let (power_standby_status, power_standby_init_error) =
            match PinDriver::input(pins.gpio1, Pull::Floating) {
                Ok(driver) => (Some(driver), None),
                Err(error) => (None, Some(error.code())),
            };
        let (charge_status, charge_status_init_error) =
            match PinDriver::input(pins.gpio2, Pull::Floating) {
                Ok(driver) => (Some(driver), None),
                Err(error) => (None, Some(error.code())),
            };

        // Construction performs no bus transaction and is deliberately done
        // while GPIO42 remains held low. External I2C pull-ups therefore remain
        // unpowered; internal pull-ups are explicitly disabled.
        let i2c_config = I2cConfig::new()
            .baudrate(Hertz(I2C_HZ))
            .sda_enable_pullup(false)
            .scl_enable_pullup(false)
            .timeout(APBTickType::from(CoreDuration::from_millis(
                I2C_HARDWARE_TIMEOUT_MS,
            )));
        let (i2c, i2c_init_error) =
            match I2cDriver::new(i2c0, pins.gpio47, pins.gpio48, &i2c_config) {
                Ok(driver) => (Some(driver), None),
                Err(error) => (None, Some(error.code())),
            };

        let adc_config = AdcChannelConfig {
            attenuation: DB_12,
            resolution: Resolution::Resolution12Bit,
            calibration: Calibration::Curve,
        };
        let (battery_adc, battery_adc_init_error) = match AdcDriver::new(adc1) {
            Ok(adc) => match AdcChannelDriver::new(adc, pins.gpio4, &adc_config) {
                Ok(channel) => (Some(channel), None),
                Err(error) => (None, Some(error.code())),
            },
            Err(error) => (None, Some(error.code())),
        };

        let hold_release_health = release_all_prepared_holds();
        Ok(Self {
            display_cache: panelet_management::DisplayCache::default(),
            power_hold,
            epd_power,
            epd_reset,
            epd_dc,
            epd_cs,
            epd_busy,
            epd_spi,
            nfc_power,
            audio_rail,
            audio_pa,
            led,
            button_pins: Some(ButtonPins { up, down, confirm }),
            power_standby_status,
            charge_status,
            i2c,
            battery_adc,
            i2c_init_error,
            battery_adc_init_error,
            power_standby_init_error,
            charge_status_init_error,
            hold_release_health,
            epd_inhibited: !hold_release_health.is_healthy(),
            i2c_inhibited: false,
        })
    }

    /// Startup reads directly; after handoff only the sampler touches GPIO.
    pub fn buttons(&self) -> ButtonLevels {
        self.button_pins
            .as_ref()
            .map(ButtonPins::levels)
            .unwrap_or_else(crate::input::levels)
    }

    pub fn take_input_pins(&mut self) -> Option<ButtonPins> {
        self.button_pins.take()
    }

    pub fn set_led(&mut self, on: bool) -> Result<(), EspError> {
        if on {
            self.led.set_low()
        } else {
            self.led.set_high()
        }
    }

    pub const fn hold_release_health(&self) -> HoldReleaseHealth {
        self.hold_release_health
    }

    pub const fn epd_available(&self) -> bool {
        self.hold_release_health.is_healthy() && !self.epd_inhibited
    }

    pub fn peripheral_availability(&self) -> PeripheralAvailability {
        PeripheralAvailability {
            i2c: self.i2c.is_some() && !self.i2c_inhibited,
            i2c_init_error: self.i2c_init_error,
            battery_adc: self.battery_adc.is_some(),
            battery_adc_init_error: self.battery_adc_init_error,
            power_standby_status: self.power_standby_status.is_some(),
            power_standby_init_error: self.power_standby_init_error,
            charge_status: self.charge_status.is_some(),
            charge_status_init_error: self.charge_status_init_error,
        }
    }

    pub fn i2c_scan(&mut self) -> Result<I2cScanReport, I2cAccessError> {
        self.with_i2c_power(|i2c| {
            let started = Instant::now();
            let mut report = I2cScanReport::new();
            for address in 0x08_u8..=0x77 {
                if started.elapsed().as_millis() >= I2C_SCAN_MAX_ELAPSED_MS {
                    report.round_deadline_hit = true;
                    break;
                }
                report.attempted += 1;
                match i2c.write(
                    address,
                    &[],
                    TickType::new_millis(I2C_TRANSACTION_TIMEOUT_MS).ticks(),
                ) {
                    Ok(()) => report.found[address as usize] = true,
                    Err(error) if error.code() == sys::ESP_FAIL => report.nack_or_fail += 1,
                    Err(error) if error.code() == sys::ESP_ERR_TIMEOUT => {
                        report.timeout += 1;
                        report.first_timeout_address = Some(address);
                        // A timeout may mean the shared bus is stuck. Do not
                        // issue more transactions in an unknown bus state.
                        break;
                    }
                    Err(error) => {
                        report.other_error += 1;
                        report.first_other_error = Some((address, error.code()));
                        // Only ESP_FAIL/NACK is expected during a scan. Stop
                        // on every other error instead of repeatedly probing.
                        break;
                    }
                }
            }
            report.transaction_elapsed_ms = started.elapsed().as_millis();
            Ok(report)
        })
    }

    pub fn rtc_read_pair(&mut self) -> Result<RtcReadPair, I2cAccessError> {
        self.with_i2c_power(|i2c| {
            let timeout = TickType::new_millis(I2C_TRANSACTION_TIMEOUT_MS).ticks();
            let mut first = [0_u8; 7];
            let mut second = [0_u8; 7];
            i2c.write_read(0x51, &[0x02], &mut first, timeout)?;
            let interval_started = Instant::now();
            FreeRtos::delay_ms(RTC_SAMPLE_INTERVAL_MS);
            i2c.write_read(0x51, &[0x02], &mut second, timeout)?;
            Ok(RtcReadPair {
                first,
                second,
                interval_elapsed_ms: interval_started.elapsed().as_millis(),
            })
        })
    }

    /// Read one raw PCF8563 sample for the widget runtime. The existing
    /// diagnostic pair deliberately waits 1.1 s to verify clock progress;
    /// widget reads only need one bounded transaction and are therefore kept
    /// as a separate operation.
    pub fn rtc_read_once(&mut self) -> Result<[u8; 7], I2cAccessError> {
        self.with_i2c_power(|i2c| {
            let timeout = TickType::new_millis(I2C_TRANSACTION_TIMEOUT_MS).ticks();
            let mut sample = [0_u8; 7];
            i2c.write_read(0x51, &[0x02], &mut sample, timeout)?;
            Ok(sample)
        })
    }

    /// Write all seven PCF8563 clock/calendar registers in one I2C
    /// transaction, then read them back before releasing the shared bus.
    pub fn rtc_write_utc(&mut self, registers: [u8; 7]) -> Result<[u8; 7], I2cAccessError> {
        self.with_i2c_power(|i2c| {
            let timeout = TickType::new_millis(I2C_TRANSACTION_TIMEOUT_MS).ticks();
            let payload = [
                0x02,
                registers[0],
                registers[1],
                registers[2],
                registers[3],
                registers[4],
                registers[5],
                registers[6],
            ];
            i2c.write(0x51, &payload, timeout)?;
            let mut verified = [0_u8; 7];
            i2c.write_read(0x51, &[0x02], &mut verified, timeout)?;
            Ok(verified)
        })
    }

    pub fn battery_status_levels(&self) -> Result<BatteryStatusLevels, PeripheralUnavailable> {
        let standby = self
            .power_standby_status
            .as_ref()
            .ok_or(PeripheralUnavailable {
                component: "GPIO1_STDBY_H",
                init_error: self.power_standby_init_error,
            })?;
        let charge = self.charge_status.as_ref().ok_or(PeripheralUnavailable {
            component: "GPIO2_CHRG_L",
            init_error: self.charge_status_init_error,
        })?;
        Ok(BatteryStatusLevels {
            standby_high: standby.is_high(),
            charge_high: charge.is_high(),
        })
    }

    pub fn battery_adc_samples(&mut self) -> Result<BatteryAdcSamples, PeripheralReadError> {
        let adc = self.battery_adc.as_mut().ok_or(PeripheralReadError {
            component: "GPIO4_ADC1_CH3",
            init_error: self.battery_adc_init_error,
            read_error: None,
        })?;
        let mut report = BatteryAdcSamples {
            raw: [0; BATTERY_ADC_SAMPLE_COUNT],
            calibrated_mv: [0; BATTERY_ADC_SAMPLE_COUNT],
        };
        for index in 0..BATTERY_ADC_SAMPLE_COUNT {
            let raw = adc.read_raw().map_err(|error| PeripheralReadError {
                component: "GPIO4_ADC1_CH3",
                init_error: None,
                read_error: Some(error.code()),
            })?;
            let calibrated_mv = adc.raw_to_mv(raw).map_err(|error| PeripheralReadError {
                component: "GPIO4_ADC1_CH3_CURVE",
                init_error: None,
                read_error: Some(error.code()),
            })?;
            report.raw[index] = raw;
            report.calibrated_mv[index] = calibrated_mv;
            FreeRtos::delay_ms(10);
        }
        Ok(report)
    }

    fn with_i2c_power<T>(
        &mut self,
        operation: impl FnOnce(&mut I2cDriver<'static>) -> Result<T, EspError>,
    ) -> Result<T, I2cAccessError> {
        if self.i2c_inhibited {
            return Err(I2cAccessError {
                phase: I2cFailurePhase::Inhibited,
                code: None,
                rail_off_code: None,
            });
        }
        if self.i2c.is_none() {
            return Err(I2cAccessError {
                phase: I2cFailurePhase::Unavailable,
                code: self.i2c_init_error,
                rail_off_code: None,
            });
        }

        // Keep the optional NFC rail disabled before starting the required
        // PA-low -> shared-rail-high I2C power sequence.
        if let Err(error) = self.nfc_power.set_low() {
            let rail_off_code = self.audio_rail.set_low().err().map(|cause| cause.code());
            self.i2c_inhibited = true;
            return Err(I2cAccessError {
                phase: I2cFailurePhase::NfcOff,
                code: Some(error.code()),
                rail_off_code,
            });
        }
        if let Err(error) = self.audio_pa.set_low() {
            let rail_off_code = self.audio_rail.set_low().err().map(|cause| cause.code());
            self.i2c_inhibited = true;
            return Err(I2cAccessError {
                phase: I2cFailurePhase::PaLow,
                code: Some(error.code()),
                rail_off_code,
            });
        }
        if let Err(error) = self.audio_rail.set_high() {
            let rail_off_code = self.audio_rail.set_low().err().map(|cause| cause.code());
            self.i2c_inhibited = true;
            return Err(I2cAccessError {
                phase: I2cFailurePhase::RailOn,
                code: Some(error.code()),
                rail_off_code,
            });
        }
        FreeRtos::delay_ms(I2C_RAIL_SETTLE_MS);

        let operation_result = operation(self.i2c.as_mut().expect("availability checked above"));
        let rail_off_error = self.audio_rail.set_low().err().map(|error| error.code());
        if let Some(rail_off_code) = rail_off_error {
            self.i2c_inhibited = true;
            return Err(I2cAccessError {
                phase: if operation_result.is_err() {
                    I2cFailurePhase::Transaction
                } else {
                    I2cFailurePhase::RailOff
                },
                code: operation_result.err().map(|error| error.code()),
                rail_off_code: Some(rail_off_code),
            });
        }
        operation_result.map_err(|error| I2cAccessError {
            phase: I2cFailurePhase::Transaction,
            code: Some(error.code()),
            rail_off_code: None,
        })
    }

    /// Perform the one allowed SSD2683 operation: a bounded full refresh.
    ///
    /// The archived firmware's undocumented `E9 01` compatibility command is
    /// explicitly opted into here. The driver performs graceful shutdown on
    /// success and best-effort cleanup (including rail-off) on every failure.

    pub fn run_epd_full_refresh(&mut self, frame: &[u8; FRAMEBUFFER_LEN]) -> EpdRunReport {
        self.display_cache.invalidate();
        if !self.epd_available() {
            let forced_safe = self.enforce_epd_safe_outputs();
            return EpdRunReport {
                attempted: false,
                success: false,
                driver_cleanup_clean: false,
                pon_drf_elapsed_ms: None,
                forced_safe,
            };
        }

        let rates = SpiRates::new(EPD_SPI_WRITE_HZ, None)
            .expect("the fixed EPD SPI write rate is controller-safe");
        let config = OperationConfig::default();
        let interface = EmbeddedHalInterface::new(
            &mut self.epd_spi,
            &mut self.epd_dc,
            &mut self.epd_cs,
            &mut self.epd_reset,
            &mut self.epd_busy,
            &mut self.epd_power,
            FreeRtos,
        );
        let mut display = Ssd2683::new(interface, rates);

        let mut stage = "initialize";
        let mut pon_drf_elapsed_ms = None;
        let sequence_started = Instant::now();
        let result = (|| {
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"initialize\"}}");
            display.initialize(config)?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"initialize\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "vendor_e9_01";
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"vendor_e9_01\"}}");
            display.apply_reference_vendor_compatibility()?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"vendor_e9_01\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "frame_load";
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"frame_load\"}}");
            display.load_full_frame(frame, config.busy_wait)?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"frame_load\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "pon_drf_refresh";
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"pon_drf_refresh\"}}");
            let refresh_result = display.refresh_loaded_frame(config);
            pon_drf_elapsed_ms = Some(started.elapsed().as_millis());
            refresh_result?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"pon_drf_refresh\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "pof_deep_sleep";
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"pof_deep_sleep\"}}");
            display.shutdown(config)?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"pof_deep_sleep\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );
            Ok(())
        })();

        let (success, driver_cleanup_clean) = match result {
            Ok(()) => {
                log::info!(
                    "{{\"event\":\"epd_driver_result\",\"status\":\"ok\",\"state\":\"{:?}\",\"vendor_e9_01\":true,\"elapsed_ms\":{}}}",
                    display.status().state,
                    sequence_started.elapsed().as_millis()
                );
                (true, true)
            }
            Err(cause) => {
                let cleanup = display.cleanup_after_error(&cause, config);
                let cleanup_clean = cleanup.is_clean();
                log::error!(
                    "{{\"event\":\"epd_driver_result\",\"status\":\"error\",\"stage\":\"{}\",\"cause\":\"{:?}\",\"cleanup\":\"{:?}\",\"cleanup_clean\":{},\"elapsed_ms\":{}}}",
                    stage,
                    cause,
                    cleanup,
                    cleanup_clean,
                    sequence_started.elapsed().as_millis()
                );
                (false, cleanup_clean)
            }
        };
        // Explicitly release the interface so its mutable GPIO/SPI borrows end
        // before the board-level independent forced-safe pass.
        let _ = display.release();

        // Independently re-assert electrical safe state after the driver's
        // cleanup. This second attempt is useful if a transient GPIO call failed.
        let forced_safe = self.enforce_epd_safe_outputs();
        if success && driver_cleanup_clean && forced_safe.is_clean() {
            self.display_cache.record_success(frame);
        }
        EpdRunReport {
            attempted: true,
            success,
            driver_cleanup_clean,
            pon_drf_elapsed_ms,
            forced_safe,
        }
    }

    /// PTLW window refresh using the sequence observed on this NOTE4C board.
    /// The complete 30,000-byte frame is still sent before PTLW is enabled.
    /// Automatic callers must retain a successful full-frame baseline.
    pub fn run_epd_window_refresh(
        &mut self,
        frame: &[u8; FRAMEBUFFER_LEN],
        window: PartialWindow,
    ) -> EpdRunReport {
        self.display_cache.invalidate();
        if !self.epd_available() {
            let forced_safe = self.enforce_epd_safe_outputs();
            return EpdRunReport {
                attempted: false,
                success: false,
                driver_cleanup_clean: false,
                pon_drf_elapsed_ms: None,
                forced_safe,
            };
        }

        let rates = SpiRates::new(EPD_SPI_WRITE_HZ, None)
            .expect("the fixed EPD SPI write rate is controller-safe");
        let config = OperationConfig::default();
        let interface = EmbeddedHalInterface::new(
            &mut self.epd_spi,
            &mut self.epd_dc,
            &mut self.epd_cs,
            &mut self.epd_reset,
            &mut self.epd_busy,
            &mut self.epd_power,
            FreeRtos,
        );
        let mut display = Ssd2683::new(interface, rates);

        let mut stage = "initialize";
        let mut pon_drf_elapsed_ms = None;
        let sequence_started = Instant::now();
        let result = (|| {
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"partial_initialize\"}}");
            display.initialize(config)?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"partial_initialize\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "partial_vendor_e9_01";
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"partial_vendor_e9_01\"}}");
            display.apply_reference_vendor_compatibility()?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"partial_vendor_e9_01\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "partial_full_frame_load";
            let started = Instant::now();
            log::info!(
                "{{\"event\":\"epd_stage_start\",\"stage\":\"partial_full_frame_load\",\"dtm_bytes\":30000}}"
            );
            display.load_full_frame(frame, config.busy_wait)?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"partial_full_frame_load\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "partial_window_enable";
            let started = Instant::now();
            log::info!(
                "{{\"event\":\"epd_stage_start\",\"stage\":\"partial_window_enable\",\"x_start\":{},\"x_end\":{},\"y_start\":{},\"y_end\":{},\"inclusive\":true}}",
                window.x_start(),
                window.x_end(),
                window.y_start(),
                window.y_end()
            );
            display.enable_experimental_partial_window(window)?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"partial_window_enable\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "partial_pon_drf_refresh";
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"partial_pon_drf_refresh\"}}");
            let refresh_result = display.refresh_loaded_frame(config);
            pon_drf_elapsed_ms = Some(started.elapsed().as_millis());
            refresh_result?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"partial_pon_drf_refresh\",\"elapsed_ms\":{}}}",
                pon_drf_elapsed_ms.expect("the bounded refresh stage just completed")
            );

            stage = "partial_window_disable";
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"partial_window_disable\"}}");
            display.disable_experimental_partial_window(window)?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"partial_window_disable\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );

            stage = "partial_pof_deep_sleep";
            let started = Instant::now();
            log::info!("{{\"event\":\"epd_stage_start\",\"stage\":\"partial_pof_deep_sleep\"}}");
            display.shutdown(config)?;
            log::info!(
                "{{\"event\":\"epd_stage_finish\",\"stage\":\"partial_pof_deep_sleep\",\"elapsed_ms\":{}}}",
                started.elapsed().as_millis()
            );
            Ok(())
        })();

        let (success, driver_cleanup_clean) = match result {
            Ok(()) => {
                log::info!(
                    "{{\"event\":\"epd_driver_result\",\"mode\":\"experimental_partial\",\"status\":\"ok\",\"state\":\"{:?}\",\"vendor_e9_01\":true,\"elapsed_ms\":{}}}",
                    display.status().state,
                    sequence_started.elapsed().as_millis()
                );
                (true, true)
            }
            Err(cause) => {
                let cleanup = display.cleanup_after_error(&cause, config);
                let cleanup_clean = cleanup.is_clean();
                log::error!(
                    "{{\"event\":\"epd_driver_result\",\"mode\":\"experimental_partial\",\"status\":\"error\",\"stage\":\"{}\",\"cause\":\"{:?}\",\"cleanup\":\"{:?}\",\"cleanup_clean\":{},\"elapsed_ms\":{}}}",
                    stage,
                    cause,
                    cleanup,
                    cleanup_clean,
                    sequence_started.elapsed().as_millis()
                );
                (false, cleanup_clean)
            }
        };
        // Explicitly release the interface so its mutable GPIO/SPI borrows end
        // before the board-level independent forced-safe pass.
        let _ = display.release();

        let forced_safe = self.enforce_epd_safe_outputs();
        EpdRunReport {
            attempted: true,
            success,
            driver_cleanup_clean,
            pon_drf_elapsed_ms,
            forced_safe,
        }
    }

    fn enforce_epd_safe_outputs(&mut self) -> EpdSafeReport {
        // Every operation is independent, and rail-off is always attempted last.
        let report = EpdSafeReport {
            cs_high: self.epd_cs.set_high().is_ok(),
            dc_low: self.epd_dc.set_low().is_ok(),
            reset_low: self.epd_reset.set_low().is_ok(),
            rail_off: self.epd_power.set_low().is_ok(),
        };
        if !report.is_clean() {
            self.epd_inhibited = true;
        }
        report
    }

    /// Re-assert all optional outputs to the non-energized state.
    pub fn enforce_safe_outputs(&mut self) -> SafeOutputReport {
        // Do not short-circuit: each rail gets its own best-effort safe write.
        let power_hold_high = self.power_hold.set_high().is_ok();
        let epd = self.enforce_epd_safe_outputs();
        let nfc_off = self.nfc_power.set_low().is_ok();
        let audio_rail_off = self.audio_rail.set_low().is_ok();
        if !audio_rail_off {
            self.i2c_inhibited = true;
        }
        let audio_pa_off = self.audio_pa.set_low().is_ok();
        SafeOutputReport {
            power_hold_high,
            epd,
            nfc_off,
            audio_rail_off,
            audio_pa_off,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpdRunReport {
    pub attempted: bool,
    pub success: bool,
    pub driver_cleanup_clean: bool,
    /// PON + settle + DRF + bounded BUSY waits; not a pure BUSY-low duration.
    pub pon_drf_elapsed_ms: Option<u128>,
    pub forced_safe: EpdSafeReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpdSafeReport {
    pub cs_high: bool,
    pub dc_low: bool,
    pub reset_low: bool,
    pub rail_off: bool,
}

impl EpdSafeReport {
    pub const fn is_clean(self) -> bool {
        self.cs_high && self.dc_low && self.reset_low && self.rail_off
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SafeOutputReport {
    pub power_hold_high: bool,
    pub epd: EpdSafeReport,
    pub nfc_off: bool,
    pub audio_rail_off: bool,
    pub audio_pa_off: bool,
}

impl SafeOutputReport {
    pub const fn is_clean(self) -> bool {
        self.power_hold_high
            && self.epd.is_clean()
            && self.nfc_off
            && self.audio_rail_off
            && self.audio_pa_off
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralAvailability {
    pub i2c: bool,
    pub i2c_init_error: Option<i32>,
    pub battery_adc: bool,
    pub battery_adc_init_error: Option<i32>,
    pub power_standby_status: bool,
    pub power_standby_init_error: Option<i32>,
    pub charge_status: bool,
    pub charge_status_init_error: Option<i32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cFailurePhase {
    Unavailable,
    Inhibited,
    PaLow,
    NfcOff,
    RailOn,
    Transaction,
    RailOff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cAccessError {
    pub phase: I2cFailurePhase,
    pub code: Option<i32>,
    pub rail_off_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cScanReport {
    pub found: [bool; 128],
    pub attempted: u16,
    pub nack_or_fail: u16,
    pub timeout: u16,
    pub other_error: u16,
    pub first_timeout_address: Option<u8>,
    pub first_other_error: Option<(u8, i32)>,
    pub round_deadline_hit: bool,
    pub transaction_elapsed_ms: u128,
}

impl I2cScanReport {
    const fn new() -> Self {
        Self {
            found: [false; 128],
            attempted: 0,
            nack_or_fail: 0,
            timeout: 0,
            other_error: 0,
            first_timeout_address: None,
            first_other_error: None,
            round_deadline_hit: false,
            transaction_elapsed_ms: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RtcReadPair {
    pub first: [u8; 7],
    pub second: [u8; 7],
    pub interval_elapsed_ms: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatteryStatusLevels {
    pub standby_high: bool,
    pub charge_high: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralUnavailable {
    pub component: &'static str,
    pub init_error: Option<i32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralReadError {
    pub component: &'static str,
    pub init_error: Option<i32>,
    pub read_error: Option<i32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatteryAdcSamples {
    pub raw: [u16; BATTERY_ADC_SAMPLE_COUNT],
    pub calibrated_mv: [u16; BATTERY_ADC_SAMPLE_COUNT],
}

impl BatteryAdcSamples {
    pub fn average_raw(self) -> u16 {
        (self
            .raw
            .iter()
            .map(|sample| u32::from(*sample))
            .sum::<u32>()
            / BATTERY_ADC_SAMPLE_COUNT as u32) as u16
    }

    pub fn average_calibrated_mv(self) -> u16 {
        (self
            .calibrated_mv
            .iter()
            .map(|sample| u32::from(*sample))
            .sum::<u32>()
            / BATTERY_ADC_SAMPLE_COUNT as u32) as u16
    }
}
