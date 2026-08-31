//! `src/email.js` karşılığı. SMTP yapılandırılmamışsa e-postalar maskelenmiş
//! biçimde loglanır — geliştirme ortamında randevu akışı e-postasız da çalışır.

use std::sync::Arc;

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::Config;
use crate::db::Appointment;
use crate::time::format_date_time_long;

#[derive(Clone)]
pub struct EmailService {
    inner: Arc<Inner>,
}

struct Inner {
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: String,
    admin_email: String,
    app_origin: String,
}

impl EmailService {
    pub fn new(config: &Config) -> Self {
        let smtp = &config.smtp;
        let enabled = !smtp.host.is_empty() && !smtp.user.is_empty() && !smtp.pass.is_empty();

        let transport = if enabled {
            let builder = if smtp.secure {
                AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)
            } else {
                // Node `secure: false` + 587 = STARTTLS yükseltmesi.
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)
            };
            match builder {
                Ok(builder) => Some(
                    builder
                        .port(smtp.port)
                        .credentials(Credentials::new(smtp.user.clone(), smtp.pass.clone()))
                        .build(),
                ),
                Err(error) => {
                    tracing::error!(%error, "SMTP taşıyıcısı kurulamadı; e-postalar loglanacak");
                    None
                }
            }
        } else {
            None
        };

        Self {
            inner: Arc::new(Inner {
                transport,
                from: smtp.from.clone(),
                admin_email: config.admin_email.clone(),
                app_origin: config.app_origin.clone(),
            }),
        }
    }

    /// Testlerde kullanılan, hiçbir şey göndermeyen sürüm.
    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(Inner {
                transport: None,
                from: "Cem Avat Randevu <randevu@localhost>".into(),
                admin_email: "admin@example.com".into(),
                app_origin: String::new(),
            }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.transport.is_some()
    }

    async fn send(&self, to: &str, subject: &str, html: String) {
        let Some(transport) = &self.inner.transport else {
            tracing::info!("[email disabled] {subject} -> {}", mask_email(to));
            return;
        };

        let message = Message::builder()
            .from(match self.inner.from.parse() {
                Ok(mailbox) => mailbox,
                Err(error) => {
                    tracing::error!(%error, "SMTP_FROM ayrıştırılamadı");
                    return;
                }
            })
            .to(match to.parse() {
                Ok(mailbox) => mailbox,
                Err(error) => {
                    tracing::error!(%error, "alıcı adresi ayrıştırılamadı");
                    return;
                }
            })
            .subject(subject)
            .header(ContentType::TEXT_HTML)
            .body(html);

        match message {
            Ok(message) => {
                if let Err(error) = transport.send(message).await {
                    tracing::error!(%error, subject, "e-posta gönderilemedi");
                }
            }
            Err(error) => tracing::error!(%error, "e-posta oluşturulamadı"),
        }
    }

    pub async fn request_received(&self, appointment: &Appointment) {
        let when = format_date_time_long(appointment.start_at);
        let to_client = self.send(
            &appointment.email,
            "Randevu seçiminiz alındı",
            format!(
                "<p>Merhaba {name},</p><p><strong>{service}</strong> için <strong>{when}</strong> tarihini seçtiniz.</p>\
                 <p>Bu saat 24 saat boyunca sizin için tutulacak ve antrenör onayından sonra kesinleşecektir.</p>\
                 <p>Randevu numarası: {id}</p>",
                name = escape_html(&appointment.name),
                service = escape_html(&appointment.service_name),
                id = escape_html(&appointment.id),
            ),
        );
        let admin_subject = format!("Onay bekleyen randevu — {}", appointment.service_name);
        let to_admin = self.send(
            &self.inner.admin_email,
            &admin_subject,
            format!(
                "<p><strong>{name}</strong>, {when} saatini seçti.</p>\
                 <p>Telefon: {phone}<br>E-posta: {email}<br>Not: {note}</p>\
                 <p><a href=\"{origin}/admin\">Randevuyu onaylayın veya reddedin</a></p>",
                name = escape_html(&appointment.name),
                phone = escape_html(&appointment.phone),
                email = escape_html(&appointment.email),
                note = escape_html(if appointment.note.is_empty() {
                    "—"
                } else {
                    &appointment.note
                }),
                origin = self.inner.app_origin,
            ),
        );
        // `Promise.allSettled` karşılığı: biri başarısız olsa da diğeri denenir.
        tokio::join!(to_client, to_admin);
    }

    pub async fn appointment_approved(&self, appointment: &Appointment) {
        self.send(
            &appointment.email,
            "Telefon ön görüşmeniz onaylandı",
            format!(
                "<p>Merhaba {name},</p><p><strong>{service}</strong> için telefon ön görüşmeniz <strong>{when}</strong> tarihine onaylandı.</p>\
                 <p>Cem Avat randevu saatinde kayıt sırasında verdiğiniz telefon numarasından sizi arayacaktır.</p>\
                 <p>İptal veya değişiklik için en geç 12 saat öncesinde <a href=\"tel:+905512327468\">telefon</a> ya da <a href=\"https://wa.me/905512327468\">WhatsApp</a> üzerinden iletişime geçin.</p>",
                name = escape_html(&appointment.name),
                service = escape_html(&appointment.service_name),
                when = format_date_time_long(appointment.start_at),
            ),
        )
        .await;
    }

    pub async fn appointment_rejected(&self, appointment: &Appointment) {
        let note = if appointment.admin_note.is_empty() {
            String::new()
        } else {
            format!("<p>Not: {}</p>", escape_html(&appointment.admin_note))
        };
        self.send(
            &appointment.email,
            "Randevu talebiniz sonuçlandı",
            format!(
                "<p>Merhaba {name},</p><p><strong>{service}</strong> için oluşturduğunuz randevu talebi uygunluk nedeniyle onaylanamadı.</p>{note}\
                 <p>Web sitesinden farklı bir saat için yeniden talep oluşturabilirsiniz.</p>",
                name = escape_html(&appointment.name),
                service = escape_html(&appointment.service_name),
            ),
        )
        .await;
    }

    pub async fn appointment_cancelled(&self, appointment: &Appointment) {
        self.send(
            &appointment.email,
            "Randevunuz iptal edildi",
            format!(
                "<p>Merhaba {name},</p><p><strong>{when}</strong> tarihli telefon ön görüşmeniz iptal edildi.</p>\
                 <p>{note}</p><p>Yeni bir saat için web sitesinden tekrar talep oluşturabilirsiniz.</p>",
                name = escape_html(&appointment.name),
                when = format_date_time_long(appointment.start_at),
                note = escape_html(&appointment.admin_note),
            ),
        )
        .await;
    }

    pub async fn appointment_expired(&self, appointment: &Appointment) {
        self.send(
            &appointment.email,
            "Randevu talebinizin süresi doldu",
            format!(
                "<p>Merhaba {name},</p><p><strong>{service}</strong> için oluşturduğunuz talep 24 saat içinde sonuçlandırılamadığı için süresi doldu ve seçilen saat yeniden müsait hale geldi.</p>\
                 <p>Web sitesinden yeni bir talep oluşturabilirsiniz.</p>",
                name = escape_html(&appointment.name),
                service = escape_html(&appointment.service_name),
            ),
        )
        .await;
    }
}

fn escape_html(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '\'' => output.push_str("&#39;"),
            '"' => output.push_str("&quot;"),
            other => output.push(other),
        }
    }
    output
}

fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((name, domain)) => {
            let prefix: String = name.chars().take(2).collect();
            format!("{prefix}***@{domain}")
        }
        None => "***".into(),
    }
}
