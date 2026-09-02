//! Takvim senkronu, Google'ı taklit eden yerel bir sunucuya karşı sınanır.
//! Gerçek Google çağrısı burada doğrulanamaz; doğrulanan şey bizim ürettiğimiz
//! istekler, hata yolları ve uzlaştırma davranışıdır.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use cemox_server::calendar::{GoogleCalendar, ServiceAccount};
use cemox_server::config::BOOKING_RULES;
use cemox_server::time::{civil_from_ms, now_ms, to_iso_string, utc_ms_hm};
use reqwest::Method;
use serde_json::{Value, json};

/// Sahte Google'ın aldığı istekler.
#[derive(Default)]
struct Recorded {
    calls: Vec<(String, String, Value)>,
    /// Bu sayı sıfırdan büyükken sunucu 500 döndürür.
    fail_next: usize,
    next_event_id: usize,
}

type Shared = Arc<Mutex<Recorded>>;

async fn fake_google(shared: Shared) -> String {
    async fn create(
        State(shared): State<Shared>,
        Path(calendar_id): Path<String>,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
        let mut state = shared.lock().unwrap();
        state.calls.push(("POST".into(), calendar_id, body));
        if state.fail_next > 0 {
            state.fail_next -= 1;
            return Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "patladı".into(),
            ));
        }
        state.next_event_id += 1;
        let id = format!("evt-{}", state.next_event_id);
        Ok(Json(json!({ "id": id })))
    }

    async fn modify(
        State(shared): State<Shared>,
        method: Method,
        Path((calendar_id, event_id)): Path<(String, String)>,
        body: Option<Json<Value>>,
    ) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
        let mut state = shared.lock().unwrap();
        state.calls.push((
            method.to_string(),
            format!("{calendar_id}/{event_id}"),
            body.map(|Json(value)| value).unwrap_or(Value::Null),
        ));
        if state.fail_next > 0 {
            state.fail_next -= 1;
            return Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "patladı".into(),
            ));
        }
        Ok(Json(json!({ "id": event_id })))
    }

    let app = Router::new()
        .route("/calendars/{calendar_id}/events", post(create))
        .route(
            "/calendars/{calendar_id}/events/{event_id}",
            axum::routing::patch(modify).delete(modify),
        )
        .with_state(shared);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

fn calendar_for(api_base: &str) -> Arc<GoogleCalendar> {
    let calendar = GoogleCalendar::new(
        ServiceAccount {
            client_email: "cemox@test.iam.gserviceaccount.com".into(),
            private_key: String::new(),
            token_uri: None,
        },
        None,
        Some(api_base.to_string()),
    );
    // İmzalama adımı atlanır; bkz. GoogleCalendar::seed_token.
    calendar.seed_token("test-token", 3600);
    Arc::new(calendar)
}

#[tokio::test]
async fn approved_appointments_reach_the_calendar() {
    let shared: Shared = Arc::default();
    let api_base = fake_google(shared.clone()).await;
    let server = common::start_with_calendar(Some(calendar_for(&api_base))).await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let read = [("Cookie", cookie.as_str())];

    // Bağlantı kurulmadan hiçbir şey gönderilmemeli.
    let status = server
        .request(Method::GET, "/api/admin/calendar", &read, None)
        .await;
    assert_eq!(status.body["configured"], true, "servis hesabı tanımlı");
    assert_eq!(status.body["enabled"], false, "başlangıçta kapalı");

    let now = now_ms();
    let local = civil_from_ms(now + BOOKING_RULES.offset_ms());
    let slot = |day: i64, hour: i64| -> i64 {
        utc_ms_hm(
            local.year,
            local.month as i64 - 1,
            local.day as i64 + day,
            hour,
            0,
        ) - BOOKING_RULES.offset_ms()
    };

    let created = server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(slot(3, 14)),
                "name": "Şule Öztürk", "phone": "+905551112233", "note": "Sırt ağrısı"
            })),
        )
        .await;
    assert_eq!(created.status, 201);
    let appointment_id = created.body["appointment"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        shared.lock().unwrap().calls.is_empty(),
        "senkron kapalıyken Google'a istek gitmemeli"
    );

    // Bağlantıyı aç: kapalıyken oluşan kayıtlar da uzlaştırılmalı.
    let enabled = server
        .request(
            Method::PUT,
            "/api/admin/calendar",
            &headers,
            Some(json!({ "calendarId": "antrenor@example.com", "enabled": true })),
        )
        .await;
    assert_eq!(enabled.status, 200, "{:?}", enabled.body);
    assert_eq!(enabled.body["synced"], 1, "bekleyen kayıt uzlaştırılmalı");

    {
        let state = shared.lock().unwrap();
        assert_eq!(state.calls.len(), 1);
        let (method, target, body) = &state.calls[0];
        assert_eq!(method, "POST");
        assert_eq!(target, "antrenor@example.com");
        assert_eq!(body["summary"], "Şule Öztürk · Medical Fitness");
        assert!(
            body["description"]
                .as_str()
                .unwrap()
                .contains("Sırt ağrısı"),
            "not açıklamaya girmeli: {body:?}"
        );
        // Saat yerel ofsetle yazılır ki Google belirsizlik yaşamasın.
        assert_eq!(body["start"]["dateTime"].as_str().unwrap().len(), 25);
        assert!(
            body["start"]["dateTime"]
                .as_str()
                .unwrap()
                .ends_with("+03:00")
        );
        assert_eq!(body["start"]["timeZone"], "Europe/Istanbul");
    }

    // Taşıma güncelleme göndermeli.
    server
        .request(
            Method::PUT,
            &format!("/api/admin/appointments/{appointment_id}"),
            &headers,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(slot(4, 16)),
                "name": "Şule Öztürk", "phone": "+905551112233"
            })),
        )
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    {
        let state = shared.lock().unwrap();
        assert_eq!(state.calls.len(), 2);
        let (method, target, _) = &state.calls[1];
        assert_eq!(method, "PATCH");
        assert_eq!(
            target, "antrenor@example.com/evt-1",
            "aynı etkinlik güncellenmeli"
        );
    }

    // İptal etkinliği silmeli.
    server
        .request(
            Method::PATCH,
            &format!("/api/admin/appointments/{appointment_id}"),
            &headers,
            Some(json!({ "action": "cancel", "adminNote": "" })),
        )
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    {
        let state = shared.lock().unwrap();
        assert_eq!(state.calls.len(), 3);
        let (method, target, _) = &state.calls[2];
        assert_eq!(method, "DELETE");
        assert_eq!(target, "antrenor@example.com/evt-1");
    }
}

