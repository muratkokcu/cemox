//! Takvim aritmetiği ve tr-TR biçimlendirme testleri.
//! Beklenen değerler Node'un `Date.UTC` / `Intl.DateTimeFormat` çıktılarından alınmıştır,
//! böylece Rust portunun Node sürümüyle aynı dizeleri ürettiği sabitlenir.

use cemox_server::time::{
    civil_from_ms, format_date_time_long, format_day, format_time, iso_date, parse_timestamp,
    to_iso_string, utc_ms, utc_ms_hm,
};

const SEP_3_2026: i64 = 1_788_393_600_000; // Date.UTC(2026, 8, 3)
const SEP_3_2026_0700Z: i64 = 1_788_418_800_000; // Date.UTC(2026, 8, 3, 7, 0) = Istanbul 10:00

#[test]
fn utc_ms_matches_date_utc() {
    assert_eq!(utc_ms(2026, 8, 3), SEP_3_2026);
    assert_eq!(utc_ms_hm(2026, 8, 3, 7, 0), SEP_3_2026_0700Z);
    // Date.UTC gibi ay ve gün taşması normalize edilir: 2026/12/32 -> 2027-02-01
    assert_eq!(utc_ms(2026, 12, 32), 1_801_440_000_000);
    assert_eq!(iso_date(utc_ms(2026, 12, 32)), "2027-02-01");
    // 1970 öncesi negatif zaman damgaları
    assert_eq!(utc_ms(1969, 0, 1), -31_536_000_000);
}

#[test]
fn civil_round_trips() {
    let civil = civil_from_ms(SEP_3_2026_0700Z);
    assert_eq!((civil.year, civil.month, civil.day), (2026, 9, 3));
    assert_eq!((civil.hour, civil.minute), (7, 0));
    assert_eq!(civil.weekday, 4, "3 Eylül 2026 Perşembe");
    assert_eq!(
        utc_ms_hm(civil.year, civil.month as i64 - 1, civil.day as i64, 7, 0),
        SEP_3_2026_0700Z
    );
}

#[test]
fn turkish_formats_match_intl() {
    // Intl.DateTimeFormat('tr-TR', { weekday:'long', day:'numeric', month:'long', timeZone:'UTC' })
    assert_eq!(format_day(SEP_3_2026), "3 Eylül Perşembe");
    // { hour:'2-digit', minute:'2-digit', timeZone:'Europe/Istanbul' } — UTC+3 kayması dahil
    assert_eq!(format_time(SEP_3_2026_0700Z), "10:00");
    // { dateStyle:'long', timeStyle:'short', timeZone:'Europe/Istanbul' }
    assert_eq!(
        format_date_time_long(SEP_3_2026_0700Z),
        "3 Eylül 2026 10:00"
    );
    // Tek haneli saatler sıfırla doldurulur.
    assert_eq!(format_time(utc_ms_hm(2026, 8, 3, 6, 5)), "09:05");
}

#[test]
fn iso_serialisation_matches_to_iso_string() {
    assert_eq!(to_iso_string(SEP_3_2026_0700Z), "2026-09-03T07:00:00.000Z");
    assert_eq!(iso_date(SEP_3_2026_0700Z), "2026-09-03");
}

#[test]
fn parse_timestamp_accepts_the_formats_the_client_sends() {
    // Ön yüz her zaman `new Date(x).toISOString()` gönderir.
    assert_eq!(
        parse_timestamp("2026-09-03T07:00:00.000Z"),
        Some(SEP_3_2026_0700Z)
    );
    assert_eq!(
        parse_timestamp("2026-09-03T07:00:00Z"),
        Some(SEP_3_2026_0700Z)
    );
    // Ofsetli biçimler (toTimestamp `+03:00` üretir)
    assert_eq!(
        parse_timestamp("2026-09-03T10:00:00+03:00"),
        Some(SEP_3_2026_0700Z)
    );
    assert_eq!(
        parse_timestamp("2026-09-03T10:00:00+0300"),
        Some(SEP_3_2026_0700Z)
    );
    assert_eq!(
        parse_timestamp("2026-09-03T04:00:00-03:00"),
        Some(SEP_3_2026_0700Z)
    );
    // Yalnızca tarih
    assert_eq!(parse_timestamp("2026-09-03"), Some(SEP_3_2026));
    // Geçersiz girdiler
    assert_eq!(parse_timestamp(""), None);
    assert_eq!(parse_timestamp("yarın"), None);
    assert_eq!(parse_timestamp("2026-13-03T00:00:00Z"), None);
    assert_eq!(parse_timestamp("2026-09-03T25:00:00Z"), None);
}

#[test]
fn iso_round_trip_is_stable() {
    for offset in [0i64, 1, 86_400_000, 1_788_418_800_123, -31_536_000_000] {
        let text = to_iso_string(offset);
        assert_eq!(parse_timestamp(&text), Some(offset), "tur atmadı: {text}");
    }
}
