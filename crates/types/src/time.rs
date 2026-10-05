//! Tendermint timestamps.
//!
//! Go's zero `time.Time` is 1 January year 1 UTC (`seconds = -62135596800`), not the
//! Unix epoch. Canonical sign bytes still encode that timestamp.

use std::time::{SystemTime, UNIX_EPOCH};

use prost::Message;
use prost_types::Timestamp;

use crate::Error;

/// Seconds of Go's zero `time.Time` since the Unix epoch.
pub const GO_ZERO_UNIX: i64 = -62_135_596_800;

/// UTC timestamp stored as Unix seconds and nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Time {
    seconds: i64,
    nanos: i32,
}

impl Time {
    /// Go zero time. Sign bytes include this value; they do not omit the field.
    pub const GO_ZERO: Self = Self {
        seconds: GO_ZERO_UNIX,
        nanos: 0,
    };

    #[must_use]
    pub const fn from_unix_parts(seconds: i64, nanos: i32) -> Self {
        Self { seconds, nanos }
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.seconds == GO_ZERO_UNIX && self.nanos == 0
    }

    #[must_use]
    pub const fn unix_seconds(self) -> i64 {
        self.seconds
    }

    #[must_use]
    pub const fn nanos(self) -> i32 {
        self.nanos
    }

    /// Unix time in nanoseconds, for ordering and `WeightedMedian`.
    #[must_use]
    pub fn unix_nanos(self) -> i128 {
        i128::from(self.seconds) * 1_000_000_000 + i128::from(self.nanos)
    }

    /// `time.Time.Add` of `millis` milliseconds. The result is normalized to `0..1e9` nanos.
    #[must_use]
    pub fn add_millis(self, millis: i64) -> Self {
        let extra = i128::from(millis).saturating_mul(1_000_000);
        let total = self.unix_nanos().saturating_add(extra);
        let seconds = total.div_euclid(1_000_000_000);
        let nanos = total.rem_euclid(1_000_000_000);
        Self {
            seconds: i64::try_from(seconds).unwrap_or(if seconds < 0 {
                i64::MIN
            } else {
                i64::MAX
            }),
            nanos: i32::try_from(nanos).unwrap_or(0),
        }
    }

    /// Current UTC time, matching `types/time.Now` aside from the exact instant.
    #[must_use]
    pub fn now() -> Self {
        let dur = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            seconds: i64::try_from(dur.as_secs()).unwrap_or(i64::MAX),
            nanos: i32::try_from(dur.subsec_nanos()).unwrap_or(0),
        }
    }

    /// `google.protobuf.Timestamp` for hashing and sign bytes.
    #[must_use]
    pub fn to_prost(self) -> Timestamp {
        Timestamp {
            seconds: self.seconds,
            nanos: self.nanos,
        }
    }

    /// Protobuf encoding of this time (`gogotypes.StdTimeMarshal`).
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        self.to_prost().encode_to_vec()
    }

    /// Reads a protobuf timestamp. A missing field is Go's zero time.
    #[must_use]
    pub fn from_prost(timestamp: Option<&Timestamp>) -> Self {
        match timestamp {
            Some(ts) => Self {
                seconds: ts.seconds,
                nanos: ts.nanos,
            },
            None => Self::GO_ZERO,
        }
    }

    /// Parses RFC3339 / RFC3339Nano the way Go's `time.Parse` does for genesis and votes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidTime`] when `input` is not a timestamp with a `Z` or numeric offset.
    pub fn parse_rfc3339(input: &str) -> Result<Self, Error> {
        let bytes = input.as_bytes();
        if bytes.len() < 20 {
            return Err(Error::InvalidTime);
        }
        let year = parse_digits(&bytes[0..4])?;
        expect(bytes, 4, b'-')?;
        let month = parse_digits(&bytes[5..7])?;
        expect(bytes, 7, b'-')?;
        let day = parse_digits(&bytes[8..10])?;
        expect(bytes, 10, b'T')?;
        let hour = parse_digits(&bytes[11..13])?;
        expect(bytes, 13, b':')?;
        let minute = parse_digits(&bytes[14..16])?;
        expect(bytes, 16, b':')?;
        let second = parse_digits(&bytes[17..19])?;

        let mut index = 19;
        let mut nanos = 0i32;
        if index < bytes.len() && bytes[index] == b'.' {
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            let digits = &bytes[start..index];
            if digits.is_empty() || digits.len() > 9 {
                return Err(Error::InvalidTime);
            }
            let mut value = 0i32;
            for digit in digits {
                value = value * 10 + i32::from(digit - b'0');
            }
            for _ in digits.len()..9 {
                value *= 10;
            }
            nanos = value;
        }

        if index >= bytes.len() {
            return Err(Error::InvalidTime);
        }
        let offset = parse_offset(bytes, index)?;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 60
        {
            return Err(Error::InvalidTime);
        }

        let days = days_from_civil(
            year,
            u32::try_from(month).unwrap_or(0),
            u32::try_from(day).unwrap_or(0),
        );
        let seconds =
            days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second)
                - offset;
        Ok(Self { seconds, nanos })
    }

    /// Go's `time.RFC3339Nano`: fractional seconds drop trailing zeros.
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        let days = self.seconds.div_euclid(86_400);
        let mut sod = self.seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let hour = sod / 3_600;
        sod %= 3_600;
        let minute = sod / 60;
        let second = sod % 60;
        if self.nanos == 0 {
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
        } else {
            let mut frac = format!("{:09}", self.nanos.max(0));
            while frac.ends_with('0') {
                frac.pop();
            }
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{frac}Z")
        }
    }
}

