//! Randevu listesinin CSV ve iCal karşılıkları.

use crate::config::BOOKING_RULES;
use crate::db::Appointment;
use crate::time::{format_date_time_long, iso_date, local_civil, now_ms};

/// Excel Türkçe yerelinde ayırıcı olarak `;` bekler; virgül kullanılırsa tüm
/// satır tek hücreye düşer. BOM olmadan da Türkçe karakterler bozuk görünür.
const CSV_SEPARATOR: char = ';';
const UTF8_BOM: &str = "\u{feff}";

const CSV_HEADERS: [&str; 11] = [
    "Durum",
    "Tarih",
    "Saat",
    "Branş",
    "Ad soyad",
    "Telefon",
    "E-posta",
    "Danışan notu",
    "Yönetici notu",
    "Talep zamanı",
    "Karar zamanı",
];

fn status_label(status: &str) -> &'static str {
    match status {
        "PENDING" => "Onay bekliyor",
        "APPROVED" => "Onaylandı",
        "REJECTED" => "Reddedildi",
        "EXPIRED" => "Süresi doldu",
        "CANCELLED" => "İptal",
        "CONFLICT" => "Çakışma",
        _ => "Bilinmiyor",
    }
}

/// Alanı gerektiğinde tırnak içine alır ve içindeki tırnakları ikiler.
fn csv_field(value: &str) -> String {
    let needs_quotes = value.contains(CSV_SEPARATOR)
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r');
    if !needs_quotes {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn local_date(timestamp: i64) -> String {
    iso_date(timestamp + BOOKING_RULES.offset_ms())
}

fn local_time(timestamp: i64) -> String {
    let civil = local_civil(timestamp);
    format!("{:02}:{:02}", civil.hour, civil.minute)
}

pub fn csv_export(appointments: &[Appointment]) -> String {
    let mut out = String::from(UTF8_BOM);
    out.push_str(&CSV_HEADERS.join(&CSV_SEPARATOR.to_string()));
    out.push_str("\r\n");

    for appointment in appointments {
        let decision = appointment
            .decision_at
            .map(format_date_time_long)
            .unwrap_or_default();
        let row = [
            status_label(&appointment.status).to_string(),
            local_date(appointment.start_at),
            local_time(appointment.start_at),
            appointment.service_name.clone(),
            appointment.name.clone(),
            appointment.phone.clone(),
            appointment.email.clone(),
            appointment.note.clone(),
            appointment.admin_note.clone(),
            format_date_time_long(appointment.created_at),
            decision,
        ];
        let line: Vec<String> = row.iter().map(|value| csv_field(value)).collect();
        out.push_str(&line.join(&CSV_SEPARATOR.to_string()));
        out.push_str("\r\n");
    }
    out
}

/// RFC 5545 metin kaçırması.
fn ics_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
        .replace('\r', "")
}

/// RFC 5545 satırları 75 oktetten uzun olamaz. Türkçe harfler UTF-8'de iki bayt
/// tuttuğu için katlama bayt sayarak ama karakter ortasından bölmeden yapılır.
fn fold_line(line: &str, out: &mut String) {
    const LIMIT: usize = 74;
    let mut used = 0;
    for character in line.chars() {
        let width = character.len_utf8();
        if used + width > LIMIT {
            // Devam satırı bir boşlukla başlar ve o boşluk da sınıra sayılır.
            out.push_str("\r\n ");
            used = 1;
        }
        out.push(character);
        used += width;
    }
    out.push_str("\r\n");
}

fn ics_timestamp(timestamp: i64) -> String {
    let civil = crate::time::civil_from_ms(timestamp);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        civil.year, civil.month, civil.day, civil.hour, civil.minute, civil.second
    )
}

pub fn ics_export(appointments: &[Appointment]) -> String {
    let mut out = String::new();
    let stamp = ics_timestamp(now_ms());

    for line in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Cem Avat//Randevu//TR",
        "CALSCALE:GREGORIAN",
        "METHOD:PUBLISH",
    ] {
        fold_line(line, &mut out);
    }

    for appointment in appointments {
        // İptal ve ret kayıtları da takvimde iptal olarak görünür; yönetici
        // filtreyle neyi indireceğine kendisi karar verir.
        let status = match appointment.status.as_str() {
            "APPROVED" => "CONFIRMED",
            "PENDING" | "CONFLICT" => "TENTATIVE",
            _ => "CANCELLED",
        };
        let mut description = format!("Telefon: {}", appointment.phone);
        if !appointment.email.is_empty() {
            description.push_str(&format!("\nE-posta: {}", appointment.email));
        }
        if !appointment.note.is_empty() {
            description.push_str(&format!("\nNot: {}", appointment.note));
        }
        description.push_str(&format!("\nDurum: {}", status_label(&appointment.status)));

        for line in [
            "BEGIN:VEVENT".to_string(),
            format!("UID:{}@cemavat", appointment.id),
            format!("DTSTAMP:{stamp}"),
            format!("DTSTART:{}", ics_timestamp(appointment.start_at)),
            format!("DTEND:{}", ics_timestamp(appointment.end_at)),
            format!(
                "SUMMARY:{}",
                ics_text(&format!(
                    "{} · {}",
                    appointment.name, appointment.service_name
                ))
            ),
            format!("DESCRIPTION:{}", ics_text(&description)),
            format!("STATUS:{status}"),
            "END:VEVENT".to_string(),
        ] {
            fold_line(&line, &mut out);
        }
    }

    fold_line("END:VCALENDAR", &mut out);
    out
}
