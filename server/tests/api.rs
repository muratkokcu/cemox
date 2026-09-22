//! `test/app.test.js` paketinin Rust karşılığı; aynı akışları ve aynı beklentileri doğrular.

mod common;

use cemox_server::config::BOOKING_RULES;
use cemox_server::db::{Db, NewAppointment};
use cemox_server::time::{civil_from_ms, now_ms, parse_timestamp, to_iso_string, utc_ms_hm};
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

#[tokio::test]
async fn admin_appointments_are_filtered_and_paginated_server_side() {
    let server = common::start().await;
    let (cookie, _csrf) = server.login().await;
    let read = [("Cookie", cookie.as_str())];

    // Sayfa sınırını aşacak kadar kayıt üret. Doğrudan veritabanına yazılır:
    // HTTP üzerinden kişi başına tek bekleyen talep kuralı buna izin vermez.
    let now = now_ms();
    for index in 0..60i64 {
        let start_at = now + (2 + index) * 86_400_000;
        server
            .db
            .create_appointment(
                &NewAppointment {
                    service_id: if index % 2 == 0 {
                        "medical-fitness".into()
                    } else {
                        "kisisel-antrenman".into()
                    },
                    service_name: "Test".into(),
                    start_at,
                    end_at: start_at + BOOKING_RULES.slot_ms(),
                    name: format!("Kayıt {index}"),
                    email: format!("kayit{index}@example.com"),
                    phone: format!("+90555000{index:04}"),
                    note: String::new(),
                },
                now,
            )
            .expect("randevu eklenemedi");
    }

    // Toplam sayı sayfa boyutundan bağımsızdır.
    let first = server
        .request(
            Method::GET,
            "/api/admin/appointments?limit=25&offset=0",
            &read,
            None,
        )
        .await;
    assert_eq!(first.status, 200);
    assert_eq!(first.body["appointments"].as_array().unwrap().len(), 25);
    assert_eq!(first.body["total"], 60);

    // Sayfalar örtüşmez ve sıralama kararlıdır.
    let second = server
        .request(
            Method::GET,
            "/api/admin/appointments?limit=25&offset=25",
            &read,
            None,
        )
        .await;
    let ids = |response: &Value| -> Vec<String> {
        response["appointments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_string())
            .collect()
    };
    let page_one = ids(&first.body);
    let page_two = ids(&second.body);
    assert!(
        page_one.iter().all(|id| !page_two.contains(id)),
        "sayfalar örtüşmemeli"
    );

    let repeat = server
        .request(
            Method::GET,
            "/api/admin/appointments?limit=25&offset=25",
            &read,
            None,
        )
        .await;
    assert_eq!(
        ids(&repeat.body),
        page_two,
        "aynı sayfa aynı sırayla dönmeli"
    );

    // Branş filtresi.
    let by_service = server
        .request(
            Method::GET,
            "/api/admin/appointments?service=medical-fitness&limit=100",
            &read,
            None,
        )
        .await;
    assert_eq!(by_service.body["total"], 30);
    assert!(
        by_service.body["appointments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["service_id"] == "medical-fitness")
    );

    // Tarih aralığı filtresi: takvim görünümünün kullandığı sorgu.
    let ranged = server
        .request(
            Method::GET,
            &format!(
                "/api/admin/appointments?from={}&to={}&limit=500",
                to_iso_string(now),
                to_iso_string(now + 12 * 86_400_000)
            ),
            &read,
            None,
        )
        .await;
    assert_eq!(ranged.status, 200, "{:?}", ranged.body);
    // 2..=11 gün sonrasındaki 10 kayıt aralığa girer.
    assert_eq!(ranged.body["total"], 10);

    // Geçersiz parametreler.
    for (query, message) in [
        ("?limit=0", "Kayıt sayısı geçerli değil."),
        ("?limit=501", "Kayıt sayısı geçerli değil."),
        ("?limit=abc", "Kayıt sayısı geçerli değil."),
        ("?offset=-1", "Başlangıç konumu geçerli değil."),
        ("?service=yok", "Geçerli bir hizmet seçin."),
        ("?from=2026-01-01T00:00:00.000Z", "Bitiş geçerli değil."),
        (
            "?from=2026-02-01T00:00:00.000Z&to=2026-01-01T00:00:00.000Z",
            "Randevu aralığı geçerli değil.",
        ),
    ] {
        let response = server
            .request(
                Method::GET,
                &format!("/api/admin/appointments{query}"),
                &read,
                None,
            )
            .await;
        assert_eq!(response.status, 400, "{query}: {:?}", response.body);
        assert_eq!(response.body["error"]["message"], message, "{query}");
    }

    // Boş parametreler varsayılana düşer.
    let empty = server
        .request(
            Method::GET,
            "/api/admin/appointments?limit=&offset=&status=&service=&from=&to=",
            &read,
            None,
        )
        .await;
    assert_eq!(empty.status, 200);
    assert_eq!(empty.body["total"], 60);
}

#[tokio::test]
async fn admin_can_write_many_slots_in_one_request() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    // 30 dakikalık ızgarada, gelecekteki saatler.
    let now = now_ms();
    let local = civil_from_ms(now + BOOKING_RULES.offset_ms());
    let grid = |day_offset: i64, index: i64| -> i64 {
        let minutes = 8 * 60 + index * 30;
        utc_ms_hm(
            local.year,
            local.month as i64 - 1,
            local.day as i64 + day_offset,
            minutes / 60,
            minutes % 60,
        ) - BOOKING_RULES.offset_ms()
    };

    // Bir günün 28 saati tek istekte.
    let day: Vec<Value> = (0..28)
        .map(|index| json!({ "start": to_iso_string(grid(3, index)), "open": true }))
        .collect();
    let opened = server
        .request(
            Method::PUT,
            "/api/admin/availability-slots/bulk",
            &headers,
            Some(json!({ "serviceId": "medical-fitness", "slots": day })),
        )
        .await;
    assert_eq!(opened.status, 200, "{:?}", opened.body);
    assert_eq!(opened.body["applied"], 28);

    let listed = server
        .request(
            Method::GET,
            &format!(
                "/api/admin/availability-slots?service=medical-fitness&from={}&to={}",
                to_iso_string(grid(3, 0) - 3_600_000),
                to_iso_string(grid(3, 27) + 3_600_000)
            ),
            &headers,
            None,
        )
        .await;
    assert_eq!(listed.body["slots"].as_array().unwrap().len(), 28);

    // Karışık açık/kapalı: kopyalama senaryosu.
    let mixed: Vec<Value> = (0..28)
        .map(|index| json!({ "start": to_iso_string(grid(3, index)), "open": index % 2 == 0 }))
        .collect();
    let applied = server
        .request(
            Method::PUT,
            "/api/admin/availability-slots/bulk",
            &headers,
            Some(json!({ "serviceId": "medical-fitness", "slots": mixed })),
        )
        .await;
    assert_eq!(applied.body["applied"], 28);
    let after = server
        .request(
            Method::GET,
            &format!(
                "/api/admin/availability-slots?service=medical-fitness&from={}&to={}",
                to_iso_string(grid(3, 0) - 3_600_000),
                to_iso_string(grid(3, 27) + 3_600_000)
            ),
            &headers,
            None,
        )
        .await;
    assert_eq!(
        after.body["slots"].as_array().unwrap().len(),
        14,
        "yalnızca açık işaretlenenler kalmalı"
    );

    // Doğrulamalar tek slotluk uçla aynı mesajları verir.
    for (body, message) in [
        (
            json!({ "serviceId": "medical-fitness", "slots": [] }),
            "En az bir saat seçilmelidir.",
        ),
        (
            json!({ "serviceId": "yok", "slots": [{ "start": to_iso_string(grid(3, 0)), "open": true }] }),
            "Geçerli bir hizmet seçin.",
        ),
        (
            json!({ "serviceId": "medical-fitness", "slots": [{ "start": "2020-01-01T07:00:00.000Z", "open": true }] }),
            "Geçmiş bir saat değiştirilemez.",
        ),
        (
            json!({ "serviceId": "medical-fitness", "slots": [{ "start": "2040-01-01T07:07:00.000Z", "open": true }] }),
            "Saat takvim ızgarasına uymuyor.",
        ),
        (
            json!({ "serviceId": "medical-fitness", "slots": [{ "start": "yarin", "open": true }] }),
            "Slot zamanı geçerli değil.",
        ),
        (
            json!({ "serviceId": "medical-fitness", "slots": [
                { "start": to_iso_string(grid(4, 0)), "open": true },
                { "start": to_iso_string(grid(4, 0)), "open": false }
            ] }),
            "Aynı saat birden fazla kez gönderildi.",
        ),
    ] {
        let response = server
            .request(
                Method::PUT,
                "/api/admin/availability-slots/bulk",
                &headers,
                Some(body),
            )
            .await;
        assert_eq!(response.status, 400, "{:?}", response.body);
        assert_eq!(response.body["error"]["message"], message);
    }

    // Sınır aşımı.
    let too_many: Vec<Value> = (0..1001)
        .map(
            |index| json!({ "start": to_iso_string(grid(3, 0) + index * 1_800_000), "open": true }),
        )
        .collect();
    let rejected = server
        .request(
            Method::PUT,
            "/api/admin/availability-slots/bulk",
            &headers,
            Some(json!({ "serviceId": "medical-fitness", "slots": too_many })),
        )
        .await;
    assert_eq!(rejected.status, 400);
    assert_eq!(
        rejected.body["error"]["message"],
        "Tek seferde en fazla 1000 saat değiştirilebilir."
    );

    // CSRF başlığı olmadan yazılamaz.
    let no_csrf = server
        .request(
            Method::PUT,
            "/api/admin/availability-slots/bulk",
            &[("Cookie", cookie.as_str())],
            Some(json!({ "serviceId": "medical-fitness", "slots": [{ "start": to_iso_string(grid(5, 0)), "open": true }] })),
        )
        .await;
    assert_eq!(no_csrf.status, 403);
}

#[tokio::test]
async fn pending_appointments_are_ordered_by_hold_expiry() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let read = [("Cookie", cookie.as_str())];

    // Randevu tarihi ile talep sırası kasten ters: en geç randevu en eski taleptir.
    // Aciliyet sıralaması doğruysa listede o başa gelmelidir.
    let now = now_ms();
    for (index, hours_ago) in [2i64, 20, 11].into_iter().enumerate() {
        let start_at = now + (30 - index as i64) * 86_400_000;
        server
            .db
            .create_appointment(
                &NewAppointment {
                    service_id: "medical-fitness".into(),
                    service_name: "Test".into(),
                    start_at,
                    end_at: start_at + BOOKING_RULES.slot_ms(),
                    name: format!("{hours_ago} saat önce"),
                    email: format!("hold{hours_ago}@example.com"),
                    phone: format!("+9055500{hours_ago:05}"),
                    note: String::new(),
                },
                now - hours_ago * 3_600_000,
            )
            .expect("randevu eklenemedi");
    }

    let names = |response: &Value| -> Vec<String> {
        response["appointments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_string())
            .collect()
    };

    // En eski talep (süresi ilk dolacak) başta.
    let filtered = server
        .request(
            Method::GET,
            "/api/admin/appointments?status=PENDING",
            &read,
            None,
        )
        .await;
    assert_eq!(
        names(&filtered.body),
        vec!["20 saat önce", "11 saat önce", "2 saat önce"]
    );

    // Filtresiz listede de bekleyenler aynı sırayla ve başta yer alır.
    let all = server
        .request(Method::GET, "/api/admin/appointments", &read, None)
        .await;
    assert_eq!(
        names(&all.body),
        vec!["20 saat önce", "11 saat önce", "2 saat önce"]
    );

    // Karara bağlananlar randevu tarihine göre sıralanmaya devam eder.
    let pending_ids: Vec<String> = all.body["appointments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    for id in &pending_ids {
        server
            .request(
                Method::PATCH,
                &format!("/api/admin/appointments/{id}"),
                &[("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())],
                Some(json!({ "action": "reject", "adminNote": "" })),
            )
            .await;
    }
    let decided = server
        .request(
            Method::GET,
            "/api/admin/appointments?status=REJECTED",
            &read,
            None,
        )
        .await;
    assert_eq!(
        names(&decided.body),
        vec!["11 saat önce", "20 saat önce", "2 saat önce"],
        "karara bağlananlar randevu tarihine göre sıralanır"
    );
}

#[tokio::test]
async fn admin_can_search_appointments_and_read_status_counts() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let read = [("Cookie", cookie.as_str())];

    let now = now_ms();
    let people = [
        ("Şule Öztürk", "sule.ozturk@example.com", "+905551112233"),
        ("Ahmet Çağlar", "ahmet@example.com", "+905324445566"),
        ("Iğdır Gümüş", "igdir@example.com", "+905337778899"),
        ("Ali Veli", "ali%veli@example.com", "+905441234567"),
    ];
    for (index, (name, email, phone)) in people.iter().enumerate() {
        let start_at = now + (2 + index as i64) * 86_400_000;
        server
            .db
            .create_appointment(
                &NewAppointment {
                    service_id: "medical-fitness".into(),
                    service_name: "Test".into(),
                    start_at,
                    end_at: start_at + BOOKING_RULES.slot_ms(),
                    name: (*name).into(),
                    email: (*email).into(),
                    phone: (*phone).into(),
                    note: String::new(),
                },
                now,
            )
            .expect("randevu eklenemedi");
    }

    let names = |response: &Value| -> Vec<String> {
        response["appointments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_string())
            .collect()
    };
    let search = async |term: &str| -> Value {
        server
            .request(
                Method::GET,
                &format!("/api/admin/appointments?q={term}"),
                &read,
                None,
            )
            .await
            .body
    };

    // Türkçe karakterler katlanır: ASCII yazarak da bulunur, tersi de geçerli.
    assert_eq!(
        names(&search("sule").await),
        vec!["Şule Öztürk"],
        "sule -> Şule"
    );
    assert_eq!(
        names(&search("Şule").await),
        vec!["Şule Öztürk"],
        "Şule -> Şule"
    );
    assert_eq!(
        names(&search("ozturk").await),
        vec!["Şule Öztürk"],
        "soyadı da katlanır"
    );
    assert_eq!(
        names(&search("caglar").await),
        vec!["Ahmet Çağlar"],
        "caglar -> Çağlar"
    );
    assert_eq!(
        names(&search("gumus").await),
        vec!["Iğdır Gümüş"],
        "gumus -> Gümüş"
    );
    assert_eq!(
        names(&search("igdir").await),
        vec!["Iğdır Gümüş"],
        "büyük I ve ı aynı katlanır"
    );
    assert_eq!(
        names(&search("AHMET").await),
        vec!["Ahmet Çağlar"],
        "büyük harf duyarsız"
    );

    // E-posta ve telefon da aranır.
    assert_eq!(names(&search("ahmet@example").await), vec!["Ahmet Çağlar"]);
    assert_eq!(
        names(&search("5324445566").await),
        vec!["Ahmet Çağlar"],
        "telefon"
    );

    // LIKE jokerleri kaçırılır. Kayıtlardan yalnızca birinin e-postasında '%' geçiyor;
    // kaçırma bozuk olsaydı joker olarak yorumlanıp dört kaydı da eşlerdi.
    assert_eq!(
        names(&search("%").await),
        vec!["Ali Veli"],
        "% joker değil, harf olarak aranır"
    );
    assert_eq!(names(&search("ali%veli").await), vec!["Ali Veli"]);
    assert_eq!(search("_").await["total"], 0, "_ de joker sayılmamalı");

    // Eşleşme yoksa boş.
    assert_eq!(search("bulunmayan").await["total"], 0);

    // Sekme sayıları durum filtresinden bağımsız, ama aramayı dikkate alır.
    let all = server
        .request(Method::GET, "/api/admin/appointments", &read, None)
        .await;
    assert_eq!(all.body["counts"]["PENDING"], 4);
    assert_eq!(
        all.body["counts"]["APPROVED"], 0,
        "hiç yoksa sıfır olarak döner"
    );

    let scoped = search("sule").await;
    assert_eq!(scoped["counts"]["PENDING"], 1, "sayaçlar aramayla daralır");

    // Bir kaydı onayla; sayaçlar buna göre değişmeli.
    let id = all.body["appointments"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    server
        .request(
            Method::PATCH,
            &format!("/api/admin/appointments/{id}"),
            &[("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())],
            Some(json!({ "action": "approve", "adminNote": "" })),
        )
        .await;
    let after = server
        .request(Method::GET, "/api/admin/appointments", &read, None)
        .await;
    assert_eq!(after.body["counts"]["PENDING"], 3);
    assert_eq!(after.body["counts"]["APPROVED"], 1);
    assert_eq!(after.body["total"], 4, "toplam durumdan bağımsız");

    // Durum filtresi uygulanınca toplam daralır ama sayaçlar tüm dağılımı verir.
    let pending = server
        .request(
            Method::GET,
            "/api/admin/appointments?status=PENDING",
            &read,
            None,
        )
        .await;
    assert_eq!(pending.body["total"], 3);
    assert_eq!(pending.body["counts"]["APPROVED"], 1);

    // Çok uzun arama reddedilir.
    let long = server
        .request(
            Method::GET,
            &format!("/api/admin/appointments?q={}", "a".repeat(101)),
            &read,
            None,
        )
        .await;
    assert_eq!(long.status, 400);
    assert_eq!(long.body["error"]["message"], "Arama metni çok uzun.");
}

#[tokio::test]
async fn admin_can_create_an_appointment_by_hand() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let read = [("Cookie", cookie.as_str())];

    // 30 dakikalık ızgarada, yayınlanmamış bir saat seç.
    let now = now_ms();
    let local = civil_from_ms(now + BOOKING_RULES.offset_ms());
    let slot = |day_offset: i64, hour: i64, minute: i64| -> i64 {
        utc_ms_hm(
            local.year,
            local.month as i64 - 1,
            local.day as i64 + day_offset,
            hour,
            minute,
        ) - BOOKING_RULES.offset_ms()
    };
    let start_at = slot(1, 19, 0); // yarın 19:00 — kamuya açık takvimde sunulmuyor

    // Kamuya açık akış bu saati reddeder: yayınlanmamış ve bildirim süresi dolmamış.
    let public = server
        .post(
            "/api/appointments",
            json!({
                "serviceId": "medical-fitness", "start": to_iso_string(start_at),
                "name": "Telefon Danışanı", "email": "telefon@example.com",
                "phone": "+905551110000", "note": "", "consent": true,
                "website": "", "startedAt": now - 5_000
            }),
        )
        .await;
    assert_ne!(
        public.status, 201,
        "kamuya açık akış yayınlanmamış saati kabul etmemeli"
    );

    // Yönetici aynı saate elle randevu oluşturabilir.
    let created = server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness",
                "start": to_iso_string(start_at),
                "name": "Telefon Danışanı",
                "phone": "0555 111 00 00",
                "note": "Telefonla talep etti."
            })),
        )
        .await;
    assert_eq!(created.status, 201, "{:?}", created.body);
    let appointment = &created.body["appointment"];
    assert_eq!(
        appointment["status"], "APPROVED",
        "elle giriş doğrudan onaylıdır"
    );
    assert_eq!(
        appointment["phone"], "05551110000",
        "telefon rakamlara indirgenir"
    );
    assert_eq!(appointment["email"], "", "e-posta isteğe bağlıdır");
    assert!(
        appointment["admin_note"]
            .as_str()
            .unwrap()
            .contains("elle oluşturuldu"),
        "kaynağı yönetici notunda kalır"
    );
    assert!(
        appointment["decision_at"].is_number(),
        "karar zamanı yazılır"
    );

    // Aynı saat artık dolu: ikinci giriş de, kamuya açık rezervasyon da reddedilir.
    let duplicate = server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "kisisel-antrenman", "start": to_iso_string(start_at),
                "name": "Başka Kişi", "phone": "+905552220000"
            })),
        )
        .await;
    assert_eq!(duplicate.status, 409);
    assert_eq!(
        duplicate.body["error"]["message"],
        "Bu saat başka bir randevu veya kapalı zamanla çakışıyor."
    );

    // Kapalı zamanla çakışan saat de reddedilir.
    let blocked_start = slot(2, 15, 0);
    server
        .request(
            Method::POST,
            "/api/admin/blocks",
            &headers,
            Some(json!({
                "start": to_iso_string(blocked_start),
                "end": to_iso_string(blocked_start + 3_600_000),
                "reason": "Toplantı"
            })),
        )
        .await;
    let in_block = server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(blocked_start),
                "name": "Kapalı Zaman", "phone": "+905553330000"
            })),
        )
        .await;
    assert_eq!(in_block.status, 409);

    // Doğrulamalar.
    let base = json!({
        "serviceId": "medical-fitness", "start": to_iso_string(slot(3, 11, 0)),
        "name": "Geçerli Ad", "phone": "+905554440000"
    });
    let with = |key: &str, value: Value| {
        let mut body = base.clone();
        body[key] = value;
        body
    };
    for (body, message) in [
        (with("serviceId", json!("yok")), "Geçerli bir hizmet seçin."),
        (
            with("start", json!(to_iso_string(slot(3, 11, 7)))),
            "Saat takvim ızgarasına uymuyor.",
        ),
        (
            with("start", json!("2020-01-01T07:00:00.000Z")),
            "Geçmiş bir saate randevu oluşturulamaz.",
        ),
        (
            with("start", json!(to_iso_string(slot(400, 11, 0)))),
            "Randevu en fazla bir yıl sonrasına oluşturulabilir.",
        ),
        (
            with("name", json!("A")),
            "Ad soyad alanı 2–100 karakter olmalıdır.",
        ),
        (
            with("phone", json!("123")),
            "Geçerli bir telefon numarası girin.",
        ),
        (
            with("email", json!("bozuk")),
            "Geçerli bir e-posta adresi girin.",
        ),
    ] {
        let response = server
            .request(
                Method::POST,
                "/api/admin/appointments",
                &headers,
                Some(body),
            )
            .await;
        assert_eq!(response.status, 400, "{:?}", response.body);
        assert_eq!(response.body["error"]["message"], message);
    }

    // CSRF başlığı olmadan oluşturulamaz.
    let no_csrf = server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &read,
            Some(base.clone()),
        )
        .await;
    assert_eq!(no_csrf.status, 403);

    // Elle giriş listede ve takvim sorgusunda görünür.
    let listed = server
        .request(
            Method::GET,
            "/api/admin/appointments?status=APPROVED",
            &read,
            None,
        )
        .await;
    assert_eq!(listed.body["total"], 1);
    assert_eq!(listed.body["appointments"][0]["name"], "Telefon Danışanı");

    // Ve o saat artık kamuya açık müsaitlikte sunulmaz.
    let availability = server
        .get("/api/availability?service=medical-fitness")
        .await;
    assert!(!slot_starts(&availability.body).contains(&to_iso_string(start_at)));
}

