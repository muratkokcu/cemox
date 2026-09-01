//! `test/app.test.js` içindeki `createTestServer` yardımcısının karşılığı.

use std::collections::HashMap;
use std::net::SocketAddr;

use cemox_server::app::{AppState, build_router};
use cemox_server::config::{BOOKING_RULES, load_config};
use cemox_server::db::Db;
use cemox_server::email::EmailService;
use cemox_server::time::{civil_from_ms, now_ms, utc_ms, utc_ms_hm};
use serde_json::Value;

pub struct TestServer {
    pub base_url: String,
    /// Testlerin HTTP'yi atlayarak veri hazırlaması için.
    pub db: Db,
    pub client: reqwest::Client,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub fn test_env() -> HashMap<String, String> {
    [
        ("NODE_ENV", "test"),
        ("APP_ORIGIN", "http://127.0.0.1"),
        ("DATABASE_PATH", ":memory:"),
        ("ADMIN_EMAIL", "admin@example.com"),
        ("ADMIN_PASSWORD", "test-admin-password"),
        (
            "SESSION_SECRET",
            "test-session-secret-at-least-32-characters",
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

/// İki branş için önümüzdeki hafta içi günlere 10:00 / 10:30 / 11:00 saatlerini açar.
fn seed_availability(db: &Db) {
    let now = now_ms();
    let local_now = civil_from_ms(now + BOOKING_RULES.offset_ms());
    for offset in 2..=10 {
        let day_ms = utc_ms(
            local_now.year,
            local_now.month as i64 - 1,
            local_now.day as i64 + offset,
        );
        let day = civil_from_ms(day_ms);
        // Hafta sonlarını atla.
        if day.weekday == 0 || day.weekday == 6 {
            continue;
        }
        for service_id in ["medical-fitness", "kisisel-antrenman"] {
            for minute in [600i64, 630, 660] {
                let start_at = utc_ms_hm(
                    day.year,
                    day.month as i64 - 1,
                    day.day as i64,
                    minute / 60,
                    minute % 60,
                ) - BOOKING_RULES.offset_ms();
                db.set_availability_slot(
                    service_id,
                    start_at,
                    start_at + BOOKING_RULES.slot_ms(),
                    true,
                    now,
                )
                .expect("slot açılamadı");
            }
        }
    }
}

pub async fn start() -> TestServer {
    let config = load_config(&test_env()).expect("yapılandırma yüklenemedi");
    let db = Db::new(":memory:").expect("veritabanı açılamadı");
    seed_availability(&db);

    let state = AppState::new(config, db.clone(), EmailService::disabled());
    let router = build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("port açılamadı");
    let port = listener.local_addr().expect("adres okunamadı").port();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    TestServer {
        base_url: format!("http://127.0.0.1:{port}"),
        db,
        client: reqwest::Client::new(),
        server,
    }
}

pub struct ApiResponse {
    pub status: u16,
    pub body: Value,
    /// JSON olmayan yanıtlar (CSV, iCal) için ham gövde.
    pub text: String,
    pub set_cookie: Option<String>,
}

impl TestServer {
    pub async fn request(
        &self,
        method: reqwest::Method,
        route: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> ApiResponse {
        let mut request = self
            .client
            .request(method, format!("{}{route}", self.base_url))
            .header("Content-Type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            request = request.body(body.to_string());
        }

        let response = request.send().await.expect("istek gönderilemedi");
        let status = response.status().as_u16();
        let set_cookie = response
            .headers()
            .get("set-cookie")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or("").to_string());
        let text = response.text().await.unwrap_or_default();
        let body = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::Null)
        };
        ApiResponse {
            status,
            body,
            text,
            set_cookie,
        }
    }

    pub async fn get(&self, route: &str) -> ApiResponse {
        self.request(reqwest::Method::GET, route, &[], None).await
    }

    pub async fn post(&self, route: &str, body: Value) -> ApiResponse {
        self.request(reqwest::Method::POST, route, &[], Some(body))
            .await
    }

    /// Yönetici olarak giriş yapar; dönen çerez ve CSRF jetonu yazma isteklerinde kullanılır.
    pub async fn login(&self) -> (String, String) {
        let response = self
            .post(
                "/api/admin/login",
                serde_json::json!({ "password": "test-admin-password" }),
            )
            .await;
        assert_eq!(response.status, 200, "giriş başarısız: {:?}", response.body);
        (
            response.set_cookie.expect("oturum çerezi yok"),
            response.body["csrfToken"]
                .as_str()
                .expect("csrf yok")
                .to_string(),
        )
    }
}
