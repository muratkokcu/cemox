//! Takvim aritmetiği ve tr-TR biçimlendirme.
//!
//! Türkiye 2016'dan beri kalıcı UTC+3 olduğu için `BOOKING_RULES.utc_offset_minutes`
//! sabit ofseti yeterlidir; ayrı bir saat dilimi veritabanına ihtiyaç yoktur.

use crate::config::BOOKING_RULES;

pub const DAY_MS: i64 = 86_400_000;

const MONTHS_LONG: [&str; 12] = [
    "Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim",
    "Kasım", "Aralık",
];
/// Pazar'dan başlar; `weekday_index` 0 = Pazar.
const WEEKDAYS: [&str; 7] = [
    "Pazar",
    "Pazartesi",
    "Salı",
    "Çarşamba",
    "Perşembe",
    "Cuma",
    "Cumartesi",
];

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

/// Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms". `m` 1-tabanlı.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// `Date.UTC(y, month, day)` karşılığı: `month` 0-tabanlıdır ve `month`/`day` taşabilir.
pub fn utc_ms(year: i64, month0: i64, day: i64) -> i64 {
    let total_months = year * 12 + month0;
    let normalized_year = total_months.div_euclid(12);
    let normalized_month = total_months.rem_euclid(12) + 1;
    (days_from_civil(normalized_year, normalized_month, 1) + day - 1) * DAY_MS
}

pub fn utc_ms_hm(year: i64, month0: i64, day: i64, hour: i64, minute: i64) -> i64 {
    utc_ms(year, month0, day) + hour * 3_600_000 + minute * 60_000
}

#[derive(Clone, Copy, Debug)]
pub struct Civil {
    pub year: i64,
    /// 1-tabanlı.
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
    /// 0 = Pazar.
    pub weekday: u32,
}

/// Verilen epoch milisaniyesini UTC takvim alanlarına ayırır.
pub fn civil_from_ms(ms: i64) -> Civil {
    let days = ms.div_euclid(DAY_MS);
    let rest = ms.rem_euclid(DAY_MS);
    let (year, month, day) = civil_from_days(days);
    Civil {
        year,
        month,
        day,
        hour: (rest / 3_600_000) as u32,
        minute: (rest / 60_000 % 60) as u32,
        second: (rest / 1_000 % 60) as u32,
        millis: (rest % 1_000) as u32,
        weekday: (days + 4).rem_euclid(7) as u32,
    }
}

/// Yerel (Istanbul) takvim alanları.
pub fn local_civil(ms: i64) -> Civil {
    civil_from_ms(ms + BOOKING_RULES.offset_ms())
}

/// `new Date(ms).toISOString().slice(0, 10)`
pub fn iso_date(ms: i64) -> String {
    let civil = civil_from_ms(ms);
    format!("{:04}-{:02}-{:02}", civil.year, civil.month, civil.day)
}

/// `new Date(ms).toISOString()`
pub fn to_iso_string(ms: i64) -> String {
    let c = civil_from_ms(ms);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        c.year, c.month, c.day, c.hour, c.minute, c.second, c.millis
    )
}

/// `Intl.DateTimeFormat('tr-TR', { hour: '2-digit', minute: '2-digit', timeZone: 'Europe/Istanbul' })`
pub fn format_time(ms: i64) -> String {
    let c = local_civil(ms);
    format!("{:02}:{:02}", c.hour, c.minute)
}

/// `Intl.DateTimeFormat('tr-TR', { weekday: 'long', day: 'numeric', month: 'long', timeZone: 'UTC' })`
/// → "3 Eylül Perşembe". Girdi, yerel günün UTC gece yarısını temsil eden zaman damgasıdır.
pub fn format_day(local_midnight_utc_ms: i64) -> String {
    let c = civil_from_ms(local_midnight_utc_ms);
    format!(
        "{} {} {}",
        c.day,
        MONTHS_LONG[(c.month - 1) as usize],
        WEEKDAYS[c.weekday as usize]
    )
}

/// ISO 8601 / RFC 3339 ayrıştırıcı. `new Date(string).getTime()` yerine geçer.
/// Desteklenen biçimler: `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS[.fff]][Z|±HH:MM|±HHMM]`.
pub fn parse_timestamp(value: &str) -> Option<i64> {
    let text = value.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return None;
    }

    let year: i64 = text.get(0..4)?.parse().ok()?;
    if bytes[4] != b'-' {
        return None;
    }
    let month: i64 = text.get(5..7)?.parse().ok()?;
    if bytes[7] != b'-' {
        return None;
    }
    let day: i64 = text.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let date_ms = utc_ms(year, month - 1, day);
    if bytes.len() == 10 {
        return Some(date_ms);
    }
    if bytes[10] != b'T' && bytes[10] != b't' && bytes[10] != b' ' {
        return None;
    }

    let rest = text.get(11..)?;
    // Saat dilimi son ekini ayır.
    let (clock, offset_minutes) = split_offset(rest)?;

    let mut parts = clock.split(':');
    let hour: i64 = parts.next()?.parse().ok()?;
    let minute: i64 = parts.next()?.parse().ok()?;
    let (second, millis) = match parts.next() {
        None => (0, 0),
        Some(seconds_text) => {
            let (whole, fraction) = match seconds_text.split_once('.') {
                Some((whole, fraction)) => (whole, fraction),
                None => (seconds_text, ""),
            };
            let second: i64 = whole.parse().ok()?;
            // Kesirli saniyeyi milisaniyeye normalize et.
            let millis: i64 = if fraction.is_empty() {
                0
            } else {
                let mut digits: String = fraction.chars().take(3).collect();
                while digits.len() < 3 {
                    digits.push('0');
                }
                digits.parse().ok()?
            };
            (second, millis)
        }
    };
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    Some(
        date_ms + hour * 3_600_000 + minute * 60_000 + second * 1_000 + millis
            - offset_minutes * 60_000,
    )
}

/// Saat kısmını saat dilimi ekinden ayırır; dakika cinsinden ofset döndürür.
fn split_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(clock) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
        return Some((clock, 0));
    }
    // '+' veya '-' işaretini saatten sonra ara (tarih kısmı zaten ayrıldı).
    if let Some(index) = rest.rfind(['+', '-']) {
        let (clock, offset_text) = rest.split_at(index);
        let sign = if offset_text.starts_with('-') { -1 } else { 1 };
        let digits = &offset_text[1..];
        let (hours, minutes) = match digits.split_once(':') {
            Some((hours, minutes)) => (hours, minutes),
            None if digits.len() == 4 => (&digits[0..2], &digits[2..4]),
            None if digits.len() == 2 => (digits, "0"),
            None => return None,
        };
        let hours: i64 = hours.parse().ok()?;
        let minutes: i64 = minutes.parse().ok()?;
        return Some((clock, sign * (hours * 60 + minutes)));
    }
    // Saat dilimi yoksa JS bunu yerel saat kabul eder; sunucu tarafında UTC varsayıyoruz.
    Some((rest, 0))
}

/// `Intl.DateTimeFormat('tr-TR', { dateStyle: 'long', timeStyle: 'short', timeZone: 'Europe/Istanbul' })`
/// → "3 Eylül 2026 10:00". E-posta şablonlarında kullanılır.
pub fn format_date_time_long(ms: i64) -> String {
    let c = local_civil(ms);
    format!(
        "{} {} {} {:02}:{:02}",
        c.day,
        MONTHS_LONG[(c.month - 1) as usize],
        c.year,
        c.hour,
        c.minute
    )
}
