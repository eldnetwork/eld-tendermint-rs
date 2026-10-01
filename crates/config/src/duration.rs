//! Go `time.Duration`: a signed nanosecond count parsed from strings like `"3s"`.

use serde::Deserialize;
use serde::de::{self, Deserializer};

use crate::Error;

/// Signed duration in nanoseconds. Negative values exist so `ValidateBasic` can reject them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Duration {
    nanos: i64,
}

impl Duration {
    #[must_use]
    pub const fn from_nanos(nanos: i64) -> Self {
        Self { nanos }
    }

    #[must_use]
    pub const fn as_nanos(self) -> i64 {
        self.nanos
    }

    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self {
            nanos: millis.saturating_mul(1_000_000),
        }
    }

    #[must_use]
    pub const fn from_secs(secs: i64) -> Self {
        Self {
            nanos: secs.saturating_mul(1_000_000_000),
        }
    }

    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.nanos < 0
    }

    /// `time.ParseDuration`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidDuration`] when `text` is not a Go duration string.
    pub fn parse(text: &str) -> Result<Self, Error> {
        parse_go(text).map(Self::from_nanos)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

fn parse_go(input: &str) -> Result<i64, Error> {
    let fail = || Error::InvalidDuration(input.to_owned());
    if input.is_empty() {
        return Err(fail());
    }
    let mut rest = input;
    let mut neg = false;
    if let Some(stripped) = rest.strip_prefix('-') {
        neg = true;
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix('+') {
        rest = stripped;
    }
    if rest == "0" {
        return Ok(0);
    }
    if rest.is_empty() {
        return Err(fail());
    }
    let mut total: i128 = 0;
    while !rest.is_empty() {
        let (whole, next, saw_int) = leading_int(rest).map_err(|_| fail())?;
        rest = next;
        let mut frac = 0i64;
        let mut scale = 1f64;
        let mut saw_frac = false;
        if let Some(stripped) = rest.strip_prefix('.') {
            let (fraction, fraction_scale, next, consumed) = leading_fraction(stripped);
            frac = fraction;
            scale = fraction_scale;
            saw_frac = consumed;
            rest = next;
        }
        if !saw_int && !saw_frac {
            return Err(fail());
        }
        let unit_end = rest
            .char_indices()
            .find(|(_, ch)| *ch == '.' || ch.is_ascii_digit())
            .map(|(idx, _)| idx)
            .unwrap_or(rest.len());
        if unit_end == 0 {
            return Err(fail());
        }
        let unit = &rest[..unit_end];
        rest = &rest[unit_end..];
        let unit_nanos = unit_nanos(unit).ok_or_else(fail)?;
        let mut part = i128::from(whole) * i128::from(unit_nanos);
        if frac > 0 {
            // `time.ParseDuration` scales the fraction with float64.
            let extra = (frac as f64) * ((unit_nanos as f64) / scale);
            if !extra.is_finite() || extra < 0.0 {
                return Err(fail());
            }
            part += extra as i128;
        }
        if part < 0 {
            return Err(fail());
        }
        total += part;
        if total > i128::from(i64::MAX) {
            return Err(fail());
        }
    }
    if neg {
        total = -total;
    }
    i64::try_from(total).map_err(|_| fail())
}

fn leading_int(text: &str) -> Result<(i64, &str, bool), ()> {
    let bytes = text.as_bytes();
    let mut value: i64 = 0;
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        let digit = i64::from(bytes[index] - b'0');
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(digit))
            .ok_or(())?;
        index += 1;
    }
    Ok((value, &text[index..], index > 0))
}

fn leading_fraction(text: &str) -> (i64, f64, &str, bool) {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut value: i64 = 0;
    let mut scale = 1f64;
    let mut overflow = false;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        if !overflow {
            let digit = i64::from(bytes[index] - b'0');
            match value.checked_mul(10).and_then(|v| v.checked_add(digit)) {
                Some(next) => {
                    value = next;
                    scale *= 10.0;
                }
                None => overflow = true,
            }
        }
        index += 1;
    }
    (value, scale, &text[index..], index > 0)
}

fn unit_nanos(unit: &str) -> Option<i64> {
    Some(match unit {
        "ns" => 1,
        "us" | "µs" | "μs" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::Duration;

    #[test]
    fn parses_go_duration_strings() {
        assert_eq!(Duration::parse("0").unwrap().as_nanos(), 0);
        assert_eq!(Duration::parse("-0").unwrap().as_nanos(), 0);
        assert_eq!(Duration::parse("3s").unwrap().as_nanos(), 3_000_000_000);
        assert_eq!(Duration::parse("100ms").unwrap().as_nanos(), 100_000_000);
        assert_eq!(Duration::parse("500ms").unwrap().as_nanos(), 500_000_000);
        assert_eq!(Duration::parse("1.5s").unwrap().as_nanos(), 1_500_000_000);
        assert_eq!(Duration::parse("-1s").unwrap().as_nanos(), -1_000_000_000);
        assert_eq!(Duration::parse("1us").unwrap().as_nanos(), 1_000);
        assert_eq!(Duration::parse("1µs").unwrap().as_nanos(), 1_000);
        assert_eq!(
            Duration::parse("168h").unwrap().as_nanos(),
            168 * 3_600 * 1_000_000_000
        );
        assert_eq!(
            Duration::parse("2h45m").unwrap().as_nanos(),
            2 * 3_600_000_000_000 + 45 * 60_000_000_000
        );
        assert_eq!(
            Duration::parse("1h2m3s").unwrap().as_nanos(),
            3_600_000_000_000 + 2 * 60_000_000_000 + 3_000_000_000
        );
        assert!(Duration::parse("").is_err());
        assert!(Duration::parse("nope").is_err());
        assert!(Duration::parse("1").is_err());
    }
}
