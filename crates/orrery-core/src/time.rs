//! Time scales.
//!
//! The ephemeris wants Barycentric Dynamical Time (TDB); the system clock gives
//! UTC. We go UTC -> TT via a fixed leap-second offset and treat TT as TDB, which
//! differs by under 2 ms — far below the arcsecond level this orrery draws at.

use std::time::{SystemTime, UNIX_EPOCH};

/// Julian Date of the J2000.0 epoch (2000-01-01T12:00:00 TT).
pub const J2000: f64 = 2451545.0;

/// Days in a Julian century.
pub const DAYS_PER_CENTURY: f64 = 36525.0;

/// TAI - UTC, in seconds. Fixed at 37 since 2017-01-01; no leap second has been
/// introduced since, and none is scheduled. TT = TAI + 32.184 s.
const TAI_MINUS_UTC: f64 = 37.0;
const TT_MINUS_TAI: f64 = 32.184;

/// An instant expressed as a Julian Date in Terrestrial Time.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct JulianDate(pub f64);

impl JulianDate {
    /// Julian Date (TT) for the current system time.
    ///
    /// Falls back to J2000 if the clock is set before the Unix epoch, which is
    /// the only way `duration_since` can fail here.
    pub fn now() -> Self {
        let unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(946_728_000.0);
        Self::from_unix_seconds(unix_secs)
    }

    /// Julian Date (TT) from a Unix timestamp (seconds since 1970-01-01 UTC).
    pub fn from_unix_seconds(unix_secs: f64) -> Self {
        let tt_secs = unix_secs + TAI_MINUS_UTC + TT_MINUS_TAI;
        Self(tt_secs / 86_400.0 + 2_440_587.5)
    }

    /// Julian Date (TT) from a proleptic Gregorian calendar date and time (UTC).
    ///
    /// `day` may be fractional; `month` is 1-12.
    pub fn from_gregorian_utc(year: i32, month: u32, day: f64) -> Self {
        // Meeus, *Astronomical Algorithms*, ch. 7.
        let (y, m) = if month <= 2 {
            (year - 1, month as i32 + 12)
        } else {
            (year, month as i32)
        };
        let a = (y as f64 / 100.0).floor();
        let b = 2.0 - a + (a / 4.0).floor();
        let jd_utc =
            (365.25 * (y as f64 + 4716.0)).floor() + (30.6001 * (m as f64 + 1.0)).floor() + day + b
                - 1524.5;
        Self(jd_utc + (TAI_MINUS_UTC + TT_MINUS_TAI) / 86_400.0)
    }

    /// Julian centuries elapsed since J2000.0. This is the `T` of the
    /// Keplerian element tables in [`crate::ephemeris`].
    pub fn centuries_since_j2000(self) -> f64 {
        (self.0 - J2000) / DAYS_PER_CENTURY
    }

    /// Days elapsed since J2000.0.
    pub fn days_since_j2000(self) -> f64 {
        self.0 - J2000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn j2000_epoch_round_trips() {
        // 2000-01-01T12:00:00 TT is JD 2451545.0 by definition. Expressed as a
        // UTC calendar date that instant is 12:00:00 - 69.184 s.
        let jd = JulianDate::from_gregorian_utc(2000, 1, 1.5);
        assert!((jd.0 - (J2000 + 69.184 / 86_400.0)).abs() < 1e-9);
    }

    #[test]
    fn unix_epoch_is_jd_2440587_5() {
        let jd = JulianDate::from_unix_seconds(0.0);
        assert!((jd.0 - 2_440_587.5 - 69.184 / 86_400.0).abs() < 1e-9);
    }

    #[test]
    fn gregorian_and_unix_agree() {
        // 2026-08-03T00:00:00Z == 1785715200 unix.
        let from_unix = JulianDate::from_unix_seconds(1_785_715_200.0);
        let from_cal = JulianDate::from_gregorian_utc(2026, 8, 3.0);
        assert!((from_unix.0 - from_cal.0).abs() < 1e-9);
    }

    #[test]
    fn centuries_since_j2000_is_zero_at_epoch() {
        assert_eq!(JulianDate(J2000).centuries_since_j2000(), 0.0);
    }
}