#[tokio::test]
async fn admin_can_edit_and_reschedule_an_appointment() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let read = [("Cookie", cookie.as_str())];

    let now = now_ms();
    let local = civil_from_ms(now + BOOKING_RULES.offset_ms());
    let slot = |day: i64, hour: i64, minute: i64| -> i64 {
        utc_ms_hm(
            local.year,
            local.month as i64 - 1,
            local.day as i64 + day,
            hour,
            minute,
        ) - BOOKING_RULES.offset_ms()
    };

    let create = async |start: i64, name: &str, phone: &str| -> Value {
        server
            .request(
                Method::POST,
                "/api/admin/appointments",
                &headers,
                Some(json!({
                    "serviceId": "medical-fitness", "start": to_iso_string(start),
                    "name": name, "phone": phone, "email": "duzenle@example.com"
                })),
            )
            .await
            .body["appointment"]
            .clone()
    };
    let update = async |id: &str, body: Value| -> common::ApiResponse {
        server
            .request(
                Method::PUT,
                &format!("/api/admin/appointments/{id}"),
                &headers,
                Some(body),
            )
            .await
    };

    let original = create(slot(3, 14, 0), "İlk Ad", "+905551110000").await;
    let id = original["id"].as_str().unwrap().to_string();

    // Saat değişmeden alan düzeltmesi.
    let renamed = update(
        &id,
        json!({
            "serviceId": "medical-fitness", "start": to_iso_string(slot(3, 14, 0)),
            "name": "Düzeltilmiş Ad", "phone": "+905559998877",
            "email": "yeni@example.com", "note": "Notu da güncelledik."
        }),
    )
    .await;
    assert_eq!(renamed.status, 200, "{:?}", renamed.body);
    assert_eq!(renamed.body["appointment"]["name"], "Düzeltilmiş Ad");
    assert_eq!(renamed.body["appointment"]["phone"], "+905559998877");
    assert_eq!(renamed.body["appointment"]["email"], "yeni@example.com");
    assert_eq!(renamed.body["appointment"]["note"], "Notu da güncelledik.");
    assert_eq!(
        renamed.body["appointment"]["status"], "APPROVED",
        "düzenlemek karar vermek değildir, durum korunur"
    );

    // Boş saate taşıma.
    let moved = update(
        &id,
        json!({
            "serviceId": "kisisel-antrenman", "start": to_iso_string(slot(4, 16, 30)),
            "name": "Düzeltilmiş Ad", "phone": "+905559998877", "email": "yeni@example.com"
        }),
    )
    .await;
    assert_eq!(moved.status, 200, "{:?}", moved.body);
    assert_eq!(moved.body["appointment"]["start_at"], slot(4, 16, 30));
    assert_eq!(moved.body["appointment"]["service_id"], "kisisel-antrenman");
    assert_eq!(
        moved.body["appointment"]["service_name"],
        "Kişisel Antrenman"
    );

    // Eski saat serbest kaldı: oraya yeni bir randevu girilebiliyor.
    let neighbour = create(slot(3, 14, 0), "Eski Saati Alan", "+905552220000").await;
    assert!(neighbour["id"].is_string(), "taşınan saat boşalmalı");

    // Dolu bir saate taşınamaz.
    let clash = update(
        &id,
        json!({
            "serviceId": "medical-fitness", "start": to_iso_string(slot(3, 14, 0)),
            "name": "Düzeltilmiş Ad", "phone": "+905559998877"
        }),
    )
    .await;
    assert_eq!(clash.status, 409);
    assert_eq!(
        clash.body["error"]["message"],
        "Bu saat başka bir randevu veya kapalı zamanla çakışıyor."
    );

    // Kendi saatinde bırakmak çakışma sayılmaz (kayıt kendisi hariç tutulur).
    let same = update(
        &id,
        json!({
            "serviceId": "kisisel-antrenman", "start": to_iso_string(slot(4, 16, 30)),
            "name": "Aynı Saat", "phone": "+905559998877"
        }),
    )
    .await;
    assert_eq!(same.status, 200, "{:?}", same.body);

    // Geçmişe taşınamaz.
    let past = update(
        &id,
        json!({
            "serviceId": "kisisel-antrenman", "start": "2020-01-01T07:00:00.000Z",
            "name": "Aynı Saat", "phone": "+905559998877"
        }),
    )
    .await;
    assert_eq!(past.status, 400);
    assert_eq!(past.body["error"]["message"], "Geçmiş bir saate taşınamaz.");

    // Sonuçlanmış kayıtlar düzenlenemez.
    let cancel_id = neighbour["id"].as_str().unwrap();
    server
        .request(
            Method::PATCH,
            &format!("/api/admin/appointments/{cancel_id}"),
            &headers,
            Some(json!({ "action": "cancel", "adminNote": "" })),
        )
        .await;
    let terminal = update(
        cancel_id,
        json!({
            "serviceId": "medical-fitness", "start": to_iso_string(slot(5, 10, 0)),
            "name": "İptal Edilmiş", "phone": "+905552220000"
        }),
    )
    .await;
    assert_eq!(terminal.status, 409);
    assert_eq!(
        terminal.body["error"]["message"],
        "Yalnızca bekleyen, onaylı veya çakışan randevular düzenlenebilir."
    );

    // Olmayan kayıt.
    let missing = update(
        "yok-boyle-bir-id",
        json!({
            "serviceId": "medical-fitness", "start": to_iso_string(slot(5, 10, 0)),
            "name": "Yok", "phone": "+905553330000"
        }),
    )
    .await;
    assert_eq!(missing.status, 404);

    // CSRF başlığı olmadan düzenlenemez.
    let no_csrf = server
        .request(
            Method::PUT,
            &format!("/api/admin/appointments/{id}"),
            &read,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(slot(5, 10, 0)),
                "name": "Yetkisiz", "phone": "+905554440000"
            })),
        )
        .await;
    assert_eq!(no_csrf.status, 403);

    // Doğrulamalar oluşturma ile aynı.
    for (body, message) in [
        (
            json!({ "serviceId": "yok", "start": to_iso_string(slot(5, 10, 0)), "name": "Ad", "phone": "+905550000000" }),
            "Geçerli bir hizmet seçin.",
        ),
        (
            json!({ "serviceId": "medical-fitness", "start": to_iso_string(slot(5, 10, 7)), "name": "Ad", "phone": "+905550000000" }),
            "Saat takvim ızgarasına uymuyor.",
        ),
        (
            json!({ "serviceId": "medical-fitness", "start": to_iso_string(slot(5, 10, 0)), "name": "A", "phone": "+905550000000" }),
            "Ad soyad alanı 2–100 karakter olmalıdır.",
        ),
        (
            json!({ "serviceId": "medical-fitness", "start": to_iso_string(slot(5, 10, 0)), "name": "Ad Soyad", "phone": "123" }),
            "Geçerli bir telefon numarası girin.",
        ),
    ] {
        let response = update(&id, body).await;
        assert_eq!(response.status, 400, "{:?}", response.body);
        assert_eq!(response.body["error"]["message"], message);
    }
}

