use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const GO_ZERO_TIME: &str = "0001-01-01T00:00:00Z";

/// A UTC instant with Go-compatible zero and `RFC3339Nano` JSON behavior.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp {
    unix_nanoseconds: i128,
    zero: bool,
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::ZERO
    }
}

impl Timestamp {
    pub const ZERO: Self = Self {
        unix_nanoseconds: 0,
        zero: true,
    };

    #[must_use]
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        Self {
            unix_nanoseconds: seconds as i128 * 1_000_000_000,
            zero: false,
        }
    }

    #[must_use]
    pub const fn from_unix_nanoseconds(nanoseconds: i128) -> Self {
        Self {
            unix_nanoseconds: nanoseconds,
            zero: false,
        }
    }

    #[must_use]
    pub fn now_utc() -> Self {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(value) => Self::from_unix_nanoseconds(
                i128::from(value.as_secs()) * 1_000_000_000 + i128::from(value.subsec_nanos()),
            ),
            Err(error) => {
                let value = error.duration();
                Self::from_unix_nanoseconds(
                    -(i128::from(value.as_secs()) * 1_000_000_000
                        + i128::from(value.subsec_nanos())),
                )
            }
        }
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.zero
    }

    #[must_use]
    pub const fn unix_nanoseconds(self) -> Option<i128> {
        if self.zero {
            None
        } else {
            Some(self.unix_nanoseconds)
        }
    }

    #[must_use]
    pub fn add_seconds(self, seconds: i64) -> Self {
        if self.zero {
            self
        } else {
            Self::from_unix_nanoseconds(
                self.unix_nanoseconds
                    .saturating_add(i128::from(seconds) * 1_000_000_000),
            )
        }
    }

    #[must_use]
    pub fn add_nanoseconds(self, nanoseconds: i128) -> Self {
        if self.zero {
            self
        } else {
            Self::from_unix_nanoseconds(self.unix_nanoseconds.saturating_add(nanoseconds))
        }
    }

    /// Parses a Go-compatible zero value or RFC 3339 timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the timestamp is not valid RFC 3339.
    pub fn parse(value: &str) -> Result<Self, String> {
        if value == GO_ZERO_TIME {
            return Ok(Self::ZERO);
        }
        let parsed = OffsetDateTime::parse(value, &Rfc3339)
            .map_err(|_| "invalid agent timestamp".to_owned())?;
        Ok(Self::from_unix_nanoseconds(parsed.unix_timestamp_nanos()))
    }

    fn format(self) -> Result<String, String> {
        if self.zero {
            return Ok(GO_ZERO_TIME.to_owned());
        }
        OffsetDateTime::from_unix_timestamp_nanos(self.unix_nanoseconds)
            .map_err(|_| "agent timestamp is outside RFC3339 range".to_owned())?
            .format(&Rfc3339)
            .map_err(|_| "agent timestamp cannot be formatted".to_owned())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.format() {
            Ok(value) => formatter.write_str(&value),
            Err(_) => formatter.write_str("invalid-timestamp"),
        }
    }
}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.format().map_err(serde::ser::Error::custom)?)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}
