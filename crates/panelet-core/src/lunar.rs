// SPDX-License-Identifier: MPL-2.0

//! Compact Gregorian-to-Chinese-lunar conversion for the RTC ownership range.
//!
//! The packed year data stores the factual month lengths used by the Chinese
//! calendar. Bits 15..=4 describe months 1..=12 (1 means 30 days), the low
//! nibble is the leap month, and bit 16 is the leap-month length. Keeping only
//! 1999..=2099 avoids pulling a full calendar/astronomy stack into firmware.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LunarDate {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub is_leap_month: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LunarConversionError {
    OutOfRange,
    InvalidDate,
}

const FIRST_LUNAR_YEAR: u16 = 1999;
const FIRST_SOLAR_YEAR: u16 = 2000;
const LAST_SOLAR_YEAR: u16 = 2099;

// 1999-02-16 is lunar 1999-01-01. The extra 1999 entry lets every date in the
// RTC's 2000..=2099 convention convert without a special January boundary.
const BASE_SOLAR_DAY_INDEX: i32 = 31 + 15;

const LUNAR_YEAR_INFO: [u32; 101] = [
    0x092e0, // 1999
    0x0c960, 0x0d954, 0x0d4a0, 0x0da50, 0x07552, 0x056a0, 0x0abb7, 0x025d0, 0x092d0,
    0x0cab5, // 2000..=2009
    0x0a950, 0x0b4a0, 0x0baa4, 0x0ad50, 0x055d9, 0x04ba0, 0x0a5b0, 0x15176, 0x052b0,
    0x0a930, // 2010..=2019
    0x07954, 0x06aa0, 0x0ad50, 0x05b52, 0x04b60, 0x0a6e6, 0x0a4e0, 0x0d260, 0x0ea65,
    0x0d530, // 2020..=2029
    0x05aa0, 0x076a3, 0x096d0, 0x04afb, 0x04ad0, 0x0a4d0, 0x1d0b6, 0x0d250, 0x0d520,
    0x0dd45, // 2030..=2039
    0x0b5a0, 0x056d0, 0x055b2, 0x049b0, 0x0a577, 0x0a4b0, 0x0aa50, 0x1b255, 0x06d20,
    0x0ada0, // 2040..=2049
    0x14b63, 0x09370, 0x049f8, 0x04970, 0x064b0, 0x168a6, 0x0ea50, 0x06aa0, 0x1a6c4,
    0x0aae0, // 2050..=2059
    0x092e0, 0x0d2e3, 0x0c960, 0x0d557, 0x0d4a0, 0x0da50, 0x05d55, 0x056a0, 0x0a6d0,
    0x055d4, // 2060..=2069
    0x052d0, 0x0a9b8, 0x0a950, 0x0b4a0, 0x0b6a6, 0x0ad50, 0x055a0, 0x0aba4, 0x0a5b0,
    0x052b0, // 2070..=2079
    0x0b273, 0x06930, 0x07337, 0x06aa0, 0x0ad50, 0x14b55, 0x04b60, 0x0a570, 0x054e4,
    0x0d160, // 2080..=2089
    0x0e968, 0x0d520, 0x0daa0, 0x16aa6, 0x056d0, 0x04ae0, 0x0a9d4, 0x0a2d0, 0x0d150,
    0x0f252, // 2090..=2099
];

pub fn solar_to_lunar(year: u16, month: u8, day: u8) -> Result<LunarDate, LunarConversionError> {
    if !(FIRST_SOLAR_YEAR..=LAST_SOLAR_YEAR).contains(&year) {
        return Err(LunarConversionError::OutOfRange);
    }
    if month == 0 || month > 12 || day == 0 || day > gregorian_month_days(year, month) {
        return Err(LunarConversionError::InvalidDate);
    }

    let mut offset = solar_day_index(year, month, day) - BASE_SOLAR_DAY_INDEX;
    let mut lunar_year = FIRST_LUNAR_YEAR;
    for info in LUNAR_YEAR_INFO {
        let year_days = lunar_year_days(info);
        if offset < year_days {
            return lunar_date_in_year(lunar_year, info, offset);
        }
        offset -= year_days;
        lunar_year += 1;
    }
    Err(LunarConversionError::OutOfRange)
}

fn lunar_date_in_year(
    year: u16,
    info: u32,
    mut offset: i32,
) -> Result<LunarDate, LunarConversionError> {
    let leap_month = (info & 0x0f) as u8;
    for month in 1..=12 {
        let days = lunar_month_days(info, month);
        if offset < days {
            return Ok(LunarDate {
                year,
                month,
                day: offset as u8 + 1,
                is_leap_month: false,
            });
        }
        offset -= days;

        if leap_month == month {
            let days = if info & 0x1_0000 != 0 { 30 } else { 29 };
            if offset < days {
                return Ok(LunarDate {
                    year,
                    month,
                    day: offset as u8 + 1,
                    is_leap_month: true,
                });
            }
            offset -= days;
        }
    }
    Err(LunarConversionError::OutOfRange)
}

fn lunar_year_days(info: u32) -> i32 {
    let regular = (1..=12)
        .map(|month| lunar_month_days(info, month))
        .sum::<i32>();
    if info & 0x0f == 0 {
        regular
    } else if info & 0x1_0000 != 0 {
        regular + 30
    } else {
        regular + 29
    }
}

fn lunar_month_days(info: u32, month: u8) -> i32 {
    29 + ((info >> (16 - u32::from(month))) & 1) as i32
}

fn solar_day_index(year: u16, month: u8, day: u8) -> i32 {
    let mut days = 0_i32;
    for candidate in 1999..year {
        days += if is_gregorian_leap_year(candidate) {
            366
        } else {
            365
        };
    }
    for candidate in 1..month {
        days += i32::from(gregorian_month_days(year, candidate));
    }
    days + i32::from(day - 1)
}

const fn is_gregorian_leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn gregorian_month_days(year: u16, month: u8) -> u8 {
    match month {
        2 if is_gregorian_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_new_year_and_selected_official_calendar_dates() {
        assert_eq!(
            solar_to_lunar(2000, 2, 5),
            Ok(LunarDate {
                year: 2000,
                month: 1,
                day: 1,
                is_leap_month: false,
            })
        );
        assert_eq!(
            solar_to_lunar(2024, 2, 10),
            Ok(LunarDate {
                year: 2024,
                month: 1,
                day: 1,
                is_leap_month: false,
            })
        );
        assert_eq!(
            solar_to_lunar(2026, 9, 5),
            Ok(LunarDate {
                year: 2026,
                month: 7,
                day: 24,
                is_leap_month: false,
            })
        );
        assert_eq!(solar_to_lunar(2026, 9, 11).unwrap().day, 1);
        assert_eq!(solar_to_lunar(2026, 9, 11).unwrap().month, 8);
        assert_eq!(solar_to_lunar(2026, 9, 30).unwrap().day, 20);
    }

    #[test]
    fn preserves_leap_month_identity() {
        assert_eq!(
            solar_to_lunar(2017, 7, 28),
            Ok(LunarDate {
                year: 2017,
                month: 6,
                day: 6,
                is_leap_month: true,
            })
        );
    }

    #[test]
    fn rejects_dates_outside_the_rtc_range_or_calendar() {
        assert_eq!(
            solar_to_lunar(1999, 12, 31),
            Err(LunarConversionError::OutOfRange)
        );
        assert_eq!(
            solar_to_lunar(2026, 2, 29),
            Err(LunarConversionError::InvalidDate)
        );
    }
}
