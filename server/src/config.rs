//! `src/config.js` karşılığı: hizmet listesi, randevu kuralları ve ortam yapılandırması.

use std::collections::HashMap;
use std::path::PathBuf;

/// Hizmetler tanım sırasını korur; arayüz branş listesini bu sırada gösterir.
pub const SERVICES: [(&str, &str); 6] = [
    ("medical-fitness", "Medical Fitness"),
    ("kisisel-antrenman", "Kişisel Antrenman"),
    ("fonksiyonel-antrenman", "Fonksiyonel Antrenman"),
    ("performans-gelistirme", "Performans Geliştirme"),
    ("kurek-antrenorlugu", "Kürek Antrenörlüğü"),
    ("korektif-egzersiz", "Korektif Egzersiz Yaklaşımı"),
];

pub fn service_name(id: &str) -> Option<&'static str> {
    SERVICES
        .iter()
        .find(|(service_id, _)| *service_id == id)
        .map(|(_, name)| *name)
}

pub struct BookingRules {
    pub timezone: &'static str,
    pub utc_offset_minutes: i64,
    /// Bir antrenman seansının uzunluğu.
    pub slot_minutes: i64,
    /// Seanslar arası zorunlu boşluk. Sıfır: seanslar uç uca verilebilir.
    pub buffer_minutes: i64,
    /// Panelde seans ızgarasının adımı; çalışma penceresinin başından sayılır,
    /// gece yarısından değil. Antrenör 08:00 açtığında seanslar 08:00, 09:30,
    /// 11:00 diye gider.
    pub slot_step_minutes: i64,
    /// Bir zaman damgasının kabul edilebilir en küçük çözünürlüğü. Seans adımı
    /// pencereye göreli olduğu için hizalama bundan ayrı tutulur; aksi halde
    /// 90'a bölünmeyen bir çalışma başlangıcı (08:00 gibi) seçilemezdi.
    pub grid_minutes: i64,
    pub minimum_notice_hours: i64,
    pub horizon_days: i64,
    pub hold_hours: i64,
}

pub const BOOKING_RULES: BookingRules = BookingRules {
    timezone: "Europe/Istanbul",
    utc_offset_minutes: 180,
    slot_minutes: 90,
    buffer_minutes: 0,
    slot_step_minutes: 90,
    grid_minutes: 30,
    minimum_notice_hours: 24,
    horizon_days: 30,
    hold_hours: 24,
};

impl BookingRules {
    pub const fn buffer_ms(&self) -> i64 {
        self.buffer_minutes * 60_000
    }
    pub const fn slot_ms(&self) -> i64 {
        self.slot_minutes * 60_000
    }
    pub const fn offset_ms(&self) -> i64 {
        self.utc_offset_minutes * 60_000
    }
}

#[derive(Clone, Debug)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub secure: bool,
    pub user: String,
    pub pass: String,
    pub from: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub env: String,
    pub production: bool,
    pub port: u16,
    pub app_origin: String,
    pub database_path: String,
    pub admin_email: String,
    pub admin_password: String,
    pub session_secret: String,
    pub smtp: SmtpConfig,
}

fn get(env: &HashMap<String, String>, key: &str) -> Option<String> {
    // JS `env.X || fallback` boş string'i de yok sayar.
    env.get(key).filter(|value| !value.is_empty()).cloned()
}

/// `loadConfig` karşılığı. Üretimde şifre/secret zorunlu, aksi halde geliştirme varsayılanları kullanılır.
pub fn load_config(env: &HashMap<String, String>) -> Result<Config, String> {
    let node_env = get(env, "NODE_ENV");
    let production = node_env.as_deref() == Some("production");

    let admin_password = get(env, "ADMIN_PASSWORD").unwrap_or_else(|| {
        if production {
            String::new()
        } else {
            "development-password-change-me".into()
        }
    });
    let session_secret = get(env, "SESSION_SECRET").unwrap_or_else(|| {
        if production {
            String::new()
        } else {
            "development-session-secret-change-me-now".into()
        }
    });

    if admin_password.chars().count() < 12 {
        return Err("ADMIN_PASSWORD en az 12 karakter olmalıdır.".into());
    }
    if session_secret.chars().count() < 32 {
        return Err("SESSION_SECRET en az 32 karakter olmalıdır.".into());
    }

    let app_origin = get(env, "APP_ORIGIN")
        .unwrap_or_else(|| {
            if production {
                String::new()
            } else {
                "http://localhost:5100".into()
            }
        })
        .trim_end_matches('/')
        .to_string();

    let raw_database_path =
        get(env, "DATABASE_PATH").unwrap_or_else(|| "./data/appointments.sqlite".into());
    let database_path = if raw_database_path == ":memory:" {
        raw_database_path
    } else {
        absolutize(&raw_database_path)
    };

    Ok(Config {
        env: node_env.unwrap_or_else(|| "development".into()),
        production,
        port: get(env, "PORT")
            .and_then(|value| value.parse().ok())
            .unwrap_or(4100),
        app_origin,
        database_path,
        admin_email: get(env, "ADMIN_EMAIL").unwrap_or_else(|| "cemavat@gmail.com".into()),
        admin_password,
        session_secret,
        smtp: SmtpConfig {
            host: get(env, "SMTP_HOST").unwrap_or_default(),
            port: get(env, "SMTP_PORT")
                .and_then(|value| value.parse().ok())
                .unwrap_or(587),
            secure: get(env, "SMTP_SECURE").as_deref() == Some("true"),
            user: get(env, "SMTP_USER").unwrap_or_default(),
            pass: get(env, "SMTP_PASS").unwrap_or_default(),
            from: get(env, "SMTP_FROM")
                .unwrap_or_else(|| "Cem Avat Randevu <randevu@localhost>".into()),
        },
    })
}

fn absolutize(value: &str) -> String {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Uygulama kökü: statik dosyalar (`assets/`, `galery/`, `dist/`) buradan sunulur.
/// `server/` alt dizininden çalıştırıldığında `APP_ROOT` ile geçersiz kılınabilir.
pub fn app_root() -> PathBuf {
    if let Ok(root) = std::env::var("APP_ROOT")
        && !root.is_empty()
    {
        return PathBuf::from(root);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