#[tokio::test]
async fn working_hours_bound_what_clients_are_offered() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let read = [("Cookie", cookie.as_str())];

    // Varsayılan pencere eski sabit davranışla aynı olmalı: her gün 08:00–22:00.
    let defaults = server
        .request(Method::GET, "/api/admin/working-hours", &read, None)
        .await;
    assert_eq!(defaults.status, 200);
    let hours = defaults.body["hours"].as_array().unwrap();
    assert_eq!(hours.len(), 7);
    for (index, entry) in hours.iter().enumerate() {
        assert_eq!(entry["weekday"], index as i64);
        assert_eq!(entry["start_minute"], 480);
        assert_eq!(entry["end_minute"], 1320);
        assert_eq!(entry["closed"], false);
    }

    // Hafta içi bir güne, çalışma penceresi içinde ve dışında birer saat yayınla.
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
    // Hafta sonuna denk gelmeyen bir gün seç; pencere gün bazlı kapatılacak.
    let mut day_offset = 3;
    while civil_from_ms(slot(day_offset, 0) + BOOKING_RULES.offset_ms()).weekday == 0 {
        day_offset += 1;
    }
    let weekday = civil_from_ms(slot(day_offset, 0) + BOOKING_RULES.offset_ms()).weekday as i64;

    for hour in [9, 20] {
        let response = server
            .request(
                Method::PUT,
                "/api/admin/availability-slots",
                &headers,
                Some(json!({
                    "serviceId": "medical-fitness",
                    "start": to_iso_string(slot(day_offset, hour)),
                    "open": true
                })),
            )
            .await;
        assert_eq!(response.status, 200, "{:?}", response.body);
    }

    let offered = async || -> Vec<String> {
        slot_starts(
            &server
                .get("/api/availability?service=medical-fitness")
                .await
                .body,
        )
    };
    let all = offered().await;
    assert!(
        all.contains(&to_iso_string(slot(day_offset, 9))),
        "09:00 sunulmalı"
    );
    assert!(
        all.contains(&to_iso_string(slot(day_offset, 20))),
        "20:00 sunulmalı"
    );

    // Pencereyi daralt: 08:00–18:00. Yayınlanmış 20:00 artık sunulmamalı.
    let mut window: Vec<Value> = (0..7)
        .map(
            |day| json!({ "weekday": day, "startMinute": 480, "endMinute": 1080, "closed": false }),
        )
        .collect();
    let narrowed = server
        .request(
            Method::PUT,
            "/api/admin/working-hours",
            &headers,
            Some(json!({ "hours": window })),
        )
        .await;
    assert_eq!(narrowed.status, 200, "{:?}", narrowed.body);

    let after = offered().await;
    assert!(
        after.contains(&to_iso_string(slot(day_offset, 9))),
        "09:00 hâlâ sunulmalı"
    );
    assert!(
        !after.contains(&to_iso_string(slot(day_offset, 20))),
        "pencere dışındaki yayınlanmış saat sunulmamalı"
    );

    // Günü tamamen kapat: o günün tüm saatleri düşer.
    window[weekday as usize] =
        json!({ "weekday": weekday, "startMinute": 480, "endMinute": 1080, "closed": true });
    server
        .request(
            Method::PUT,
            "/api/admin/working-hours",
            &headers,
            Some(json!({ "hours": window.clone() })),
        )
        .await;
    let closed_day = offered().await;
    assert!(
        !closed_day.contains(&to_iso_string(slot(day_offset, 9))),
        "kapalı günde hiçbir saat sunulmamalı"
    );

    // Pencere yeniden genişletilince saatler geri gelir: slot kayıtları silinmez.
    window[weekday as usize] =
        json!({ "weekday": weekday, "startMinute": 480, "endMinute": 1320, "closed": false });
    server
        .request(
            Method::PUT,
            "/api/admin/working-hours",
            &headers,
            Some(json!({ "hours": window })),
        )
        .await;
    let restored = offered().await;
    assert!(restored.contains(&to_iso_string(slot(day_offset, 9))));
    assert!(
        restored.contains(&to_iso_string(slot(day_offset, 20))),
        "geri gelmeli"
    );

    // Doğrulamalar.
    let seven = |start: i64, end: i64| -> Value {
        json!({ "hours": (0..7).map(|d| json!({ "weekday": d, "startMinute": start, "endMinute": end, "closed": false })).collect::<Vec<_>>() })
    };
    for (body, message) in [
        (json!({ "hours": [] }), "Yedi günün tamamı gönderilmelidir."),
        (
            seven(1080, 480),
            "Çalışma saatleri 30 dakikalık dilimlerle ve başlangıç bitişten önce olmalıdır.",
        ),
        (
            seven(485, 1080),
            "Çalışma saatleri 30 dakikalık dilimlerle ve başlangıç bitişten önce olmalıdır.",
        ),
        (
            seven(480, 1500),
            "Çalışma saatleri 30 dakikalık dilimlerle ve başlangıç bitişten önce olmalıdır.",
        ),
        (
            json!({ "hours": (0..7).map(|_| json!({ "weekday": 0, "startMinute": 480, "endMinute": 1080, "closed": false })).collect::<Vec<_>>() }),
            "Aynı gün birden fazla kez gönderildi.",
        ),
    ] {
        let response = server
            .request(
                Method::PUT,
                "/api/admin/working-hours",
                &headers,
                Some(body),
            )
            .await;
        assert_eq!(response.status, 400, "{:?}", response.body);
        assert_eq!(response.body["error"]["message"], message);
    }

    // CSRF olmadan değiştirilemez.
    let no_csrf = server
        .request(
            Method::PUT,
            "/api/admin/working-hours",
            &read,
            Some(seven(480, 1080)),
        )
        .await;
    assert_eq!(no_csrf.status, 403);
}

