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
            "Slot 30 dakikalık takvime uygun değil.",
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
