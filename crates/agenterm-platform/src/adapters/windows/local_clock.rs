//! Windows local civil time via `GetLocalTime`.

use crate::contract::local_clock::LocalCivilTime;

/// Reads the host's local civil time, honouring the user's time zone.
pub(crate) fn local_civil_now() -> LocalCivilTime {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    // SAFETY: GetLocalTime only writes the provided SYSTEMTIME buffer.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut time = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    unsafe {
        GetLocalTime(&raw mut time);
    }
    LocalCivilTime {
        year: i32::from(time.year),
        month: time.month as u8,
        day: time.day as u8,
        weekday: time.day_of_week as u8,
        hour: time.hour as u8,
        minute: time.minute as u8,
        second: time.second as u8,
    }
}

/// Reads the host's UTC offset in seconds east of UTC.
pub(crate) fn local_utc_offset_seconds() -> i32 {
    let local = local_civil_now();
    let utc = crate::contract::local_clock::civil_from_unix_seconds(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64),
    );
    let local_seconds =
        i32::from(local.hour) * 3_600 + i32::from(local.minute) * 60 + i32::from(local.second);
    let utc_seconds =
        i32::from(utc.hour) * 3_600 + i32::from(utc.minute) * 60 + i32::from(utc.second);
    let day_delta = i32::from(local.day) - i32::from(utc.day);
    local_seconds - utc_seconds + day_delta * 86_400
}