#[tokio::test]
async fn admin_can_export_appointments_as_csv_and_ical() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let read = [("Cookie", cookie.as_str())];

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

    // Ayırıcı, tırnak ve satır sonu içeren bir not; kaçırma bunları bozmamalı.
    server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(slot(2, 14)),
                "name": "Şule Öztürk", "phone": "+905551112233",
                "email": "sule@example.com",
                "note": "Sırt; bel ve \"omuz\" ağrısı"
            })),
        )
        .await;
    server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "kisisel-antrenman", "start": to_iso_string(slot(3, 10)),
                "name": "Ahmet Çağlar", "phone": "+905324445566"
            })),
        )
        .await;

    let export = async |query: &str| -> common::ApiResponse {
        server
            .request(
                Method::GET,
                &format!("/api/admin/appointments/export{query}"),
                &read,
                None,
            )
            .await
    };

    // ---- CSV ----
    let csv = export("?format=csv").await;
    assert_eq!(csv.status, 200);
    // BOM baytlar üzerinden sınanır: UTF-8 çözücüler onu metinden kırpar.
    assert_eq!(
        &csv.bytes[..3],
        &[0xEF, 0xBB, 0xBF],
        "Excel'in Türkçe karakterleri okuması için BOM gerekir"
    );
    let text = csv.text.clone();
    let lines: Vec<&str> = text.trim_end().split("\r\n").collect();
    assert_eq!(lines.len(), 3, "başlık + iki kayıt");
    assert!(
        lines[0].contains("Ad soyad;Telefon"),
        "ayırıcı ';' olmalı: {}",
        lines[0]
    );
    assert!(text.contains("Şule Öztürk"), "Türkçe karakterler korunmalı");
    assert!(
        text.contains("\"Sırt; bel ve \"\"omuz\"\" ağrısı\""),
        "ayırıcı ve tırnak içeren alan kaçırılmalı: {text}"
    );
    assert!(text.contains("Onaylandı"), "durum Türkçe yazılmalı");

    // ---- iCal ----
    let ics = export("?format=ics").await;
    assert_eq!(ics.status, 200);
    let calendar = ics.text.clone();
    assert!(calendar.starts_with("BEGIN:VCALENDAR\r\n"));
    assert!(calendar.trim_end().ends_with("END:VCALENDAR"));
    assert_eq!(calendar.matches("BEGIN:VEVENT").count(), 2);
    assert_eq!(
        calendar.matches("STATUS:CONFIRMED").count(),
        2,
        "elle giriş onaylıdır"
    );
    assert!(calendar.contains(&format!("DTSTART:{}", {
        let civil = civil_from_ms(slot(2, 14));
        format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            civil.year, civil.month, civil.day, civil.hour, civil.minute, civil.second
        )
    })));
    // RFC 5545: hiçbir satır 75 okteti aşmamalı.
    for line in calendar.split("\r\n") {
        assert!(
            line.len() <= 75,
            "katlanmamış satır ({} bayt): {line}",
            line.len()
        );
    }
    // İçerik katlanmış satırlara bölünmüş olabilir; RFC 5545'e göre "\r\n " dizisi
    // kaldırılarak geri açılır ve ancak sonra aranır.
    let unfolded = calendar.replace("\r\n ", "");
    assert!(
        unfolded.contains(r#"Sırt\; bel ve "omuz" ağrısı"#),
        "iCal metni kaçırılmalı: {unfolded}"
    );
    assert!(
        unfolded.contains("SUMMARY:Şule Öztürk · Medical Fitness"),
        "başlık korunmalı"
    );
    assert!(unfolded.contains("Telefon: +905551112233"));

    // ---- filtreler dışa aktarmaya da uygulanır ----
    let filtered = export("?format=csv&service=kisisel-antrenman").await;
    assert_eq!(
        filtered.text.trim_end().split("\r\n").count(),
        2,
        "başlık + tek kayıt"
    );
    assert!(filtered.text.contains("Ahmet"));
    assert!(!filtered.text.contains("Şule"));

    let searched = export("?format=csv&q=sule").await;
    assert!(
        searched.text.contains("Şule Öztürk"),
        "arama katlaması burada da geçerli"
    );
    assert!(!searched.text.contains("Ahmet"));

    let empty = export("?format=csv&status=REJECTED").await;
    assert_eq!(
        empty.text.trim_end().split("\r\n").count(),
        1,
        "yalnızca başlık"
    );

    // ---- doğrulama ve yetki ----
    let bad_format = export("?format=pdf").await;
    assert_eq!(bad_format.status, 400);
    assert_eq!(
        bad_format.body["error"]["message"],
        "Geçersiz dışa aktarma biçimi."
    );

    let bad_filter = export("?format=csv&status=BOZUK").await;
    assert_eq!(bad_filter.status, 400);

    let anonymous = server
        .request(
            Method::GET,
            "/api/admin/appointments/export?format=csv",
            &[],
            None,
        )
        .await;
    assert_eq!(anonymous.status, 401, "dışa aktarma oturum ister");
}

