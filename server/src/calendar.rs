//! Google Takvim entegrasyonu.
//!
//! Kullanıcı OAuth'u yerine **servis hesabı** kullanılır: antrenör kendi
//! takvimini servis hesabının e-posta adresiyle paylaşır ("Etkinliklerde
//! değişiklik yap" izniyle). Böylece onay ekranı, kullanıcı jetonu ve Google'ın
//! doğrulama süreci devreden çıkar. Erişim jetonu servis hesabının kendi
//! anahtarından üretildiği için süresiz yenilenebilir; OAuth'un "Test
//! durumunda yenileme jetonu 7 günde düşer" tuzağı burada yoktur.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::BOOKING_RULES;
use crate::db::Appointment;
use crate::error::AppError;
use crate::time::{civil_from_ms, now_ms};

const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_API_BASE: &str = "https://www.googleapis.com/calendar/v3";
/// Jeton bir saat geçerli; süresi dolmadan bir dakika önce yenilenir.
const TOKEN_SKEW_SECONDS: i64 = 60;

/// Servis hesabı anahtar dosyasının ihtiyaç duyulan alanları.
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceAccount {
    pub client_email: String,
    pub private_key: String,
    #[serde(default)]
    pub token_uri: Option<String>,
}

#[derive(Debug, Serialize)]
struct JwtClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    exp: i64,
    iat: i64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
}

struct CachedToken {
    value: String,
    expires_at: i64,
}

pub struct GoogleCalendar {
    http: reqwest::Client,
    account: ServiceAccount,
    token_uri: String,
    api_base: String,
    token: Mutex<Option<CachedToken>>,
}

impl GoogleCalendar {
    /// Ortam değişkenlerinden kurar. `GOOGLE_SERVICE_ACCOUNT` ya doğrudan JSON
    /// ya da JSON dosyasının yolu olabilir. Tanımlı değilse `None` döner ve
    /// takvim senkronu kapalı kalır.
    pub fn from_env(env: &HashMap<String, String>) -> Option<Self> {
        let raw = env
            .get("GOOGLE_SERVICE_ACCOUNT")
            .filter(|value| !value.is_empty())?;
        let json = if raw.trim_start().starts_with('{') {
            raw.clone()
        } else {
            match std::fs::read_to_string(raw) {
                Ok(contents) => contents,
                Err(error) => {
                    tracing::error!(%error, "servis hesabı dosyası okunamadı");
                    return None;
                }
            }
        };
        match serde_json::from_str::<ServiceAccount>(&json) {
            Ok(account) => Some(Self::new(
                account,
                env.get("GOOGLE_TOKEN_URI").cloned(),
                env.get("GOOGLE_CALENDAR_API").cloned(),
            )),
            Err(error) => {
                tracing::error!(%error, "servis hesabı anahtarı çözümlenemedi");
                None
            }
        }
    }

