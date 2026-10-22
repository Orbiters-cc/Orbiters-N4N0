//! Number and time formatting shared by the interfaces.

/// Bytes in SI units, one decimal under ten: "8.2 MB", "82 MB".
pub fn bytes(n: f64) -> String {
    if !n.is_finite() {
        return "–".into();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let sign = if n < 0.0 { "-" } else { "" };
    let mut v = n.abs();
    let mut i = 0;
    loop {
        // Decimals as the value will be shown: 9,960 bytes is "10 KB", not "10.0 KB", and 999,950 "1.0 MB".
        let digits = if i == 0 || v >= 9.95 { 0 } else { 1 };
        let shown = if digits == 0 { v.round() } else { (v * 10.0).round() / 10.0 };
        if shown < 1000.0 || i == units.len() - 1 {
            return format!("{sign}{v:.digits$} {}", units[i]);
        }
        v /= 1000.0;
        i += 1;
    }
}

/// "1:05" or "1:02:03".
pub fn duration(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "0:00".into();
    }
    let s = seconds.max(0.0).floor() as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// Integer with thousands separators: "12,480".
pub fn integer(n: f64) -> String {
    if !n.is_finite() {
        return "0".into();
    }
    let v = n.round() as i64;
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if v < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// "92.5%", "100%" once complete: what is not complete never rounds up to it ("99.9%").
pub fn percent(part: f64, total: f64) -> String {
    if total.is_nan() || total <= 0.0 || !part.is_finite() {
        return "–".into();
    }
    if part >= total {
        return "100%".into();
    }
    format!("{:.1}%", (part / total * 100.0).min(99.9))
}

/// "just now", "5 min ago", "3 h ago", "2 d ago", then the date ("12 Oct 2026", UTC).
pub fn relative_time(epoch_seconds: u64, now_seconds: u64) -> String {
    let diff = now_seconds.saturating_sub(epoch_seconds);
    match diff {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", diff / 60),
        3600..86400 => format!("{} h ago", diff / 3600),
        86400..604800 => format!("{} d ago", diff / 86400),
        _ => {
            let (y, m, d) = civil_from_days((epoch_seconds / 86400) as i64);
            let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            format!("{d} {} {y}", months[(m - 1) as usize])
        }
    }
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Last component of a path.
pub fn folder_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_into_the_next_unit() {
        assert_eq!(bytes(8_212_830.0), "8.2 MB");
        assert_eq!(bytes(9_960.0), "10 KB");
        assert_eq!(bytes(9_940.0), "9.9 KB");
        assert_eq!(bytes(999_950.0), "1.0 MB");
        assert_eq!(bytes(999_400.0), "999 KB");
        assert_eq!(bytes(999.6), "1.0 KB");
        assert_eq!(bytes(-5_000.0), "-5.0 KB");
        assert_eq!(bytes(0.0), "0 B");
        assert_eq!(bytes(3.2e15), "3200 TB");
        assert_eq!(bytes(f64::NAN), "–");
    }

    #[test]
    fn percent_is_complete_only_when_it_is() {
        assert_eq!(percent(99.96, 100.0), "99.9%");
        assert_eq!(percent(100.0, 100.0), "100%");
        assert_eq!(percent(0.0, 100.0), "0.0%");
        assert_eq!(percent(1.0, 0.0), "–");
        assert_eq!(percent(f64::NAN, 1.0), "–");
    }

    #[test]
    fn formats() {
        assert_eq!(bytes(82_112_830.0), "82 MB");
        assert_eq!(bytes(1_500.0), "1.5 KB");
        assert_eq!(bytes(999.0), "999 B");
        assert_eq!(duration(65.4), "1:05");
        assert_eq!(duration(3723.0), "1:02:03");
        assert_eq!(integer(12480.0), "12,480");
        assert_eq!(integer(1_234_567.0), "1,234,567");
        assert_eq!(percent(37.0, 40.0), "92.5%");
        assert_eq!(percent(40.0, 40.0), "100%");
        assert_eq!(relative_time(1_000, 1_030), "just now");
        assert_eq!(relative_time(0, 7_200), "2 h ago");
        // 2026-10-12 00:00 UTC
        assert_eq!(relative_time(1_791_763_200, 1_791_763_200 + 30 * 86400), "12 Oct 2026");
    }
}
