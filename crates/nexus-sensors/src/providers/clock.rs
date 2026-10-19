//! Local wall-clock time through the C library, which honours TZ and
//! /etc/localtime exactly like the rest of the desktop.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalClock {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    /// 0 = Monday.
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub fn local_clock() -> LocalClock {
    // SAFETY: time(NULL) and localtime_r only write into values we own.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return LocalClock::default();
        }
        LocalClock {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u8,
            day: tm.tm_mday as u8,
            // tm_wday counts from Sunday.
            weekday: ((tm.tm_wday + 6) % 7) as u8,
            hour: tm.tm_hour as u8,
            minute: tm.tm_min as u8,
            second: tm.tm_sec as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn local_clock_is_plausible() {
        let now = super::local_clock();
        assert!(now.year >= 2024);
        assert!((1..=12).contains(&now.month) && (1..=31).contains(&now.day));
        assert!(now.weekday < 7 && now.hour < 24 && now.minute < 60 && now.second < 61);
    }
}
