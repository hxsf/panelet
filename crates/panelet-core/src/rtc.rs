// SPDX-License-Identifier: MPL-2.0

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RtcSample {
    pub second: u8,
    pub minute: u8,
    pub hour: u8,
    pub day: u8,
    pub weekday: u8,
    pub month: u8,
    pub year: u8,
    pub century_bit: bool,
    pub voltage_low: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RtcField {
    Second,
    Minute,
    Hour,
    Day,
    Weekday,
    Month,
    Year,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RtcParseError {
    InvalidBcd { field: RtcField, raw: u8 },
    OutOfRange { field: RtcField, value: u8 },
    InvalidCalendarDate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RtcProgress {
    NoAdvance,
    AdvancedBy(u8),
    InvalidTransition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CivilTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    /// Sunday is 0 and Saturday is 6, matching the PCF8563 convention used by
    /// this repository.
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeConversionError {
    OutOfRange,
    InvalidDate,
    InvalidTime,
    UnsupportedRtcCentury,
}

const SECONDS_PER_DAY: i64 = 86_400;

pub fn parse_pcf8563(bytes: [u8; 7]) -> Result<RtcSample, RtcParseError> {
    let sample = RtcSample {
        second: decode_bcd(RtcField::Second, bytes[0] & 0x7f)?,
        minute: decode_bcd(RtcField::Minute, bytes[1] & 0x7f)?,
        hour: decode_bcd(RtcField::Hour, bytes[2] & 0x3f)?,
        day: decode_bcd(RtcField::Day, bytes[3] & 0x3f)?,
        weekday: decode_bcd(RtcField::Weekday, bytes[4] & 0x07)?,
        month: decode_bcd(RtcField::Month, bytes[5] & 0x1f)?,
        year: decode_bcd(RtcField::Year, bytes[6])?,
        century_bit: bytes[5] & 0x80 != 0,
        voltage_low: bytes[0] & 0x80 != 0,
    };

    for (field, value, maximum) in [
        (RtcField::Second, sample.second, 59),
        (RtcField::Minute, sample.minute, 59),
        (RtcField::Hour, sample.hour, 23),
        (RtcField::Day, sample.day, 31),
        (RtcField::Weekday, sample.weekday, 6),
        (RtcField::Month, sample.month, 12),
        (RtcField::Year, sample.year, 99),
    ] {
        let minimum = if matches!(field, RtcField::Day | RtcField::Month) {
            1
        } else {
            0
        };
        if value < minimum || value > maximum {
            return Err(RtcParseError::OutOfRange { field, value });
        }
    }

    if sample.day > days_in_month(sample.year, sample.month) {
        return Err(RtcParseError::InvalidCalendarDate);
    }
    Ok(sample)
}

/// Convert a Unix timestamp into a bounded Gregorian UTC/local calendar.
///
/// The supported 1970..=2199 range is deliberately wider than the external
/// RTC ownership convention, so a configured UTC offset can cross a year
/// boundary without making the local clock unavailable.
pub fn unix_seconds_to_civil(seconds: i64) -> Result<CivilTime, TimeConversionError> {
    let mut days = seconds.div_euclid(SECONDS_PER_DAY);
    let seconds_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    if days < 0 {
        return Err(TimeConversionError::OutOfRange);
    }

    let weekday = ((days + 4) % 7) as u8;
    let mut year = 1970_u16;
    loop {
        let year_days = if is_leap_year(year) { 366 } else { 365 };
        if days < year_days {
            break;
        }
        days -= year_days;
        year = year.checked_add(1).ok_or(TimeConversionError::OutOfRange)?;
        if year > 2199 {
            return Err(TimeConversionError::OutOfRange);
        }
    }

    let mut month = 1_u8;
    loop {
        let month_days = days_in_gregorian_month(year, month);
        if days < i64::from(month_days) {
            break;
        }
        days -= i64::from(month_days);
        month += 1;
    }

    Ok(CivilTime {
        year,
        month,
        day: days as u8 + 1,
        weekday,
        hour: (seconds_of_day / 3600) as u8,
        minute: ((seconds_of_day % 3600) / 60) as u8,
        second: (seconds_of_day % 60) as u8,
    })
}

pub fn civil_to_unix_seconds(time: CivilTime) -> Result<i64, TimeConversionError> {
    if !(1970..=2199).contains(&time.year) {
        return Err(TimeConversionError::OutOfRange);
    }
    if time.month == 0
        || time.month > 12
        || time.day == 0
        || time.day > days_in_gregorian_month(time.year, time.month)
    {
        return Err(TimeConversionError::InvalidDate);
    }
    if time.weekday > 6 || time.hour > 23 || time.minute > 59 || time.second > 59 {
        return Err(TimeConversionError::InvalidTime);
    }

    let mut days = 0_i64;
    for year in 1970..time.year {
        days += if is_leap_year(year) { 366 } else { 365 };
    }
    for month in 1..time.month {
        days += i64::from(days_in_gregorian_month(time.year, month));
    }
    days += i64::from(time.day - 1);
    Ok(days * SECONDS_PER_DAY
        + i64::from(time.hour) * 3600
        + i64::from(time.minute) * 60
        + i64::from(time.second))
}

/// Encode UTC into one PCF8563 `0x02..=0x08` payload.
///
/// NOTE4C-owned RTC content uses `C=0` for 2000..=2099. The caller must send
/// all seven returned bytes in a single I2C transaction after register 0x02.
pub fn unix_seconds_to_pcf8563_utc(seconds: i64) -> Result<[u8; 7], TimeConversionError> {
    let time = unix_seconds_to_civil(seconds)?;
    if !(2000..=2099).contains(&time.year) {
        return Err(TimeConversionError::OutOfRange);
    }
    Ok([
        encode_bcd(time.second),
        encode_bcd(time.minute),
        encode_bcd(time.hour),
        encode_bcd(time.day),
        encode_bcd(time.weekday),
        encode_bcd(time.month),
        encode_bcd((time.year - 2000) as u8),
    ])
}

/// Decode RTC content previously written with
/// [`unix_seconds_to_pcf8563_utc`].
pub fn pcf8563_utc_to_unix_seconds(sample: RtcSample) -> Result<i64, TimeConversionError> {
    if sample.century_bit {
        return Err(TimeConversionError::UnsupportedRtcCentury);
    }
    civil_to_unix_seconds(CivilTime {
        year: 2000 + u16::from(sample.year),
        month: sample.month,
        day: sample.day,
        weekday: sample.weekday,
        hour: sample.hour,
        minute: sample.minute,
        second: sample.second,
    })
}

/// Compare two valid samples by enumerating up to `max_seconds` safe calendar steps.
///
/// The century bit is treated only as an opaque rollover marker; no Gregorian
/// century number is assigned to either polarity.
pub fn rtc_progress(first: RtcSample, second: RtcSample, max_seconds: u8) -> RtcProgress {
    if same_time(first, second) {
        return RtcProgress::NoAdvance;
    }
    let mut candidate = first;
    for elapsed in 1..=max_seconds {
        candidate = add_one_second(candidate);
        if same_time(candidate, second) {
            return RtcProgress::AdvancedBy(elapsed);
        }
    }
    RtcProgress::InvalidTransition
}

fn decode_bcd(field: RtcField, raw: u8) -> Result<u8, RtcParseError> {
    let high = raw >> 4;
    let low = raw & 0x0f;
    if high > 9 || low > 9 {
        return Err(RtcParseError::InvalidBcd { field, raw });
    }
    Ok(high * 10 + low)
}

fn days_in_month(year: u8, month: u8) -> u8 {
    match month {
        2 if year % 4 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

const fn is_leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn days_in_gregorian_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

const fn encode_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

fn same_time(left: RtcSample, right: RtcSample) -> bool {
    left.second == right.second
        && left.minute == right.minute
        && left.hour == right.hour
        && left.day == right.day
        && left.weekday == right.weekday
        && left.month == right.month
        && left.year == right.year
        && left.century_bit == right.century_bit
}

fn add_one_second(mut sample: RtcSample) -> RtcSample {
    sample.voltage_low = false;
    if sample.second < 59 {
        sample.second += 1;
        return sample;
    }
    sample.second = 0;
    if sample.minute < 59 {
        sample.minute += 1;
        return sample;
    }
    sample.minute = 0;
    if sample.hour < 23 {
        sample.hour += 1;
        return sample;
    }
    sample.hour = 0;
    sample.weekday = (sample.weekday + 1) % 7;
    let month_days = days_in_month(sample.year, sample.month);
    if sample.day < month_days {
        sample.day += 1;
        return sample;
    }
    sample.day = 1;
    if sample.month < 12 {
        sample.month += 1;
        return sample;
    }
    sample.month = 1;
    if sample.year < 99 {
        sample.year += 1;
    } else {
        sample.year = 0;
        sample.century_bit = !sample.century_bit;
    }
    sample
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normal_sample_and_reports_flags_without_century_mapping() {
        let sample = parse_pcf8563([0x58, 0x59, 0x23, 0x31, 0x02, 0x92, 0x26]).unwrap();
        assert_eq!(sample.second, 58);
        assert_eq!(sample.month, 12);
        assert!(sample.century_bit);
        assert!(!sample.voltage_low);
    }

    #[test]
    fn rejects_invalid_bcd_calendar_and_exposes_voltage_low() {
        assert!(matches!(
            parse_pcf8563([0x6a, 0, 0, 1, 0, 1, 0]),
            Err(RtcParseError::InvalidBcd {
                field: RtcField::Second,
                ..
            })
        ));
        assert_eq!(
            parse_pcf8563([0, 0, 0, 0x31, 0, 0x04, 0x25]),
            Err(RtcParseError::InvalidCalendarDate)
        );
        assert!(parse_pcf8563([0x80, 0, 0, 1, 0, 1, 0]).unwrap().voltage_low);
    }

    #[test]
    fn detects_minute_and_day_rollovers() {
        let minute_before = parse_pcf8563([0x59, 0x12, 0x10, 1, 1, 1, 0x25]).unwrap();
        let minute_after = parse_pcf8563([0x00, 0x13, 0x10, 1, 1, 1, 0x25]).unwrap();
        assert_eq!(
            rtc_progress(minute_before, minute_after, 3),
            RtcProgress::AdvancedBy(1)
        );

        let day_before = parse_pcf8563([0x59, 0x59, 0x23, 0x31, 5, 0x01, 0x25]).unwrap();
        let day_after = parse_pcf8563([0, 0, 0, 1, 6, 0x02, 0x25]).unwrap();
        assert_eq!(
            rtc_progress(day_before, day_after, 3),
            RtcProgress::AdvancedBy(1)
        );
        assert_eq!(
            rtc_progress(day_after, day_after, 3),
            RtcProgress::NoAdvance
        );
    }

    #[test]
    fn unix_and_civil_conversions_cover_leap_days_and_offsets() {
        let leap = unix_seconds_to_civil(1_709_251_199).unwrap();
        assert_eq!(
            leap,
            CivilTime {
                year: 2024,
                month: 2,
                day: 29,
                weekday: 4,
                hour: 23,
                minute: 59,
                second: 59,
            }
        );
        assert_eq!(civil_to_unix_seconds(leap), Ok(1_709_251_199));

        let shanghai = unix_seconds_to_civil(1_709_251_199 + 8 * 3600).unwrap();
        assert_eq!((shanghai.month, shanghai.day, shanghai.hour), (3, 1, 7));
        assert_eq!(shanghai.weekday, 5);
    }

    #[test]
    fn rtc_utc_encoding_is_single_payload_with_an_explicit_century_convention() {
        let payload = unix_seconds_to_pcf8563_utc(1_725_366_896).unwrap();
        assert_eq!(payload, [0x56, 0x34, 0x12, 0x03, 0x02, 0x09, 0x24]);
        let sample = parse_pcf8563(payload).unwrap();
        assert_eq!(pcf8563_utc_to_unix_seconds(sample), Ok(1_725_366_896));

        let mut unsupported = sample;
        unsupported.century_bit = true;
        assert_eq!(
            pcf8563_utc_to_unix_seconds(unsupported),
            Err(TimeConversionError::UnsupportedRtcCentury)
        );
        assert_eq!(
            unix_seconds_to_pcf8563_utc(946_684_800 - 1),
            Err(TimeConversionError::OutOfRange)
        );
    }
}
