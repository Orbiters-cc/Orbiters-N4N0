//! Number and time formatting shared by the interfaces.

use orbiters_i18n::{language, mark, tr, Language};

/// A decimal number as this language writes it: a comma in French, Spanish and Portuguese.
fn decimal(number: String) -> String {
    match language() {
        Language::Fr | Language::Es | Language::PtBr => number.replace('.', ","),
        Language::En | Language::Ja => number,
    }
}

/// What groups thousands in this language. French's is a space that does not break.
fn thousands() -> char {
    match language() {
        Language::Fr => '\u{a0}',
        Language::Es | Language::PtBr => '.',
        Language::En | Language::Ja => ',',
    }
}

/// A percentage as this language writes it: with a space that does not break in French and Spanish.
fn with_percent(number: String) -> String {
    match language() {
        Language::Fr | Language::Es => format!("{number}\u{a0}%"),
        Language::En | Language::Ja | Language::PtBr => format!("{number}%"),
    }
}

/// Bytes in SI units, one decimal under ten: "8.2 MB", "82 MB". French counts in octets: "8,2 Mo".
pub fn bytes(n: f64) -> String {
    if !n.is_finite() {
        return "–".into();
    }
    let units = if language() == Language::Fr { ["o", "Ko", "Mo", "Go", "To"] } else { ["B", "KB", "MB", "GB", "TB"] };
    let gap = if language() == Language::Fr { '\u{a0}' } else { ' ' };
    let sign = if n < 0.0 { "-" } else { "" };
    let mut v = n.abs();
    let mut i = 0;
    loop {
        // Decimals as the value will be shown: 9,960 bytes is "10 KB", not "10.0 KB", and 999,950 "1.0 MB".
        let digits = if i == 0 || v >= 9.95 { 0 } else { 1 };
        let shown = if digits == 0 { v.round() } else { (v * 10.0).round() / 10.0 };
        if shown < 1000.0 || i == units.len() - 1 {
            return format!("{sign}{}{gap}{}", decimal(format!("{v:.digits$}")), units[i]);
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

/// Integer with thousands separators: "12,480", as this language writes them ("12.480", and French's "12 480").
pub fn integer(n: f64) -> String {
    if !n.is_finite() {
        return "0".into();
    }
    let v = n.round() as i64;
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(thousands());
        }
        out.push(ch);
    }
    if v < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// "92.5%", "100%" once complete: what is not complete never rounds up to it ("99.9%"). As this language writes them: "92,5 %".
pub fn percent(part: f64, total: f64) -> String {
    if total.is_nan() || total <= 0.0 || !part.is_finite() {
        return "–".into();
    }
    if part >= total {
        return with_percent("100".into());
    }
    with_percent(decimal(format!("{:.1}", (part / total * 100.0).min(99.9))))
}

/// "just now", "5 min ago", "3 h ago", "2 d ago", then the date ("12 Oct 2026", UTC).
pub fn relative_time(epoch_seconds: u64, now_seconds: u64) -> String {
    let diff = now_seconds.saturating_sub(epoch_seconds);
    match diff {
        0..60 => tr("just now").into(),
        60..3600 => tr!("{minutes} min ago", minutes = diff / 60),
        3600..86400 => tr!("{hours} h ago", hours = diff / 3600),
        86400..604800 => tr!("{days} d ago", days = diff / 86400),
        _ => {
            let (y, m, d) = civil_from_days((epoch_seconds / 86400) as i64);
            let months = [mark("Jan"), mark("Feb"), mark("Mar"), mark("Apr"), mark("May"), mark("Jun"), mark("Jul"), mark("Aug"), mark("Sep"), mark("Oct"), mark("Nov"), mark("Dec")];
            // The whole date is one text: the order of day, month and year is each language's own.
            tr!("{day} {month} {year}", day = d, month = tr(months[(m - 1) as usize]), year = y)
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
    #[test]
    fn numbers_are_written_as_each_language_writes_them() {
        use orbiters_i18n::in_language;
        assert_eq!((bytes(8_200_000.0), integer(12_480.0), percent(925.0, 1000.0)), ("8.2 MB".to_string(), "12,480".to_string(), "92.5%".to_string()));
        in_language(Language::Ja, || assert_eq!((bytes(8_200_000.0), integer(12_480.0), percent(925.0, 1000.0)), ("8.2 MB".to_string(), "12,480".to_string(), "92.5%".to_string())));
        in_language(Language::Fr, || assert_eq!((bytes(8_200_000.0), integer(12_480.0), percent(925.0, 1000.0)), ("8,2\u{a0}Mo".to_string(), "12\u{a0}480".to_string(), "92,5\u{a0}%".to_string())));
        in_language(Language::Es, || assert_eq!((bytes(8_200_000.0), integer(12_480.0), percent(925.0, 1000.0)), ("8,2 MB".to_string(), "12.480".to_string(), "92,5\u{a0}%".to_string())));
        in_language(Language::PtBr, || assert_eq!((bytes(8_200_000.0), integer(12_480.0), percent(1000.0, 1000.0)), ("8,2 MB".to_string(), "12.480".to_string(), "100%".to_string())));
    }

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

    /// The same moments in the other languages: the order of a date and the words of a time are each language's own.
    #[test]
    fn times_and_dates_read_in_the_language_now() {
        use orbiters_i18n::{in_language, Language};
        let old = (1_791_763_200, 1_791_763_200 + 30 * 86400);
        in_language(Language::Ja, || {
            assert_eq!(relative_time(0, 7_200), "2時間前");
            assert_eq!(relative_time(old.0, old.1), "2026年10月12日");
        });
        in_language(Language::Fr, || {
            assert_eq!(relative_time(0, 300), "il y a 5 min");
            assert_eq!(relative_time(old.0, old.1), "12 oct. 2026");
        });
        in_language(Language::Es, || assert_eq!(relative_time(0, 86_400 * 2), "hace 2 d"));
        in_language(Language::PtBr, || assert_eq!(relative_time(old.0, old.1), "12 out 2026"));
    }
}
