//! Local date and time from a Unix timestamp and a UTC offset, for the Today
//! tab and the clock in the pill. Pure, no timezone database: the platform
//! supplies the offset.

/// A moment in local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    /// Year, e.g. 2026.
    pub year: i64,
    /// 1..=12.
    pub month: u32,
    /// 1..=31.
    pub day: u32,
    /// 0 = Monday .. 6 = Sunday.
    pub weekday: u32,
    /// 0..=23.
    pub hour: u32,
    /// 0..=59.
    pub minute: u32,
}

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian
/// calendar (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl LocalTime {
    /// Local time for `unix` seconds at `offset` seconds east of UTC.
    pub fn at(unix: i64, offset: i32) -> Self {
        let local = unix + i64::from(offset);
        let days = local.div_euclid(86_400);
        let secs = local.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            // 1970-01-01 was a Thursday (index 3).
            weekday: (days + 3).rem_euclid(7) as u32,
            hour: (secs / 3_600) as u32,
            minute: ((secs % 3_600) / 60) as u32,
        }
    }

    /// "14:05".
    pub fn time(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// "Sunday, 15 November".
    pub fn date_long(&self) -> String {
        format!(
            "{}, {} {}",
            WEEKDAYS[self.weekday as usize % 7],
            self.day,
            MONTHS[(self.month as usize + 11) % 12]
        )
    }

    /// "Sun 15 Nov".
    pub fn date_short(&self) -> String {
        format!(
            "{} {} {}",
            &WEEKDAYS[self.weekday as usize % 7][..3],
            self.day,
            &MONTHS[(self.month as usize + 11) % 12][..3]
        )
    }
}

/// Seconds until the next whole minute (1..=60), for waking the clock.
pub fn until_next_minute(unix: i64) -> u64 {
    (60 - unix.rem_euclid(60)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates_and_weekdays() {
        // 2023-11-14 22:13:20 UTC, a Tuesday.
        let t = LocalTime::at(1_700_000_000, 0);
        assert_eq!((t.year, t.month, t.day, t.weekday), (2023, 11, 14, 1));
        assert_eq!(t.time(), "22:13");
        assert_eq!(t.date_long(), "Tuesday, 14 November");
        assert_eq!(t.date_short(), "Tue 14 Nov");
        // The same instant in Jakarta (UTC+7) is already Wednesday.
        let j = LocalTime::at(1_700_000_000, 7 * 3_600);
        assert_eq!((j.day, j.weekday, j.time().as_str()), (15, 2, "05:13"));
        // Leap day and a negative offset across midnight.
        let leap = LocalTime::at(951_782_400, 0); // 2000-02-29 00:00 UTC
        assert_eq!((leap.year, leap.month, leap.day), (2000, 2, 29));
        let ny = LocalTime::at(951_782_400, -5 * 3_600);
        assert_eq!((ny.month, ny.day, ny.hour), (2, 28, 19));
        // Before 1970.
        let old = LocalTime::at(-86_400, 0);
        assert_eq!(
            (old.year, old.month, old.day, old.weekday),
            (1969, 12, 31, 2)
        );
    }

    #[test]
    fn wakes_on_the_next_minute() {
        assert_eq!(until_next_minute(1_700_000_000), 40);
        assert_eq!(until_next_minute(1_699_999_980), 60);
    }
}
