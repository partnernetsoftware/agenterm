//! linux local civil time.

use crate::contract::local_clock::LocalCivilTime;
use std::mem::MaybeUninit;
use std::time::{SystemTime, UNIX_EPOCH};

#[repr(C)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *const u8,
}

// SAFETY: libc localtime_r matches the platform ABI.
#[link(name = "c")]
unsafe extern "C" {
    fn localtime_r(timep: *const i64, result: *mut Tm) -> *mut Tm;
}

fn unix_seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn local_tm(unix_seconds: i64) -> Tm {
    let mut tm = MaybeUninit::<Tm>::uninit();
    let pointer = unsafe { localtime_r(&unix_seconds, tm.as_mut_ptr()) };
    if pointer.is_null() {
        return Tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 1,
            tm_mon: 0,
            tm_year: 70,
            tm_wday: 4,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null(),
        };
    }
    unsafe { tm.assume_init() }
}

fn civil_from_tm(tm: &Tm) -> LocalCivilTime {
    LocalCivilTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u8,
        day: tm.tm_mday as u8,
        weekday: tm.tm_wday as u8,
        hour: tm.tm_hour as u8,
        minute: tm.tm_min as u8,
        second: tm.tm_sec as u8,
    }
}

/// Reads the host's local civil time.
///
/// Uses `localtime_r(3)`, which honours `TZ` and the system zoneinfo database
/// including DST.
pub(crate) fn local_civil_now() -> LocalCivilTime {
    civil_from_tm(&local_tm(unix_seconds_now()))
}

/// Host UTC offset in seconds east of UTC (GNU `tm_gmtoff`).
pub(crate) fn local_utc_offset_seconds() -> i32 {
    let offset = local_tm(unix_seconds_now()).tm_gmtoff;
    i32::try_from(offset).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{local_civil_now, local_utc_offset_seconds};
    use crate::contract::local_clock::civil_from_unix_seconds;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn local_offset_matches_utc_minus_local_civil_at_now() {
        let unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64);
        let utc = civil_from_unix_seconds(unix_seconds);
        let local = local_civil_now();
        let offset = local_utc_offset_seconds();
        let utc_seconds =
            i32::from(utc.hour) * 3_600 + i32::from(utc.minute) * 60 + i32::from(utc.second);
        let local_seconds =
            i32::from(local.hour) * 3_600 + i32::from(local.minute) * 60 + i32::from(local.second);
        let delta = local_seconds - utc_seconds;
        assert!(
            delta == offset || delta == offset - 86_400 || delta == offset + 86_400,
            "offset {offset} did not match local-utc delta {delta}"
        );
    }
}