#[tokio::test]
async fn a_failed_sync_is_retried_and_surfaced() {
    let shared: Shared = Arc::default();
    let api_base = fake_google(shared.clone()).await;
    let server = common::start_with_calendar(Some(calendar_for(&api_base))).await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let read = [("Cookie", cookie.as_str())];

    server
        .request(
            Method::PUT,
            "/api/admin/calendar",
            &headers,
            Some(json!({ "calendarId": "antrenor@example.com", "enabled": true })),
        )
        .await;

    // Sıradaki istek patlasın.
    shared.lock().unwrap().fail_next = 1;

    let now = now_ms();
    let local = civil_from_ms(now + BOOKING_RULES.offset_ms());
    let start = utc_ms_hm(
        local.year,
        local.month as i64 - 1,
        local.day as i64 + 3,
        10,
        0,
    ) - BOOKING_RULES.offset_ms();
    server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(start),
                "name": "Hata Testi", "phone": "+905559990000"
            })),
        )
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    // Hata panelde görünür olmalı.
    let status = server
        .request(Method::GET, "/api/admin/calendar", &read, None)
        .await;
    assert!(
        status.body["lastError"].as_str().unwrap().contains("500"),
        "hata yüzeye çıkmalı: {:?}",
        status.body["lastError"]
    );
    assert!(status.body["lastErrorAt"].is_number());

    // Kayıt kuyrukta kalmalı; ikinci tur onu yerine oturtmalı.
    let recovered = server
        .request(
            Method::PUT,
            "/api/admin/calendar",
            &headers,
            Some(json!({ "calendarId": "antrenor@example.com", "enabled": true })),
        )
        .await;
    assert_eq!(recovered.body["synced"], 1, "yeniden denenmeli");
    assert_eq!(
        recovered.body["lastError"], "",
        "başarıdan sonra hata temizlenmeli"
    );
    assert_eq!(
        shared.lock().unwrap().calls.len(),
        2,
        "biri hatalı, biri başarılı"
    );
}

#[tokio::test]
async fn disabling_the_connection_stops_all_traffic() {
    let shared: Shared = Arc::default();
    let api_base = fake_google(shared.clone()).await;
    let server = common::start_with_calendar(Some(calendar_for(&api_base))).await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    // Takvim kimliği olmadan açılamaz.
    let missing = server
        .request(
            Method::PUT,
            "/api/admin/calendar",
            &headers,
            Some(json!({ "calendarId": "", "enabled": true })),
        )
        .await;
    assert_eq!(missing.status, 400);
    assert_eq!(
        missing.body["error"]["message"],
        "Takvim kimliği girilmelidir."
    );

    // Kapatmak için kimlik gerekmez.
    let off = server
        .request(
            Method::PUT,
            "/api/admin/calendar",
            &headers,
            Some(json!({ "calendarId": "", "enabled": false })),
        )
        .await;
    assert_eq!(off.status, 200);

    let now = now_ms();
    let local = civil_from_ms(now + BOOKING_RULES.offset_ms());
    let start = utc_ms_hm(
        local.year,
        local.month as i64 - 1,
        local.day as i64 + 3,
        9,
        0,
    ) - BOOKING_RULES.offset_ms();
    server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(start),
                "name": "Kapalı", "phone": "+905558880000"
            })),
        )
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        shared.lock().unwrap().calls.is_empty(),
        "kapalıyken istek gitmemeli"
    );
}