#[tokio::test]
async fn admin_can_change_the_password_and_read_the_audit_log() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let read = [("Cookie", cookie.as_str())];

    let change = async |current: &str, next: &str| -> common::ApiResponse {
        server
            .request(
                Method::PUT,
                "/api/admin/password",
                &headers,
                Some(json!({ "current": current, "next": next })),
            )
            .await
    };

    // Doğrulamalar.
    let short = change("test-admin-password", "kisa").await;
    assert_eq!(short.status, 400);
    assert_eq!(
        short.body["error"]["message"],
        "Yeni şifre en az 12 karakter olmalıdır."
    );

    let same = change("test-admin-password", "test-admin-password").await;
    assert_eq!(same.status, 400);
    assert_eq!(
        same.body["error"]["message"],
        "Yeni şifre mevcut şifreyle aynı olamaz."
    );

    let wrong = change("yanlis-sifre-1234", "yeni-guclu-sifre-2026").await;
    assert_eq!(wrong.status, 401);
    assert_eq!(wrong.body["error"]["message"], "Mevcut şifre hatalı.");

    // İkinci bir oturum aç; şifre değişince düşmeli.
    let other = server
        .post(
            "/api/admin/login",
            json!({ "password": "test-admin-password" }),
        )
        .await;
    let other_cookie = other.set_cookie.clone().unwrap();
    let still_valid = server
        .request(
            Method::GET,
            "/api/admin/session",
            &[("Cookie", other_cookie.as_str())],
            None,
        )
        .await;
    assert_eq!(still_valid.status, 200);

    // Değiştir.
    let changed = change("test-admin-password", "yeni-guclu-sifre-2026").await;
    assert_eq!(changed.status, 200, "{:?}", changed.body);
    assert_eq!(
        changed.body["closedSessions"], 1,
        "diğer oturum kapatılmalı"
    );

    // Mevcut oturum korunur, diğeri düşer.
    let mine = server
        .request(Method::GET, "/api/admin/session", &read, None)
        .await;
    assert_eq!(mine.status, 200, "işlemi yapan oturum korunmalı");
    let theirs = server
        .request(
            Method::GET,
            "/api/admin/session",
            &[("Cookie", other_cookie.as_str())],
            None,
        )
        .await;
    assert_eq!(theirs.status, 401, "diğer cihazdaki oturum düşmeli");

    // Eski şifre artık geçmez, yenisi geçer.
    let old_login = server
        .post(
            "/api/admin/login",
            json!({ "password": "test-admin-password" }),
        )
        .await;
    assert_eq!(
        old_login.status, 401,
        "ortam değişkenindeki şifre artık geçersiz"
    );
    let new_login = server
        .post(
            "/api/admin/login",
            json!({ "password": "yeni-guclu-sifre-2026" }),
        )
        .await;
    assert_eq!(new_login.status, 200);

    // İkinci kez değiştirmek de çalışmalı (saklanan özet üzerinden doğrulama).
    let again = change("yeni-guclu-sifre-2026", "ucuncu-sifre-degeri-2026").await;
    assert_eq!(again.status, 200, "{:?}", again.body);

    // ---- işlem kaydı ----
    let audit = server
        .request(Method::GET, "/api/admin/audit", &read, None)
        .await;
    assert_eq!(audit.status, 200);
    let entries = audit.body["entries"].as_array().unwrap();
    let actions: Vec<&str> = entries
        .iter()
        .map(|entry| entry["action"].as_str().unwrap())
        .collect();

    for expected in [
        "auth.login",
        "auth.login.failed",
        "auth.password.changed",
        "auth.password.failed",
    ] {
        assert!(
            actions.contains(&expected),
            "{expected} kaydı bekleniyordu: {actions:?}"
        );
    }
    assert_eq!(
        actions
            .iter()
            .filter(|a| **a == "auth.password.changed")
            .count(),
        2,
        "iki şifre değişikliği kaydedilmeli"
    );
    // En yeni kayıt başta olmalı.
    let times: Vec<i64> = entries
        .iter()
        .map(|entry| entry["at"].as_i64().unwrap())
        .collect();
    assert!(
        times.windows(2).all(|pair| pair[0] >= pair[1]),
        "kayıtlar yeniden eskiye sıralı"
    );

    // Randevu ve kapalı zaman işlemleri de kaydedilir.
    // Şifre bu noktada iki kez değiştiği için giriş güncel değerle yapılır.
    let fresh = server
        .post(
            "/api/admin/login",
            json!({ "password": "ucuncu-sifre-degeri-2026" }),
        )
        .await;
    assert_eq!(fresh.status, 200, "{:?}", fresh.body);
    let cookie = fresh.set_cookie.clone().unwrap();
    let csrf = fresh.body["csrfToken"].as_str().unwrap().to_string();
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let read = [("Cookie", cookie.as_str())];
    let now = now_ms();
    let local = civil_from_ms(now + BOOKING_RULES.offset_ms());
    let start = utc_ms_hm(
        local.year,
        local.month as i64 - 1,
        local.day as i64 + 3,
        11,
        0,
    ) - BOOKING_RULES.offset_ms();
    server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(start),
                "name": "Kayıt Testi", "phone": "+905551110000"
            })),
        )
        .await;
    server
        .request(
            Method::POST,
            "/api/admin/blocks",
            &headers,
            Some(json!({
                "start": to_iso_string(start + 4 * 3_600_000),
                "end": to_iso_string(start + 5 * 3_600_000),
                "reason": "Kayıt testi"
            })),
        )
        .await;

    let after = server
        .request(Method::GET, "/api/admin/audit?limit=200", &read, None)
        .await;
    let actions: Vec<&str> = after.body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"appointment.create"));
    assert!(actions.contains(&"block.create"));

    let created = after.body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["action"] == "appointment.create")
        .unwrap();
    assert!(
        created["detail"].as_str().unwrap().contains("Kayıt Testi"),
        "ayrıntı kimin randevusu olduğunu söylemeli: {created:?}"
    );

    // Sayfalama.
    let paged = server
        .request(
            Method::GET,
            "/api/admin/audit?limit=2&offset=0",
            &read,
            None,
        )
        .await;
    assert_eq!(paged.body["entries"].as_array().unwrap().len(), 2);
    assert!(paged.body["total"].as_i64().unwrap() > 2);

    // Oturumsuz erişilemez.
    let anonymous = server
        .request(Method::GET, "/api/admin/audit", &[], None)
        .await;
    assert_eq!(anonymous.status, 401);
}

