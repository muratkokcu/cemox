//! `test/app.test.js` paketinin Rust karşılığı; aynı akışları ve aynı beklentileri doğrular.

mod common;

use cemox_server::config::BOOKING_RULES;
use cemox_server::db::{Db, NewAppointment};
use cemox_server::time::{now_ms, parse_timestamp, to_iso_string};
use reqwest::Method;
use serde_json::{Value, json};

fn slot_starts(availability: &Value) -> Vec<String> {
    availability["days"]
        .as_array()
        .map(|days| {
            days.iter()
                .flat_map(|day| day["slots"].as_array().cloned().unwrap_or_default())
                .filter_map(|slot| slot["start"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn public_booking_and_admin_approval_flow() {
    let server = common::start().await;

    let services = server.get("/api/services").await;
    assert_eq!(services.status, 200);
    assert_eq!(services.body["services"].as_array().unwrap().len(), 6);

    let availability = server
        .get("/api/availability?service=medical-fitness")
        .await;
    assert_eq!(availability.status, 200);
    assert!(!availability.body["days"].as_array().unwrap().is_empty());
    let slot = availability.body["days"][0]["slots"][0]["start"]
        .as_str()
        .unwrap()
        .to_string();
    let second_slot = availability.body["days"][0]["slots"][1]["start"]
        .as_str()
        .unwrap()
        .to_string();

    let booking = json!({
        "serviceId": "medical-fitness",
        "start": slot,
        "name": "Test Kullanıcı",
        "email": "test@example.com",
        "phone": "+905551112233",
        "note": "Hareket kalitesi hakkında görüşmek istiyorum.",
        "consent": true,
        "website": "",
        "startedAt": now_ms() - 5_000
    });

    let created = server.post("/api/appointments", booking.clone()).await;
    assert_eq!(created.status, 201, "{:?}", created.body);
    assert_eq!(created.body["appointment"]["status"], "PENDING");
    let appointment_id = created.body["appointment"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Aynı kişi ikinci bir bekleyen seçim oluşturamaz.
    let mut duplicate_contact = booking.clone();
    duplicate_contact["start"] = json!(second_slot);
    let response = server.post("/api/appointments", duplicate_contact).await;
    assert_eq!(response.status, 409, "{:?}", response.body);

    // Aynı saat başka bir kişiye de verilmez.
    let mut duplicate_slot = booking.clone();
    duplicate_slot["email"] = json!("other@example.com");
    duplicate_slot["phone"] = json!("+905559998877");
    let response = server.post("/api/appointments", duplicate_slot).await;
    assert_eq!(response.status, 409, "{:?}", response.body);

    let unauthorized = server.get("/api/admin/appointments").await;
    assert_eq!(unauthorized.status, 401);

    let wrong_login = server
        .post("/api/admin/login", json!({ "password": "wrong-password" }))
        .await;
    assert_eq!(wrong_login.status, 401);
    assert_eq!(wrong_login.body["error"]["message"], "Şifre hatalı.");

    let (cookie, csrf) = server.login().await;

    // CSRF başlığı olmadan karar verilemez.
    let csrf_failure = server
        .request(
            Method::PATCH,
            &format!("/api/admin/appointments/{appointment_id}"),
            &[("Cookie", cookie.as_str())],
            Some(json!({ "action": "approve", "adminNote": "" })),
        )
        .await;
    assert_eq!(csrf_failure.status, 403);

    let approved = server
        .request(
            Method::PATCH,
            &format!("/api/admin/appointments/{appointment_id}"),
            &[("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())],
            Some(json!({ "action": "approve", "adminNote": "" })),
        )
        .await;
    assert_eq!(approved.status, 200, "{:?}", approved.body);
    assert_eq!(approved.body["appointment"]["status"], "APPROVED");

    // Onaylanan saat artık müsaitlik listesinde görünmez.
    let refreshed = server
        .get("/api/availability?service=medical-fitness")
        .await;
    assert!(!slot_starts(&refreshed.body).contains(&slot));
}

#[tokio::test]
async fn admin_can_add_and_remove_an_availability_block() {
    let server = common::start().await;

    let availability = server
        .get("/api/availability?service=kisisel-antrenman")
        .await;
    let start = availability.body["days"][0]["slots"][0]["start"]
        .as_str()
        .unwrap()
        .to_string();
    let end = to_iso_string(parse_timestamp(&start).unwrap() + 60 * 60_000);

    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    let created = server
        .request(
            Method::POST,
            "/api/admin/blocks",
            &headers,
            Some(json!({ "start": start, "end": end, "reason": "Test engeli" })),
        )
        .await;
    assert_eq!(created.status, 201, "{:?}", created.body);

    let refreshed = server
        .get("/api/availability?service=kisisel-antrenman")
        .await;
    assert!(!slot_starts(&refreshed.body).contains(&start));

    let block_id = created.body["block"]["id"].as_str().unwrap();
    let removed = server
        .request(
            Method::DELETE,
            &format!("/api/admin/blocks/{block_id}"),
            &headers,
            None,
        )
        .await;
    assert_eq!(removed.status, 204);
}

#[tokio::test]
async fn admin_toggles_service_specific_calendar_slots() {
    let server = common::start().await;

    // Bu branş için hiç saat açılmadı.
    let before = server
        .get("/api/availability?service=fonksiyonel-antrenman")
        .await;
    assert_eq!(before.body["days"].as_array().unwrap().len(), 0);

    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    let local_now = cemox_server::time::civil_from_ms(now_ms() + BOOKING_RULES.offset_ms());
    let start_at = cemox_server::time::utc_ms_hm(
        local_now.year,
        local_now.month as i64 - 1,
        local_now.day as i64 + 3,
        10,
        0,
    ) - BOOKING_RULES.offset_ms();

    let created = server
        .request(
            Method::PUT,
            "/api/admin/availability-slots",
            &headers,
            Some(json!({
                "serviceId": "fonksiyonel-antrenman",
                "start": to_iso_string(start_at),
                "open": true
            })),
        )
        .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(created.body["open"], true);

    let after = server
        .get("/api/availability?service=fonksiyonel-antrenman")
        .await;
    assert!(!after.body["days"].as_array().unwrap().is_empty());

    let removed = server
        .request(
            Method::PUT,
            "/api/admin/availability-slots",
            &headers,
            Some(json!({
                "serviceId": "fonksiyonel-antrenman",
                "start": to_iso_string(start_at),
                "open": false
            })),
        )
        .await;
    assert_eq!(removed.status, 200);
    assert_eq!(removed.body["open"], false);

    let empty_again = server
        .get("/api/availability?service=fonksiyonel-antrenman")
        .await;
    assert_eq!(empty_again.body["days"].as_array().unwrap().len(), 0);
}

#[test]
fn pending_holds_expire_after_24_hours() {
    let db = Db::new(":memory:").expect("veritabanı açılamadı");
    let now = now_ms();
    let start_at = now + 48 * 3_600_000;

    let appointment = db
        .create_appointment(
            &NewAppointment {
                service_id: "medical-fitness".into(),
                service_name: "Medical Fitness".into(),
                start_at,
                end_at: start_at + BOOKING_RULES.slot_ms(),
                name: "Süre Testi".into(),
                email: "expiry@example.com".into(),
                phone: "+905551234567".into(),
                note: String::new(),
            },
            now,
        )
        .expect("randevu oluşturulamadı");
    assert_eq!(appointment.status, "PENDING");

    let expired = db
        .expire_pending(now + 24 * 3_600_000 + 1)
        .expect("süre dolumu başarısız");
    assert_eq!(expired.len(), 1);
    assert_eq!(
        db.get_appointment(&appointment.id).unwrap().unwrap().status,
        "EXPIRED"
    );
}

#[tokio::test]
async fn admin_blocks_can_be_listed_for_an_arbitrary_range() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    // Varsayılan 90 günlük pencerenin dışında iki kayıt.
    for (start, end, reason) in [
        (
            "2020-01-05T00:00:00.000Z",
            "2020-01-06T00:00:00.000Z",
            "Geçmiş",
        ),
        (
            "2040-05-01T00:00:00.000Z",
            "2040-05-03T00:00:00.000Z",
            "Uzak gelecek",
        ),
    ] {
        let created = server
            .request(
                Method::POST,
                "/api/admin/blocks",
                &headers,
                Some(json!({ "start": start, "end": end, "reason": reason })),
            )
            .await;
        assert_eq!(created.status, 201, "{:?}", created.body);
    }

    // Parametresiz istek yalnızca bugünden itibaren 90 günü kapsar.
    let default_window = server
        .request(Method::GET, "/api/admin/blocks", &headers, None)
        .await;
    assert_eq!(default_window.status, 200);
    assert_eq!(default_window.body["blocks"].as_array().unwrap().len(), 0);

    // Geçmiş aralık artık erişilebilir.
    let past = server
        .request(
            Method::GET,
            "/api/admin/blocks?from=2020-01-01T00:00:00.000Z&to=2020-02-01T00:00:00.000Z",
            &headers,
            None,
        )
        .await;
    assert_eq!(past.status, 200);
    assert_eq!(past.body["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(past.body["blocks"][0]["reason"], "Geçmiş");

    // 90 günden uzaktaki aralık da.
    let future = server
        .request(
            Method::GET,
            "/api/admin/blocks?from=2040-05-01T00:00:00.000Z&to=2040-06-01T00:00:00.000Z",
            &headers,
            None,
        )
        .await;
    assert_eq!(future.body["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(future.body["blocks"][0]["reason"], "Uzak gelecek");

    // Geçersiz aralıklar reddedilir.
    for (query, message) in [
        (
            "?from=2040-06-01T00:00:00.000Z&to=2040-05-01T00:00:00.000Z",
            "Kapalı zaman aralığı geçerli değil.",
        ),
        (
            "?from=2020-01-01T00:00:00.000Z&to=2040-01-01T00:00:00.000Z",
            "Kapalı zaman aralığı geçerli değil.",
        ),
        ("?from=yarin", "Başlangıç geçerli değil."),
    ] {
        let response = server
            .request(
                Method::GET,
                &format!("/api/admin/blocks{query}"),
                &headers,
                None,
            )
            .await;
        assert_eq!(response.status, 400, "{query}: {:?}", response.body);
        assert_eq!(response.body["error"]["message"], message, "{query}");
    }

    // Boş parametreler yok sayılır ve varsayılan pencereye düşer.
    let empty = server
        .request(Method::GET, "/api/admin/blocks?from=&to=", &headers, None)
        .await;
    assert_eq!(empty.status, 200);
    assert_eq!(empty.body["blocks"].as_array().unwrap().len(), 0);
}