impl Default for Time {
    fn default() -> Self {
        Self::GO_ZERO
    }
}

impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Time {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.unix_nanos().cmp(&other.unix_nanos())
    }
}

fn expect(bytes: &[u8], index: usize, want: u8) -> Result<(), Error> {
    if bytes.get(index) == Some(&want) {
        Ok(())
    } else {
        Err(Error::InvalidTime)
    }
}

fn parse_digits(bytes: &[u8]) -> Result<i32, Error> {
    let mut value = 0i32;
    if bytes.is_empty() {
        return Err(Error::InvalidTime);
    }
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return Err(Error::InvalidTime);
        }
        value = value * 10 + i32::from(byte - b'0');
    }
    Ok(value)
}

fn parse_offset(bytes: &[u8], index: usize) -> Result<i64, Error> {
    match bytes[index] {
        b'Z' if index + 1 == bytes.len() => Ok(0),
        b'+' | b'-' => {
            let sign: i64 = if bytes[index] == b'+' { 1 } else { -1 };
            let rest = &bytes[index + 1..];
            let (hour, minute) = if rest.len() == 5 && rest[2] == b':' {
                (parse_digits(&rest[0..2])?, parse_digits(&rest[3..5])?)
            } else if rest.len() == 4 {
                (parse_digits(&rest[0..2])?, parse_digits(&rest[2..4])?)
            } else {
                return Err(Error::InvalidTime);
            };
            if hour > 23 || minute > 59 {
                return Err(Error::InvalidTime);
            }
            Ok(sign * (i64::from(hour) * 3_600 + i64::from(minute) * 60))
        }
        _ => Err(Error::InvalidTime),
    }
}

/// Days since the Unix epoch for a civil date (Howard Hinnant).
fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    year -= i32::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = u32::try_from(year.rem_euclid(400)).unwrap_or(0);
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = yoe * 365 + yoe / 4 - yoe / 100 + day_of_year;
    i64::from(era) * 146_097 + i64::from(day_of_era) - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = u64::try_from(z - era * 146_097).unwrap_or(0);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = era * 400 + i64::try_from(year_of_era).unwrap_or(0);
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    if month <= 2 {
        year += 1;
    }
    (
        i32::try_from(year).unwrap_or(0),
        u32::try_from(month).unwrap_or(0),
        u32::try_from(day).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::Time;

    #[test]
    fn go_zero_time_round_trips() {
        assert!(Time::GO_ZERO.is_zero());
        assert_eq!(Time::GO_ZERO.unix_seconds(), -62_135_596_800);
        assert_eq!(Time::GO_ZERO.to_rfc3339(), "0001-01-01T00:00:00Z");
        assert_eq!(
            Time::parse_rfc3339("0001-01-01T00:00:00Z").unwrap(),
            Time::GO_ZERO
        );
    }

    #[test]
    fn rfc3339_fraction_and_offset() {
        let parsed = Time::parse_rfc3339("2018-02-11T07:09:22.765Z").unwrap();
        assert_eq!(parsed.nanos(), 765_000_000);
        assert_eq!(parsed.to_rfc3339(), "2018-02-11T07:09:22.765Z");

        let zulu = Time::parse_rfc3339("2019-10-13T16:14:44Z").unwrap();
        let offset = Time::parse_rfc3339("2019-10-13T18:14:44+02:00").unwrap();
        assert_eq!(zulu, offset);
        assert_eq!(zulu.to_rfc3339(), "2019-10-13T16:14:44Z");
    }
}