/// Bir seans 90 dakika sürdüğü için, 10:00'da onaylanan antrenman 10:30 ve
/// 11:00'i de kapatır. Süre 20 dakikayken bu saatler açık kalıyordu; kural
/// değiştiğinde çakışma denetiminin de değiştiğini burada sabitliyoruz.
#[tokio::test]
async fn a_session_blocks_the_grid_slots_it_covers() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    let local_now = cemox_server::time::civil_from_ms(now_ms() + BOOKING_RULES.offset_ms());
    let at = |hour: i64, minute: i64| -> i64 {
        cemox_server::time::utc_ms_hm(
            local_now.year,
            local_now.month as i64 - 1,
            local_now.day as i64 + 3,
            hour,
            minute,
        ) - BOOKING_RULES.offset_ms()
    };

    // Izgaranın üç ardışık saatini aç: 10:00, 10:30, 11:00.
    for (hour, minute) in [(10, 0), (10, 30), (11, 0)] {
        let opened = server
            .request(
                Method::PUT,
                "/api/admin/availability-slots",
                &headers,
                Some(json!({
                    "serviceId": "medical-fitness",
                    "start": to_iso_string(at(hour, minute)),
                    "open": true
                })),
            )
            .await;
        assert_eq!(opened.status, 200, "{:?}", opened.body);
    }

    // Test ortamı başka günlere de saat açtığı için yalnızca hedef gün sayılır.
    let day = to_iso_string(at(10, 0))[..10].to_string();
    let on_day = |response: &common::ApiResponse| -> Vec<String> {
        slot_starts(&response.body)
            .into_iter()
            .filter(|start| start.starts_with(&day))
            .collect()
    };

    let offered = server
        .get("/api/availability?service=medical-fitness")
        .await;
    assert_eq!(
        on_day(&offered).len(),
        3,
        "üç saat de açık sunulmalı: {:?}",
        on_day(&offered)
    );

    // 10:00 seansı onaylanır.
    let booked = server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness", "start": to_iso_string(at(10, 0)),
                "name": "Seans Sahibi", "phone": "+905551112233"
            })),
        )
        .await;
    assert_eq!(booked.status, 201, "{:?}", booked.body);

    // 10:00–11:30 dolu olduğu için üçü de kapanmalı.
    let after = server
        .get("/api/availability?service=medical-fitness")
        .await;
    assert!(
        on_day(&after).is_empty(),
        "90 dakikalık seans kapsadığı saatleri kapatmalı, açık kalan: {:?}",
        on_day(&after)
    );
}