    pub fn new(
        account: ServiceAccount,
        token_uri: Option<String>,
        api_base: Option<String>,
    ) -> Self {
        let token_uri = token_uri
            .or_else(|| account.token_uri.clone())
            .unwrap_or_else(|| DEFAULT_TOKEN_URI.to_string());
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            account,
            token_uri,
            api_base: api_base.unwrap_or_else(|| DEFAULT_API_BASE.to_string()),
            token: Mutex::new(None),
        }
    }

    pub fn client_email(&self) -> &str {
        &self.account.client_email
    }

    /// Test desteği: jeton önbelleğini doğrudan doldurur.
    ///
    /// JWT imzalamak gerçek bir RSA anahtarı ister; onu depoya koymamak için
    /// testler bu yolu kullanır. Böylece istek gövdeleri, hata yolları ve
    /// uzlaştırma imzalama adımından bağımsız olarak sınanabilir.
    #[doc(hidden)]
    pub fn seed_token(&self, value: &str, valid_for_seconds: i64) {
        if let Ok(mut cache) = self.token.lock() {
            *cache = Some(CachedToken {
                value: value.to_string(),
                expires_at: now_ms() / 1000 + valid_for_seconds,
            });
        }
    }

    /// Önbellekteki jeton geçerliyse onu, değilse yenisini döndürür.
    async fn access_token(&self) -> Result<String, AppError> {
        let now = now_ms() / 1000;
        if let Ok(cache) = self.token.lock()
            && let Some(token) = cache.as_ref()
            && token.expires_at > now
        {
            return Ok(token.value.clone());
        }

        let claims = JwtClaims {
            iss: &self.account.client_email,
            scope: SCOPE,
            aud: &self.token_uri,
            exp: now + 3600,
            iat: now,
        };
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(self.account.private_key.as_bytes())
            .map_err(|error| {
                AppError::internal(format!("servis hesabı anahtarı geçersiz: {error}"))
            })?;
        let assertion = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &claims,
            &key,
        )
        .map_err(|error| AppError::internal(format!("jeton imzalanamadı: {error}")))?;

        let response = self
            .http
            .post(&self.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ])
            .send()
            .await
            .map_err(|error| AppError::internal(format!("jeton isteği başarısız: {error}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::internal(format!(
                "jeton alınamadı ({status}): {}",
                body.chars().take(200).collect::<String>()
            )));
        }

        let token: TokenResponse = response
            .json()
            .await
            .map_err(|error| AppError::internal(format!("jeton yanıtı çözümlenemedi: {error}")))?;

        if let Ok(mut cache) = self.token.lock() {
            *cache = Some(CachedToken {
                value: token.access_token.clone(),
                expires_at: now + token.expires_in - TOKEN_SKEW_SECONDS,
            });
        }
        Ok(token.access_token)
    }

    /// Etkinliği oluşturur ve Google'ın verdiği kimliği döndürür.
    pub async fn create_event(
        &self,
        calendar_id: &str,
        appointment: &Appointment,
    ) -> Result<String, AppError> {
        let url = format!(
            "{}/calendars/{}/events",
            self.api_base,
            encode_path(calendar_id)
        );
        let response = self
            .send(reqwest::Method::POST, &url, Some(event_body(appointment)))
            .await?;
        response["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AppError::internal("etkinlik kimliği alınamadı"))
    }

    pub async fn update_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        appointment: &Appointment,
    ) -> Result<(), AppError> {
        let url = format!(
            "{}/calendars/{}/events/{}",
            self.api_base,
            encode_path(calendar_id),
            encode_path(event_id)
        );
        self.send(reqwest::Method::PATCH, &url, Some(event_body(appointment)))
            .await?;
        Ok(())
    }

    /// Etkinliği siler. Google 404/410 döndürürse etkinlik zaten yoktur;
    /// bu bir hata değil, istenen sonucun kendisidir.
    pub async fn delete_event(&self, calendar_id: &str, event_id: &str) -> Result<(), AppError> {
        let url = format!(
            "{}/calendars/{}/events/{}",
            self.api_base,
            encode_path(calendar_id),
            encode_path(event_id)
        );
        match self.send(reqwest::Method::DELETE, &url, None).await {
            Ok(_) => Ok(()),
            Err(error) if error.message.contains("(404") || error.message.contains("(410") => {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<Value, AppError> {
        let token = self.access_token().await?;
        let mut request = self.http.request(method, url).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| AppError::internal(format!("takvim isteği başarısız: {error}")))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(AppError::internal(format!(
                "takvim isteği reddedildi ({status}): {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }
}

/// Yerel saati açık ofsetle yazar; Google böylece belirsizlik yaşamaz.
fn local_rfc3339(timestamp: i64) -> String {
    let civil = civil_from_ms(timestamp + BOOKING_RULES.offset_ms());
    let minutes = BOOKING_RULES.utc_offset_minutes;
    let sign = if minutes < 0 { '-' } else { '+' };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
        civil.year,
        civil.month,
        civil.day,
        civil.hour,
        civil.minute,
        civil.second,
        sign,
        minutes.abs() / 60,
        minutes.abs() % 60
    )
}

fn event_body(appointment: &Appointment) -> Value {
    let mut description = format!("Telefon: {}", appointment.phone);
    if !appointment.email.is_empty() {
        description.push_str(&format!("\nE-posta: {}", appointment.email));
    }
    if !appointment.note.is_empty() {
        description.push_str(&format!("\nNot: {}", appointment.note));
    }

    json!({
        "summary": format!("{} · {}", appointment.name, appointment.service_name),
        "description": description,
        "start": {
            "dateTime": local_rfc3339(appointment.start_at),
            "timeZone": BOOKING_RULES.timezone
        },
        "end": {
            "dateTime": local_rfc3339(appointment.end_at),
            "timeZone": BOOKING_RULES.timezone
        }
        // Hatırlatıcılar takvimin kendi varsayılanına bırakılır ki antrenörün
        // telefonundaki ayarı geçerli olsun.
    })
}

/// Takvim ve etkinlik kimlikleri URL yolunda geçtiği için kaçırılır.
fn encode_path(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
