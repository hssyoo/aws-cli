//! The local time zone as CPython's `time` module computes it at start-up
//! (`init_timezone` in `Modules/timemodule.c`), which dateutil's `tzlocal`
//! builds its offsets from.

/// CPython's `YEAR`: 365 days and 6 hours.
const YEAR: i64 = (365 * 24 + 6) * 3_600;

/// `time.timezone`, `time.altzone` and `time.daylight`, in seconds west of
/// UTC as Python gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalZone {
    pub timezone: i64,
    pub altzone: i64,
    pub daylight: bool,
}

impl LocalZone {
    /// The zone of this process, from `localtime_r` at the start of this
    /// year and half a year later; `None` when the C library cannot say.
    pub fn system() -> Option<Self> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())?;
        Self::from_probe(now, gmtoff)
    }

    /// `init_timezone` with `offset_at(t)` standing for `localtime(t).tm_gmtoff`:
    /// January's and July's offsets, the greater west-of-UTC one being
    /// standard time, as the southern hemisphere needs.
    pub fn from_probe(now: i64, offset_at: impl Fn(i64) -> Option<i64>) -> Option<Self> {
        let start = now / YEAR * YEAR;
        let january = -offset_at(start)?;
        let july = -offset_at(start + YEAR / 2)?;
        let (timezone, altzone) = if january < july {
            (july, january)
        } else {
            (january, july)
        };
        Some(Self {
            timezone,
            altzone,
            daylight: january != july,
        })
    }

    /// The offset east of UTC dateutil's `tzlocal` gives every instant when
    /// the zone has no daylight-saving time; `None` when it has, because
    /// `tzlocal` then switches between today's two offsets by
    /// `localtime().tm_isdst`, which no fixed offset reproduces.
    pub fn fixed_offset(self) -> Option<i64> {
        (!self.daylight).then_some(-self.timezone)
    }
}

/// `localtime_r(t).tm_gmtoff`.
fn gmtoff(at: i64) -> Option<i64> {
    let time = libc::time_t::try_from(at).ok()?;
    // SAFETY: an all-zero `tm` is a valid value of the plain C struct, which
    // `localtime_r` overwrites.
    let mut civil: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to live locals for the duration of the call;
    // `localtime_r` is the reentrant form and keeps neither.
    let filled = unsafe { libc::localtime_r(&raw const time, &raw mut civil) };
    (!filled.is_null()).then(|| i64::from(civil.tm_gmtoff))
}

#[cfg(test)]
mod tests;