/// Seansın tamamı çalışma penceresine sığmalı. Pencere 18:00'de kapanıyorsa
/// 17:00 sunulmaz: 90 dakikalık seans 18:30'a taşardı.
#[tokio::test]
async fn a_session_that_overruns_closing_time_is_not_offered() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    // Her gün 08:00–18:00.
    let hours: Vec<Value> = (0..7)
        .map(|weekday| {
            json!({ "weekday": weekday, "startMinute": 8 * 60, "endMinute": 18 * 60, "closed": false })
        })
        .collect();
    let saved = server
        .request(
            Method::PUT,
            "/api/admin/working-hours",
            &headers,
            Some(json!({ "hours": hours })),
        )
        .await;
    assert_eq!(saved.status, 200, "{:?}", saved.body);

    let local_now = cemox_server::time::civil_from_ms(now_ms() + BOOKING_RULES.offset_ms());
    let at = |hour: i64| -> i64 {
        cemox_server::time::utc_ms_hm(
            local_now.year,
            local_now.month as i64 - 1,
            local_now.day as i64 + 3,
            hour,
            0,
        ) - BOOKING_RULES.offset_ms()
    };

    // 16:00 tam sığar (17:30 biter), 17:00 taşar (18:30 biterdi).
    for hour in [16, 17] {
        let opened = server
            .request(
                Method::PUT,
                "/api/admin/availability-slots",
                &headers,
                Some(json!({
                    "serviceId": "medical-fitness",
                    "start": to_iso_string(at(hour)),
                    "open": true
                })),
            )
            .await;
        assert_eq!(opened.status, 200, "{:?}", opened.body);
    }

    let day = to_iso_string(at(16))[..10].to_string();
    let offered = server
        .get("/api/availability?service=medical-fitness")
        .await;
    let starts: Vec<String> = slot_starts(&offered.body)
        .into_iter()
        .filter(|start| start.starts_with(&day))
        .collect();

    assert!(
        starts.iter().any(|start| start == &to_iso_string(at(16))),
        "16:00 sunulmalı: {starts:?}"
    );
    assert!(
        !starts.iter().any(|start| start == &to_iso_string(at(17))),
        "17:00 kapanışı taştığı için sunulmamalı: {starts:?}"
    );
}

