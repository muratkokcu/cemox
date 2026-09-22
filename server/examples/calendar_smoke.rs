//! Google Takvim kurulumunu gerçek Google'a karşı doğrular: jeton alır,
//! bir deneme etkinliği oluşturur, taşır ve siler. Takvimde kalıcı iz bırakmaz.
//!
//! Uygulamayı çalıştırmadan önce "anahtar doğru mu, takvim paylaşıldı mı"
//! sorularını ayrı ayrı yanıtlar; panelde çıkan tek satırlık hataya bakmaktan
//! daha çabuk teşhis verdiği için örnek olarak duruyor (ağ gerektirir).
//!
//! Kullanım:
//!   GOOGLE_SERVICE_ACCOUNT=anahtar.json \
//!     cargo run --manifest-path server/Cargo.toml \
//!     --example calendar_smoke -- takvim@gmail.com

use std::collections::HashMap;

use cemox_server::calendar::{EventState, GoogleCalendar};
use cemox_server::db::Appointment;

#[tokio::main]
async fn main() {
    let calendar_id = match std::env::args().nth(1) {
        Some(value) => value,
        None => {
            eprintln!("Kullanım: ... --example calendar_smoke -- <takvim-kimliği>");
            eprintln!("Takvim kimliği genelde antrenörün Google hesabının e-postasıdır.");
            std::process::exit(2);
        }
    };

    let env: HashMap<String, String> = std::env::vars().collect();
    let Some(calendar) = GoogleCalendar::from_env(&env) else {
        eprintln!("✗ Servis hesabı okunamadı.");
        eprintln!(
            "  GOOGLE_SERVICE_ACCOUNT tanımlı mı? JSON dosyasının yolu ya da JSON'un kendisi olmalı."
        );
        std::process::exit(1);
    };
    println!(
        "✓ Anahtar okundu — servis hesabı: {}",
        calendar.client_email()
    );
    println!("  Takvim bu adresle paylaşılmış olmalı.\n");

    // Kesin geçmişte bir saat: yanlışlıkla gerçek bir randevunun üstüne yazmasın.
    let start = 1_700_000_000;
    let appointment = Appointment {
        id: "smoke".into(),
        service_id: "smoke".into(),
        service_name: "Kurulum denemesi".into(),
        start_at: start,
        end_at: start + 1800,
        name: "Cemox".into(),
        email: String::new(),
        phone: "—".into(),
        note: "Bu etkinlik kurulum denemesidir, birazdan silinecek.".into(),
        status: "APPROVED".into(),
        hold_expires_at: 0,
        created_at: start,
        decision_at: None,
        admin_note: String::new(),
        calendar_event_id: String::new(),
        calendar_synced: false,
    };

    let event_id = match calendar.create_event(&calendar_id, &appointment).await {
        Ok(id) => {
            println!("✓ Etkinlik oluşturuldu — kimlik {id}");
            id
        }
        Err(error) => {
            eprintln!("✗ Etkinlik oluşturulamadı: {error}");
            explain(&error.to_string());
            std::process::exit(1);
        }
    };

    let moved = Appointment {
        start_at: start + 3600,
        end_at: start + 5400,
        ..appointment
    };
    match calendar.update_event(&calendar_id, &event_id, &moved).await {
        Ok(EventState::Live) => println!("✓ Etkinlik güncellendi"),
        Ok(EventState::Gone) => {
            eprintln!("✗ Etkinlik güncellenirken kaybolmuş görünüyor.");
            eprintln!("  Takvimden silinmiş olabilir; uygulama bu durumda yenisini oluşturur.");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("✗ Güncellenemedi: {error}");
            eprintln!(
                "  Paylaşım izni \"Etkinliklerde değişiklik yap\" değil, salt okunur olabilir."
            );
            std::process::exit(1);
        }
    }

    match calendar.delete_event(&calendar_id, &event_id).await {
        Ok(()) => println!("✓ Etkinlik silindi — takvimde iz kalmadı"),
        Err(error) => {
            eprintln!("✗ Silinemedi: {error}");
            eprintln!("  Deneme etkinliğini takvimden elle silmen gerekebilir ({start}).");
            std::process::exit(1);
        }
    }

    println!("\nKurulum çalışıyor. Panelde Güvenlik → Takvim'den senkronu açabilirsin.");
}

/// Google'ın hata metnini yapılacak işe çevirir.
fn explain(message: &str) {
    if message.contains("404") || message.to_lowercase().contains("not found") {
        eprintln!("  Takvim bulunamadı. İki olasılık:");
        eprintln!("  • Takvim kimliği yanlış yazılmış olabilir.");
        eprintln!("  • Takvim servis hesabıyla henüz paylaşılmamış olabilir —");
        eprintln!("    servis hesapları paylaşılmayan takvimi \"yok\" olarak görür.");
    } else if message.contains("403") {
        eprintln!("  Erişim reddedildi. Calendar API projede etkin mi?");
        eprintln!("  Paylaşım izni \"Etkinliklerde değişiklik yap\" mı?");
    } else if message.contains("401") || message.to_lowercase().contains("invalid_grant") {
        eprintln!("  Kimlik doğrulanamadı. Anahtar iptal edilmiş ya da bozuk olabilir.");
        eprintln!("  Sunucunun saati de doğru olmalı; JWT zaman damgası kaymışsa reddedilir.");
    }
}