/// `X-Forwarded-For` başlığına koşulsuz güvenmek, oran sınırlarının tamamını
/// tek bir başlıkla aşılabilir kılar: saldırgan her istekte farklı bir değer
/// yazarak giriş denemesi sınırını kaldırabilir ve işlem kaydına istediği
/// IP'yi yazdırabilir. Güvenilen vekil sayısı sıfırken başlık yok sayılmalı.
#[tokio::test]
async fn a_forged_forwarded_header_cannot_lift_the_login_rate_limit() {
    let server = common::start().await;
    let mut codes = Vec::new();
    for attempt in 0..8 {
        let forged = format!("203.0.113.{attempt}");
        let response = server
            .request(
                Method::POST,
                "/api/admin/login",
                &[("X-Forwarded-For", forged.as_str())],
                Some(json!({ "password": "kesinlikle-yanlis-sifre" })),
            )
            .await;
        codes.push(response.status);
    }
    assert!(
        codes.contains(&429),
        "sahte başlıkla sınır aşılmamalı, dönen kodlar: {codes:?}"
    );
}

/// Elektronik tablolar `=`, `+`, `-`, `@` ile başlayan hücreleri hesaplar.
/// Ad alanına formül yazan bir danışan, dosyayı açan antrenörün makinesinde
/// onu çalıştırır; CSV tırnağı bunu engellemez.
#[tokio::test]
async fn exported_cells_cannot_start_a_spreadsheet_formula() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;
    let headers = [("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];

    let local_now = cemox_server::time::civil_from_ms(now_ms() + BOOKING_RULES.offset_ms());
    let start_at = cemox_server::time::utc_ms_hm(
        local_now.year,
        local_now.month as i64 - 1,
        local_now.day as i64 + 3,
        11,
        0,
    ) - BOOKING_RULES.offset_ms();

    let created = server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &headers,
            Some(json!({
                "serviceId": "medical-fitness",
                "start": to_iso_string(start_at),
                "name": "=HYPERLINK(\"http://kotu.example\")",
                "phone": "+905551112233",
                "note": "@SUM(A1:A9)"
            })),
        )
        .await;
    assert_eq!(created.status, 201, "{:?}", created.body);

    let export = server
        .request(
            Method::GET,
            "/api/admin/appointments/export?format=csv",
            &[("Cookie", cookie.as_str())],
            None,
        )
        .await;
    assert_eq!(export.status, 200);
    let csv = String::from_utf8_lossy(&export.bytes).to_string();

    for line in csv.lines().skip(1) {
        for cell in line.split(';') {
            let value = cell.trim_matches('"');
            assert!(
                !value.starts_with(['=', '+', '-', '@']),
                "hücre formül olarak başlıyor: {value:?}\nsatır: {line}"
            );
        }
    }
    assert!(
        csv.contains("'=HYPERLINK") && csv.contains("'+905551112233") && csv.contains("'@SUM"),
        "tehlikeli hücreler metin olarak işaretlenmeli:\n{csv}"
    );
}

/// Eksik başlık zaten reddediliyordu; asıl karşılaştırmayı sınayan durum
/// **yanlış** bir jetonun gönderilmesidir. Karşılaştırma sabit zamanlı olduğu
/// için doğru önekli bir tahmin de tamamen farklı biri de aynı yanıtı alır.
#[tokio::test]
async fn a_wrong_csrf_token_is_refused_whatever_its_prefix() {
    let server = common::start().await;
    let (cookie, csrf) = server.login().await;

    let local_now = cemox_server::time::civil_from_ms(now_ms() + BOOKING_RULES.offset_ms());
    let start_at = cemox_server::time::utc_ms_hm(
        local_now.year,
        local_now.month as i64 - 1,
        local_now.day as i64 + 3,
        11,
        0,
    ) - BOOKING_RULES.offset_ms();

    // Doğru jetonun önekini taşıyan tahmin, hiç benzemeyen jeton, ve boş jeton.
    let near_miss = format!("{}x", &csrf[..csrf.len() - 1]);
    for forged in [near_miss.as_str(), "tamamen-baska-bir-jeton", ""] {
        let refused = server
            .request(
                Method::POST,
                "/api/admin/appointments",
                &[("Cookie", cookie.as_str()), ("x-csrf-token", forged)],
                Some(json!({
                    "serviceId": "medical-fitness",
                    "start": to_iso_string(start_at),
                    "name": "Sahte Jeton",
                    "phone": "+905551112233"
                })),
            )
            .await;
        assert_eq!(
            refused.status, 403,
            "yanlış jeton reddedilmeli: {forged:?} → {:?}",
            refused.body
        );
        assert_eq!(refused.body["error"]["code"], "CSRF_ERROR");
    }

    // Doğru jetonla aynı istek geçmeli; test yalnızca reddi değil kabulü de bağlar.
    let accepted = server
        .request(
            Method::POST,
            "/api/admin/appointments",
            &[("Cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())],
            Some(json!({
                "serviceId": "medical-fitness",
                "start": to_iso_string(start_at),
                "name": "Gerçek Jeton",
                "phone": "+905551112233"
            })),
        )
        .await;
    assert_eq!(accepted.status, 201, "{:?}", accepted.body);
}
