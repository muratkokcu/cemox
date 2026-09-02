//! `server.js` karşılığı: router, middleware zinciri, doğrulama ve müsaitlik hesabı.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tower::ServiceBuilder;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::config::{BOOKING_RULES, Config, SERVICES, app_root, service_name};
use crate::db::{AdminSession, AppointmentQuery, Db, NewAppointment, SlotChange, WorkingHours};
use crate::email::EmailService;
use crate::error::AppError;
use crate::export::{csv_export, ics_export};
use crate::time::{
    DAY_MS, civil_from_ms, format_date_time_long, format_day, format_time, iso_date, now_ms,
    parse_timestamp, to_iso_string, utc_ms,
};

const SESSION_TTL_MS: i64 = 12 * 60 * 60 * 1000;
const SESSION_COOKIE: &str = "cemox_admin";
/// `loadConfig` ile aynı alt sınır.
const MIN_PASSWORD_LENGTH: usize = 12;
/// İşlem kaydı bu süreden eskiyse bakım sırasında silinir.
const AUDIT_RETENTION_DAYS: i64 = 180;
/// Elle oluşturulan kayıtlarda kaynağı belli etmek için yönetici notuna yazılır.
const MANUAL_ENTRY_NOTE: &str = "Panelden elle oluşturuldu (telefon).";

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub email: EmailService,
    limiters: Arc<Limiters>,
}

struct Limiters {
    public: RateLimiter,
    booking: RateLimiter,
    login: RateLimiter,
}

/// Yönetici oturumu; `require_admin` tarafından istek uzantılarına eklenir.
#[derive(Clone)]
struct AdminContext {
    token: String,
    session: AdminSession,
}

impl AppState {
    pub fn new(config: Config, db: Db, email: EmailService) -> Self {
        Self {
            config: Arc::new(config),
            db,
            email,
            limiters: Arc::new(Limiters {
                public: RateLimiter::new(15 * 60 * 1000, 120),
                booking: RateLimiter::new(60 * 60 * 1000, 8),
                login: RateLimiter::new(15 * 60 * 1000, 5),
            }),
        }
    }

    /// `runMaintenance` karşılığı: süresi dolan talepleri kapatır ve bildirim gönderir.
    pub async fn run_maintenance(&self, now: i64) -> Result<usize, AppError> {
        // İşlem kaydı sınırsız büyümesin.
        let pruner = self.db.clone();
        let cutoff = now - AUDIT_RETENTION_DAYS * DAY_MS;
        if let Err(error) = blocking(move || pruner.prune_audit(cutoff)).await {
            tracing::error!(%error, "işlem kaydı budanamadı");
        }

        let db = self.db.clone();
        let expired = blocking(move || db.expire_pending(now)).await?;
        for appointment in &expired {
            self.email.appointment_expired(appointment).await;
        }
        Ok(expired.len())
    }
}

impl AppState {
    /// İşlem kaydına satır yazar. Hatası yutulur: kayıt tutulamaması asıl
    /// işlemi geçersiz kılmamalıdır.
    pub async fn audit(&self, action: &'static str, detail: &str, ip: &str) {
        let db = self.db.clone();
        let detail = detail.to_string();
        let ip = ip.to_string();
        let written = blocking(move || db.record_audit(action, &detail, &ip, now_ms())).await;
        if let Err(error) = written {
            tracing::error!(%error, action, "işlem kaydı yazılamadı");
        }
    }
}

/// Gövdeyi okur; `Request` alan handler'lar `Bytes` çıkarıcısını kullanamaz.
async fn read_body(request: Request) -> Result<Bytes, AppError> {
    axum::body::to_bytes(request.into_body(), 256 * 1024)
        .await
        .map_err(|_| AppError::validation("İstek gövdesi okunamadı."))
}

/// SQLite çağrıları senkron olduğu için tokio çalışan iş parçacıklarını bloklamamak
/// üzere `spawn_blocking` üzerinden yürütülür.
async fn blocking<T, F>(task: F) -> Result<T, AppError>
where
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| AppError::internal(format!("görev yürütülemedi: {error}")))?
}

// ---- router ---------------------------------------------------------------

pub fn build_router(state: AppState) -> Router {
    let admin = Router::new()
        .route("/api/admin/session", get(admin_session))
        .route("/api/admin/logout", post(admin_logout))
        .route(
            "/api/admin/appointments",
            get(admin_list_appointments).post(admin_create_appointment),
        )
        .route(
            "/api/admin/appointments/export",
            get(admin_export_appointments),
        )
        .route(
            "/api/admin/appointments/{id}",
            axum::routing::patch(admin_decide_appointment).put(admin_update_appointment),
        )
        .route(
            "/api/admin/blocks",
            get(admin_list_blocks).post(admin_create_block),
        )
        .route(
            "/api/admin/blocks/{id}",
            axum::routing::delete(admin_delete_block),
        )
        .route(
            "/api/admin/availability-slots",
            get(admin_list_slots).put(admin_set_slot),
        )
        .route(
            "/api/admin/password",
            axum::routing::put(admin_change_password),
        )
        .route("/api/admin/audit", get(admin_list_audit))
        .route(
            "/api/admin/working-hours",
            get(admin_list_working_hours).put(admin_set_working_hours),
        )
        .route(
            "/api/admin/availability-slots/bulk",
            // Bir aylık takvim tek istekte gelebildiği için genel 24kb sınırı yetmez.
            axum::routing::put(admin_set_slots_bulk).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .layer(middleware::from_fn_with_state(state.clone(), require_admin));

    let login = Router::new()
        .route("/api/admin/login", post(admin_login))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_same_origin,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            login_rate_limit,
        ));

    let booking = Router::new()
        .route("/api/appointments", post(create_appointment))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_same_origin,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            booking_rate_limit,
        ));

    let api = Router::new()
        .route("/api/services", get(list_services))
        .route("/api/availability", get(availability))
        .merge(booking)
        .merge(login)
        .merge(admin)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            public_rate_limit,
        ));

    Router::new()
        .route("/health", get(|| async { Json(json!({ "ok": true })) }))
        .merge(api)
        .merge(static_routes(&state))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(24 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers,
        ))
        .with_state(state)
}

/// `assets/`, `galery/` ve derlenmiş SPA. `dist/` yoksa SPA yolları hiç tanımlanmaz.
fn static_routes(state: &AppState) -> Router<AppState> {
    let root = app_root();
    let dist = root.join("dist");
    let production = state.config.production;

    let root_cache = if production {
        "public, max-age=2592000, immutable"
    } else {
        "no-store"
    };
    let dist_cache = if production {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };

    // İç katman dist varlıklarına uzun ömür verir; dış katman yalnızca başlık
    // yoksa kök `assets/` politikasını uygular.
    let dist_assets = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static(dist_cache),
        ))
        .service(ServeDir::new(dist.join("assets")));

    let assets = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static(root_cache),
        ))
        .service(ServeDir::new(root.join("assets")).fallback(dist_assets));

    let galery = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static(root_cache),
        ))
        .service(ServeDir::new(root.join("galery")));

    let mut router = Router::new()
        .nest_service("/assets", assets)
        .nest_service("/galery", galery);

    if dist.join("index.html").exists() {
        router = router
            .route("/", get(send_web_app))
            .route("/admin", get(send_web_app));
    }
    router
}

async fn send_web_app() -> Response {
    let index = app_root().join("dist").join("index.html");
    match tokio::fs::read(&index).await {
        Ok(bytes) => (
            [
                (header::CACHE_CONTROL, "no-cache"),
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn not_found(request: Request) -> Response {
    if request.uri().path().starts_with("/api") {
        return AppError::new(StatusCode::NOT_FOUND, "NOT_FOUND", "API yolu bulunamadı.")
            .into_response();
    }
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

// ---- middleware -----------------------------------------------------------

async fn security_headers(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
             font-src https://fonts.gstatic.com; script-src 'self' 'unsafe-inline'; connect-src 'self'; \
             frame-ancestors 'self'; base-uri 'self'; form-action 'self'",
        ),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    if state.config.production {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    response
}

async fn require_same_origin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        && origin != state.config.app_origin
    {
        return AppError::forbidden("ORIGIN_ERROR", "İstek kaynağına izin verilmiyor.")
            .into_response();
    }
    next.run(request).await
}

/// Oturumu doğrular ve GET dışı her istekte CSRF başlığını arar.
/// `server.js` içinde `requireCsrf` tam olarak GET olmayan yönetici uçlarına takılıydı;
/// yönteme bağlamak yeni uçlarda da güvenli varsayılanı korur.
async fn require_admin(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let cookies = parse_cookies(
        request
            .headers()
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or(""),
    );
    let token = cookies.get(SESSION_COOKIE).cloned().unwrap_or_default();

    let db = state.db.clone();
    let lookup_token = token.clone();
    let session = match blocking(move || db.get_session(&lookup_token, now_ms())).await {
        Ok(Some(session)) => session,
        Ok(None) => return AppError::unauthorized("Oturum açmanız gerekiyor.").into_response(),
        Err(error) => return error.into_response(),
    };

    if request.method() != Method::GET {
        let provided = request
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if provided != session.csrf_token {
            return AppError::forbidden("CSRF_ERROR", "Güvenlik doğrulaması başarısız.")
                .into_response();
        }
    }

    request
        .extensions_mut()
        .insert(AdminContext { token, session });
    next.run(request).await
}

async fn public_rate_limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    rate_limit(&state.limiters.public, &state, request, next).await
}

async fn booking_rate_limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    rate_limit(&state.limiters.booking, &state, request, next).await
}

async fn login_rate_limit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    rate_limit(&state.limiters.login, &state, request, next).await
}

async fn rate_limit(
    limiter: &RateLimiter,
    state: &AppState,
    request: Request,
    next: Next,
) -> Response {
    let key = format!(
        "{}:{}",
        client_ip(&request, state.config.production),
        request.uri().path()
    );
    if !limiter.check(&key, now_ms()) {
        return AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
            "Çok fazla istek gönderildi. Lütfen daha sonra tekrar deneyin.",
        )
        .into_response();
    }
    next.run(request).await
}

/// Üretimde tek katman ters vekil varsayılır (`trust proxy = 1`).
fn client_ip(request: &Request, production: bool) -> String {
    if production
        && let Some(forwarded) = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        && let Some(first) = forwarded.split(',').next()
    {
        let first = first.trim();
        if !first.is_empty() {
            return first.to_string();
        }
    }
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip().to_string())
        .unwrap_or_else(|| "unknown".into())
}

// ---- genel uçlar ----------------------------------------------------------

async fn list_services() -> Json<Value> {
    let services: Vec<Value> = SERVICES
        .iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    Json(json!({ "services": services }))
}

async fn availability(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, AppError> {
    let service_id = params.get("service").cloned().unwrap_or_default();
    assert_service(&service_id)?;
    let db = state.db.clone();
    let result = blocking(move || build_availability(&db, &service_id, now_ms())).await?;
    Ok(Json(result))
}

async fn create_appointment(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, AppError> {
    let payload = parse_json(&body)?;
    let input = validate_booking_payload(&payload)?;

    let db = state.db.clone();
    let appointment = blocking(move || {
        let now = now_ms();
        assert_slot_offered(&db, &input.service_id, input.start_at, now)?;
        db.create_appointment(&input, now)
    })
    .await?;

    let response = (
        StatusCode::CREATED,
        Json(json!({
            "appointment": {
                "id": appointment.id,
                "status": appointment.status,
                "holdExpiresAt": appointment.hold_expires_at
            }
        })),
    )
        .into_response();

    // `server.js` yanıtı e-postayı beklemeden döner.
    let email = state.email.clone();
    tokio::spawn(async move { email.request_received(&appointment).await });

    Ok(response)
}

// ---- yönetici uçları ------------------------------------------------------

/// Şifre doğrulaması: panelden değiştirilmişse saklanan özet, değilse ortam
/// değişkenindeki değer geçerlidir. scrypt CPU-yoğun olduğu için çalışan
/// iş parçacığına taşınır.
async fn verify_password(state: &AppState, password: String) -> Result<bool, AppError> {
    let db = state.db.clone();
    let expected = state.config.admin_password.clone();
    let secret = state.config.session_secret.clone();
    blocking(move || {
        Ok(match db.stored_password()? {
            Some(stored) => stored_password_matches(&password, &stored),
            None => password_matches(&password, &expected, &secret),
        })
    })
    .await
}

async fn admin_login(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, AppError> {
    let ip = client_ip(&request, state.config.production);
    let body = read_body(request).await?;
    let payload = parse_json(&body)?;
    let password = as_string(&payload, "password");

    if !verify_password(&state, password).await? {
        state.audit("auth.login.failed", "", &ip).await;
        return Err(AppError::unauthorized("Şifre hatalı."));
    }
    state.audit("auth.login", "", &ip).await;

    let db = state.db.clone();
    let session = blocking(move || db.create_session(SESSION_TTL_MS, now_ms())).await?;

    let mut response = Json(json!({
        "ok": true,
        "csrfToken": session.csrf_token,
        "expiresAt": session.expires_at
    }))
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&session.token, state.config.production))
            .map_err(|_| AppError::internal("çerez oluşturulamadı"))?,
    );
    Ok(response)
}

async fn admin_session(Extension(admin): Extension<AdminContext>) -> Json<Value> {
    Json(json!({
        "authenticated": true,
        "csrfToken": admin.session.csrf_token,
        "expiresAt": admin.session.expires_at
    }))
}

async fn admin_logout(
    State(state): State<AppState>,
    Extension(admin): Extension<AdminContext>,
    request: Request,
) -> Result<Response, AppError> {
    let ip = client_ip(&request, state.config.production);
    let db = state.db.clone();
    blocking(move || db.delete_session(&admin.token)).await?;
    state.audit("auth.logout", "", &ip).await;

    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cleared_cookie(state.config.production))
            .map_err(|_| AppError::internal("çerez temizlenemedi"))?,
    );
    Ok(response)
}

/// Liste ve dışa aktarma aynı filtreleri paylaşır; "ne görüyorsam onu indir"
/// davranışının tek kaynağı budur.
fn appointment_query(params: &HashMap<String, String>) -> Result<AppointmentQuery, AppError> {
    let status = params
        .get("status")
        .cloned()
        .unwrap_or_default()
        .to_uppercase();
    const ALLOWED: [&str; 7] = [
        "",
        "PENDING",
        "APPROVED",
        "REJECTED",
        "EXPIRED",
        "CANCELLED",
        "CONFLICT",
    ];
    if !ALLOWED.contains(&status.as_str()) {
        return Err(AppError::validation("Geçersiz durum filtresi."));
    }

    let service_id = params.get("service").cloned().unwrap_or_default();
    if !service_id.is_empty() {
        assert_service(&service_id)?;
    }

    // `from`/`to` ikisi birlikte verilir; takvim görünümü gezilen ayı böyle daraltır.
    let (from, to) = match (
        optional_timestamp(params, "from", "Başlangıç")?,
        optional_timestamp(params, "to", "Bitiş")?,
    ) {
        (None, None) => (0, 0),
        (from, to) => {
            let from = from.ok_or_else(|| AppError::validation("Başlangıç geçerli değil."))?;
            let to = to.ok_or_else(|| AppError::validation("Bitiş geçerli değil."))?;
            if to <= from || to - from > 366 * DAY_MS {
                return Err(AppError::validation("Randevu aralığı geçerli değil."));
            }
            (from, to)
        }
    };

    // Arama; ad, e-posta ve telefonda geçer. Türkçe karakterler katlanır.
    let search = params
        .get("q")
        .map(|value| value.trim())
        .unwrap_or_default()
        .to_string();
    if search.chars().count() > 100 {
        return Err(AppError::validation("Arama metni çok uzun."));
    }

    Ok(AppointmentQuery {
        status,
        service_id,
        search,
        from,
        to,
        limit: parse_count(params, "limit", 200, 1, 500, "Kayıt sayısı")?,
        offset: parse_count(params, "offset", 0, 0, 100_000, "Başlangıç konumu")?,
    })
}

async fn admin_list_appointments(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, AppError> {
    let query = appointment_query(&params)?;
    let db = state.db.clone();
    let page = blocking(move || db.list_appointments(&query)).await?;
    Ok(Json(json!({
        "appointments": page.appointments,
        "total": page.total,
        "counts": page.counts
    })))
}

/// Sorgu dizesindeki isteğe bağlı tam sayı; boşsa varsayılana düşer, aralık dışıysa hata.
fn parse_count(
    params: &HashMap<String, String>,
    key: &str,
    fallback: i64,
    min: i64,
    max: i64,
    label: &str,
) -> Result<i64, AppError> {
    let Some(raw) = params
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(fallback);
    };
    match raw.parse::<i64>() {
        Ok(count) if (min..=max).contains(&count) => Ok(count),
        _ => Err(AppError::validation(format!("{label} geçerli değil."))),
    }
}

/// Tek istekte dışa aktarılabilecek en fazla kayıt.
const MAX_EXPORT_ROWS: i64 = 5_000;

/// Listedeki filtrelerin aynısıyla CSV veya iCal üretir; yönetici ne görüyorsa
/// onu indirir. Sayfalama uygulanmaz.
async fn admin_export_appointments(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let format = params.get("format").map(String::as_str).unwrap_or("csv");
    if !matches!(format, "csv" | "ics") {
        return Err(AppError::validation("Geçersiz dışa aktarma biçimi."));
    }

    let mut query = appointment_query(&params)?;
    query.limit = MAX_EXPORT_ROWS;
    query.offset = 0;

    let db = state.db.clone();
    let page = blocking(move || db.list_appointments(&query)).await?;

    let today = iso_date(now_ms() + BOOKING_RULES.offset_ms());
    let (body, mime, name) = if format == "csv" {
        (
            csv_export(&page.appointments),
            "text/csv; charset=utf-8",
            format!("randevular-{today}.csv"),
        )
    } else {
        (
            ics_export(&page.appointments),
            "text/calendar; charset=utf-8",
            format!("randevular-{today}.ics"),
        )
    };

    Ok((
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response())
}

/// Panelden girilen randevu alanlarını doğrular. Elle oluşturma ve düzenleme
/// aynı kuralları paylaşır: kamuya açık formdan farklı olarak bal küpü ve
/// zamanlama kontrolleri yoktur, e-posta isteğe bağlıdır ve saatin yayınlanmış
/// müsaitlikte olması gerekmez.
fn validate_admin_appointment(payload: &Value) -> Result<NewAppointment, AppError> {
    if !payload.is_object() {
        return Err(AppError::validation("Geçersiz form verisi."));
    }

    let service_id = as_string(payload, "serviceId");
    let service_name = assert_service(&service_id)?;

    let start_at = timestamp_field(payload, "start", "Randevu saati")?;
    assert_slot_grid(start_at)?;
    if start_at > now_ms() + 365 * DAY_MS {
        return Err(AppError::validation(
            "Randevu en fazla bir yıl sonrasına oluşturulabilir.",
        ));
    }

    let name = normalize_text(payload, "name", 2, 100, "Ad soyad")?;

    let phone: String = as_string(payload, "phone")
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == '+')
        .collect();
    if !is_valid_phone(&phone) {
        return Err(AppError::validation("Geçerli bir telefon numarası girin."));
    }

    // Telefonla gelen danışanın e-postası olmayabilir; boş bırakılabilir.
    let email = as_string(payload, "email").trim().to_lowercase();
    if !email.is_empty() && (!is_valid_email(&email) || email.chars().count() > 160) {
        return Err(AppError::validation("Geçerli bir e-posta adresi girin."));
    }

    Ok(NewAppointment {
        service_id,
        service_name: service_name.to_string(),
        start_at,
        end_at: start_at + BOOKING_RULES.slot_ms(),
        name,
        email,
        phone,
        note: normalize_text(payload, "note", 0, 500, "Kısa not")?,
    })
}

/// Mevcut bir randevuyu düzenler. Durum ve tutma süresi değişmez.
async fn admin_update_appointment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>, AppError> {
    let ip = client_ip(&request, state.config.production);
    let body = read_body(request).await?;
    let payload = parse_json(&body)?;
    let input = validate_admin_appointment(&payload)?;

    let db = state.db.clone();
    let lookup = state.db.clone();
    let lookup_id = id.clone();
    let previous = blocking(move || lookup.get_appointment(&lookup_id))
        .await?
        .ok_or_else(|| AppError::not_found("Randevu bulunamadı."))?;

    let (appointment, moved) =
        blocking(move || db.update_appointment(&id, &input, now_ms())).await?;

    state
        .audit(
            if moved {
                "appointment.reschedule"
            } else {
                "appointment.update"
            },
            &format!(
                "{} · {}{}",
                appointment.name,
                format_date_time_long(appointment.start_at),
                if moved {
                    format!(" (önceki: {})", format_date_time_long(previous.start_at))
                } else {
                    String::new()
                }
            ),
            &ip,
        )
        .await;

    // Bildirim yalnızca kesinleşmiş bir randevu taşındığında gider; bekleyen
    // talep henüz karara bağlanmadığı için danışana "saatiniz değişti" denmez.
    if moved && appointment.status == "APPROVED" && !appointment.email.is_empty() {
        let email = state.email.clone();
        let previous_start = previous.start_at;
        let moved_appointment = appointment.clone();
        tokio::spawn(async move {
            email
                .appointment_rescheduled(&moved_appointment, previous_start)
                .await
        });
    }

    Ok(Json(json!({ "appointment": appointment })))
}

/// Panelden elle randevu oluşturur (telefonla gelen danışan için).
/// Kayıt doğrudan onaylı açılır; yönetici kararını telefonda vermiştir.
async fn admin_create_appointment(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, AppError> {
    let ip = client_ip(&request, state.config.production);
    let body = read_body(request).await?;
    let payload = parse_json(&body)?;
    let input = validate_admin_appointment(&payload)?;

    let now = now_ms();
    if input.start_at < now {
        return Err(AppError::validation(
            "Geçmiş bir saate randevu oluşturulamaz.",
        ));
    }

    let db = state.db.clone();
    let appointment =
        blocking(move || db.create_manual_appointment(&input, MANUAL_ENTRY_NOTE, now)).await?;

    state
        .audit(
            "appointment.create",
            &format!(
                "{} · {}",
                appointment.name,
                format_date_time_long(appointment.start_at)
            ),
            &ip,
        )
        .await;

    let response = (
        StatusCode::CREATED,
        Json(json!({ "appointment": appointment })),
    )
        .into_response();

    // E-posta verildiyse onay bildirimi gider; verilmediyse sessizce atlanır.
    if !appointment.email.is_empty() {
        let email = state.email.clone();
        tokio::spawn(async move { email.appointment_approved(&appointment).await });
    }
    Ok(response)
}

async fn admin_decide_appointment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, AppError> {
    let ip = client_ip(&request, state.config.production);
    let body = read_body(request).await?;
    let payload = parse_json(&body)?;
    let action = as_string(&payload, "action");
    let admin_note = normalize_text(&payload, "adminNote", 0, 500, "Yönetici notu")?;

    let db = state.db.clone();
    let decide_action = action.clone();
    let (appointment, conflict) =
        blocking(move || db.decide_appointment(&id, &decide_action, &admin_note, now_ms())).await?;

    if conflict {
        // Çakışma yanıtı hem hata hem de güncel randevuyu taşır.
        return Ok((
            StatusCode::CONFLICT,
            Json(json!({
                "error": { "code": "CONFLICT", "message": "Seçilen saat başka bir kayıtla çakışıyor." },
                "appointment": appointment
            })),
        )
            .into_response());
    }

    state
        .audit(
            match action.as_str() {
                "approve" => "appointment.approve",
                "reject" => "appointment.reject",
                _ => "appointment.cancel",
            },
            &format!(
                "{} · {}",
                appointment.name,
                format_date_time_long(appointment.start_at)
            ),
            &ip,
        )
        .await;

    let response = Json(json!({ "appointment": appointment })).into_response();

    let email = state.email.clone();
    tokio::spawn(async move {
        match action.as_str() {
            "approve" => email.appointment_approved(&appointment).await,
            "reject" => email.appointment_rejected(&appointment).await,
            "cancel" => email.appointment_cancelled(&appointment).await,
            _ => {}
        }
    });

    Ok(response)
}

/// `from`/`to` verilmezse varsayılan pencere bugünden itibaren 90 gündür.
/// Admin takvimi başka bir aya gittiğinde o ayın aralığını göndererek
/// kapalı zamanları da o aya göre alır.
async fn admin_change_password(
    State(state): State<AppState>,
    Extension(admin): Extension<AdminContext>,
    request: Request,
) -> Result<Json<Value>, AppError> {
    let ip = client_ip(&request, state.config.production);
    let body = read_body(request).await?;
    let payload = parse_json(&body)?;

    let current = as_string(&payload, "current");
    let next = as_string(&payload, "next");

    // Yeni şifre kuralı yapılandırmadakiyle aynı; zayıf şifreyle kilitlenmeyi önler.
    if next.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(AppError::validation(format!(
            "Yeni şifre en az {MIN_PASSWORD_LENGTH} karakter olmalıdır."
        )));
    }
    if next == current {
        return Err(AppError::validation(
            "Yeni şifre mevcut şifreyle aynı olamaz.",
        ));
    }

    if !verify_password(&state, current).await? {
        state.audit("auth.password.failed", "", &ip).await;
        return Err(AppError::unauthorized("Mevcut şifre hatalı."));
    }

    let db = state.db.clone();
    let keep = admin.token.clone();
    let closed = blocking(move || {
        let salt = crate::db::random_token(16);
        let hash = derive_hash(&next, &salt)
            .ok_or_else(|| AppError::internal("şifre özeti üretilemedi"))?;
        db.set_password(&salt, &encode_hash(&hash), now_ms())?;
        // Diğer cihazlardaki oturumlar düşer; mevcut oturum korunur ki
        // yönetici işlemin ortasında dışarı atılmasın.
        db.delete_other_sessions(&keep)
    })
    .await?;

    state
        .audit(
            "auth.password.changed",
            &format!("{closed} oturum kapatıldı"),
            &ip,
        )
        .await;
    Ok(Json(json!({ "ok": true, "closedSessions": closed })))
}

async fn admin_list_audit(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, AppError> {
    let limit = parse_count(&params, "limit", 50, 1, 200, "Kayıt sayısı")?;
    let offset = parse_count(&params, "offset", 0, 0, 100_000, "Başlangıç konumu")?;
    let db = state.db.clone();
    let (entries, total) = blocking(move || db.list_audit(limit, offset)).await?;
    Ok(Json(json!({ "entries": entries, "total": total })))
}

async fn admin_list_working_hours(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let db = state.db.clone();
    let hours = blocking(move || db.list_working_hours()).await?;
    Ok(Json(json!({ "hours": hours })))
}

async fn admin_set_working_hours(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, AppError> {
    let ip = client_ip(&request, state.config.production);
    let body = read_body(request).await?;
    let payload = parse_json(&body)?;
    let entries = payload
        .get("hours")
        .and_then(Value::as_array)
        .filter(|entries| entries.len() == 7)
        .ok_or_else(|| AppError::validation("Yedi günün tamamı gönderilmelidir."))?;

    let mut hours = Vec::with_capacity(7);
    let mut seen = [false; 7];
    for entry in entries {
        let weekday = entry.get("weekday").and_then(Value::as_i64).unwrap_or(-1);
        if !(0..=6).contains(&weekday) {
            return Err(AppError::validation("Geçersiz gün."));
        }
        if std::mem::replace(&mut seen[weekday as usize], true) {
            return Err(AppError::validation(
                "Aynı gün birden fazla kez gönderildi.",
            ));
        }

        let start_minute = entry
            .get("startMinute")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        let end_minute = entry.get("endMinute").and_then(Value::as_i64).unwrap_or(-1);
        // Pencere, slot ızgarasıyla hizalı olmalı; aksi halde üretilen saatler
        // yayınlanabilir saatlerle örtüşmezdi.
        let step = BOOKING_RULES.slot_step_minutes;
        if start_minute < 0
            || end_minute > 24 * 60
            || start_minute >= end_minute
            || start_minute % step != 0
            || end_minute % step != 0
        {
            return Err(AppError::validation(
                "Çalışma saatleri 30 dakikalık dilimlerle ve başlangıç bitişten önce olmalıdır.",
            ));
        }

        hours.push(WorkingHours {
            weekday,
            start_minute,
            end_minute,
            closed: entry.get("closed") == Some(&Value::Bool(true)),
        });
    }

    let closed = hours.iter().filter(|entry| entry.closed).count();
    let db = state.db.clone();
    let saved = blocking(move || {
        db.set_working_hours(&hours)?;
        db.list_working_hours()
    })
    .await?;
    state
        .audit("hours.update", &format!("{closed} gün kapalı"), &ip)
        .await;
    Ok(Json(json!({ "hours": saved })))
}

async fn admin_list_blocks(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, AppError> {
    let now = now_ms();
    let from = optional_timestamp(&params, "from", "Başlangıç")?.unwrap_or(now);
    let to = optional_timestamp(&params, "to", "Bitiş")?.unwrap_or(now + 90 * DAY_MS);
    if to <= from || to - from > 366 * DAY_MS {
        return Err(AppError::validation("Kapalı zaman aralığı geçerli değil."));
    }

    let db = state.db.clone();
    let blocks = blocking(move || db.list_blocks(from, to)).await?;
    Ok(Json(json!({ "blocks": blocks })))
}

/// Sorgu dizesindeki isteğe bağlı ISO zaman damgası; boşsa `None`, bozuksa hata.
fn optional_timestamp(
    params: &HashMap<String, String>,
    key: &str,
    label: &str,
) -> Result<Option<i64>, AppError> {
    match params
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
    {
        None => Ok(None),
        Some(value) => parse_timestamp(value)
            .map(Some)
            .ok_or_else(|| AppError::validation(format!("{label} geçerli değil."))),
    }
}

async fn admin_create_block(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, AppError> {
    let ip = client_ip(&request, state.config.production);
    let body = read_body(request).await?;
    let payload = parse_json(&body)?;
    let start_at = timestamp_field(&payload, "start", "Başlangıç")?;
    let end_at = timestamp_field(&payload, "end", "Bitiş")?;
    if end_at <= start_at {
        return Err(AppError::validation(
            "Bitiş zamanı başlangıçtan sonra olmalıdır.",
        ));
    }
    if end_at - start_at > 31 * DAY_MS {
        return Err(AppError::validation(
            "Tek bir kapalı zaman 31 günden uzun olamaz.",
        ));
    }
    let reason = normalize_text(&payload, "reason", 0, 200, "Açıklama")?;

    let db = state.db.clone();
    let block = blocking(move || db.create_block(start_at, end_at, &reason, now_ms())).await?;
    state
        .audit(
            "block.create",
            &format!(
                "{} – {}{}",
                format_date_time_long(block.start_at),
                format_date_time_long(block.end_at),
                if block.reason.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", block.reason)
                }
            ),
            &ip,
        )
        .await;
    Ok((StatusCode::CREATED, Json(json!({ "block": block }))).into_response())
}

async fn admin_delete_block(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, AppError> {
    let ip = client_ip(&request, state.config.production);
    let db = state.db.clone();
    let removed = blocking(move || db.delete_block(&id)).await?;
    if !removed {
        return Err(AppError::not_found("Kapalı zaman kaydı bulunamadı."));
    }
    state.audit("block.delete", "", &ip).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn admin_list_slots(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, AppError> {
    let service_id = params.get("service").cloned().unwrap_or_default();
    assert_service(&service_id)?;
    let from = parse_timestamp(params.get("from").map(String::as_str).unwrap_or(""))
        .ok_or_else(|| AppError::validation("Başlangıç geçerli değil."))?;
    let to = parse_timestamp(params.get("to").map(String::as_str).unwrap_or(""))
        .ok_or_else(|| AppError::validation("Bitiş geçerli değil."))?;
    if to <= from || to - from > 32 * DAY_MS {
        return Err(AppError::validation("Takvim aralığı geçerli değil."));
    }

    let db = state.db.clone();
    let slots = blocking(move || db.list_availability_slots(&service_id, from, to)).await?;
    Ok(Json(json!({ "slots": slots })))
}

/// Bir aylık takvimin tamamı (31 gün x 28 saat) tek istekte sığsın diye.
const MAX_BULK_SLOTS: usize = 1000;

async fn admin_set_slots_bulk(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Value>, AppError> {
    let ip = client_ip(&request, state.config.production);
    let body = read_body(request).await?;
    let payload = parse_json(&body)?;
    let service_id = as_string(&payload, "serviceId");
    assert_service(&service_id)?;

    let entries = payload
        .get("slots")
        .and_then(Value::as_array)
        .filter(|entries| !entries.is_empty())
        .ok_or_else(|| AppError::validation("En az bir saat seçilmelidir."))?;
    if entries.len() > MAX_BULK_SLOTS {
        return Err(AppError::validation(format!(
            "Tek seferde en fazla {MAX_BULK_SLOTS} saat değiştirilebilir."
        )));
    }

    let now = now_ms();
    let mut seen = std::collections::HashSet::with_capacity(entries.len());
    let mut changes = Vec::with_capacity(entries.len());
    for entry in entries {
        let start_at = timestamp_field(entry, "start", "Slot zamanı")?;
        assert_slot_grid(start_at)?;
        if start_at < now {
            return Err(AppError::validation("Geçmiş bir saat değiştirilemez."));
        }
        if !seen.insert(start_at) {
            return Err(AppError::validation(
                "Aynı saat birden fazla kez gönderildi.",
            ));
        }
        changes.push(SlotChange {
            start_at,
            end_at: start_at + BOOKING_RULES.slot_ms(),
            open: entry.get("open") == Some(&Value::Bool(true)),
        });
    }

    let db = state.db.clone();
    let label = service_id.clone();
    let applied = blocking(move || db.set_availability_slots(&service_id, &changes, now)).await?;
    state
        .audit("slots.bulk", &format!("{label} · {applied} saat"), &ip)
        .await;
    Ok(Json(json!({ "applied": applied })))
}

async fn admin_set_slot(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let payload = parse_json(&body)?;
    let service_id = as_string(&payload, "serviceId");
    assert_service(&service_id)?;
    let start_at = timestamp_field(&payload, "start", "Slot zamanı")?;
    assert_slot_grid(start_at)?;
    if start_at < now_ms() {
        return Err(AppError::validation("Geçmiş bir saat değiştirilemez."));
    }
    let open = payload.get("open") == Some(&Value::Bool(true));

    let db = state.db.clone();
    let slot = blocking(move || {
        db.set_availability_slot(
            &service_id,
            start_at,
            start_at + BOOKING_RULES.slot_ms(),
            open,
            now_ms(),
        )
    })
    .await?;

    Ok(Json(json!({ "open": slot.is_some(), "slot": slot })))
}

// ---- müsaitlik hesabı -----------------------------------------------------

/// `buildAvailability` karşılığı: yayınlanmış saatlerden, bildirim süresi ve
/// tampon kuralları uygulanmış müsait saat listesi üretir.
pub fn build_availability(db: &Db, service_id: &str, now: i64) -> Result<Value, AppError> {
    let min_start = now + BOOKING_RULES.minimum_notice_hours * 3_600_000;
    let horizon = now + BOOKING_RULES.horizon_days * DAY_MS;

    let local_now = civil_from_ms(now + BOOKING_RULES.offset_ms());
    let (year, month0, day) = (
        local_now.year,
        local_now.month as i64 - 1,
        local_now.day as i64,
    );

    let busy = db.get_busy_ranges(now, horizon + DAY_MS, now)?;
    let published = db.list_availability_slots(service_id, now, horizon + DAY_MS)?;
    // Çalışma penceresi dış zarftır: yönetici bir günü kapattığında ya da saatleri
    // daralttığında, o aralıkta daha önce yayınlanmış saatler de sunulmaz.
    // Slot kayıtları silinmediği için pencere yeniden genişletilirse geri gelirler.
    let working: Vec<crate::db::WorkingHours> = db.list_working_hours()?;

    let mut days = Vec::new();
    for offset in 0..=BOOKING_RULES.horizon_days {
        let local_midnight = utc_ms(year, month0, day + offset);
        let day_start = local_midnight - BOOKING_RULES.offset_ms();
        let day_end = day_start + DAY_MS;
        let weekday = civil_from_ms(local_midnight).weekday as i64;
        let window = working.iter().find(|entry| entry.weekday == weekday);

        let mut slots: Vec<(i64, Value)> = published
            .iter()
            .filter(|slot| slot.start_at >= day_start && slot.start_at < day_end)
            .filter_map(|slot| {
                let start = slot.start_at;
                if start < min_start || start > horizon {
                    return None;
                }
                let minute_of_day = (start - day_start) / 60_000;
                if !window.is_some_and(|entry| entry.covers(minute_of_day)) {
                    return None;
                }
                // Randevunun kendi tamponu kadar sonrası da korunur.
                let protected_end = slot.end_at + BOOKING_RULES.buffer_ms();
                let overlaps = busy.iter().any(|range| {
                    let range_end = if range.is_appointment {
                        range.end + BOOKING_RULES.buffer_ms()
                    } else {
                        range.end
                    };
                    range.start < protected_end && range_end > start
                });
                if overlaps {
                    return None;
                }
                Some((
                    start,
                    json!({ "start": to_iso_string(start), "label": format_time(start) }),
                ))
            })
            .collect();

        if slots.is_empty() {
            continue;
        }
        slots.sort_by_key(|(start, _)| *start);
        days.push(json!({
            "date": iso_date(local_midnight),
            "label": format_day(local_midnight),
            "slots": slots.into_iter().map(|(_, slot)| slot).collect::<Vec<_>>()
        }));
    }

    Ok(json!({
        "timezone": BOOKING_RULES.timezone,
        "generatedAt": to_iso_string(now),
        "rangeStart": iso_date(utc_ms(year, month0, day)),
        "rangeEnd": iso_date(utc_ms(year, month0, day + BOOKING_RULES.horizon_days)),
        "days": days
    }))
}

// ---- doğrulama ------------------------------------------------------------

fn validate_booking_payload(payload: &Value) -> Result<NewAppointment, AppError> {
    if !payload.is_object() {
        return Err(AppError::validation("Geçersiz form verisi."));
    }
    // Bal küpü alanı: botlar doldurur, gerçek kullanıcılar görmez.
    if !as_string(payload, "website").trim().is_empty() {
        return Err(AppError::validation("Rezervasyon doğrulanamadı."));
    }
    let started_at = payload
        .get("startedAt")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if started_at == 0 || now_ms() - started_at < 3_000 {
        return Err(AppError::validation(
            "Form çok hızlı gönderildi. Lütfen tekrar deneyin.",
        ));
    }

    let service_id = as_string(payload, "serviceId");
    let service = assert_service(&service_id)?;

    let name = normalize_text(payload, "name", 2, 100, "Ad soyad")?;

    let email = as_string(payload, "email").trim().to_lowercase();
    if !is_valid_email(&email) || email.chars().count() > 160 {
        return Err(AppError::validation("Geçerli bir e-posta adresi girin."));
    }

    let phone: String = as_string(payload, "phone")
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == '+')
        .collect();
    if !is_valid_phone(&phone) {
        return Err(AppError::validation("Geçerli bir telefon numarası girin."));
    }

    let note = normalize_text(payload, "note", 0, 500, "Kısa not")?;

    if payload.get("consent") != Some(&Value::Bool(true)) {
        return Err(AppError::validation(
            "Veri kullanım açıklamasını onaylamalısınız.",
        ));
    }

    let start_at = timestamp_field(payload, "start", "Randevu saati")?;
    assert_slot(start_at, now_ms())?;

    Ok(NewAppointment {
        service_id,
        service_name: service.to_string(),
        start_at,
        end_at: start_at + BOOKING_RULES.slot_ms(),
        name,
        email,
        phone,
        note,
    })
}

fn assert_service(service_id: &str) -> Result<&'static str, AppError> {
    service_name(service_id).ok_or_else(|| AppError::validation("Geçerli bir hizmet seçin."))
}

fn assert_slot(timestamp: i64, now: i64) -> Result<(), AppError> {
    if timestamp.rem_euclid(60_000) != 0 {
        return Err(AppError::validation("Geçerli bir randevu saati seçin."));
    }
    if timestamp < now + BOOKING_RULES.minimum_notice_hours * 3_600_000
        || timestamp > now + BOOKING_RULES.horizon_days * DAY_MS
    {
        return Err(AppError::validation(
            "Seçilen saat rezervasyon aralığının dışında.",
        ));
    }
    let local = civil_from_ms(timestamp + BOOKING_RULES.offset_ms());
    if (local.minute as i64).rem_euclid(BOOKING_RULES.slot_step_minutes) != 0 {
        return Err(AppError::validation(
            "Seçilen saat geçerli bir zaman dilimi değil.",
        ));
    }
    Ok(())
}

fn assert_slot_grid(timestamp: i64) -> Result<(), AppError> {
    if timestamp.rem_euclid(60_000) != 0 {
        return Err(AppError::validation("Slot zamanı geçerli değil."));
    }
    let local = civil_from_ms(timestamp + BOOKING_RULES.offset_ms());
    if (local.minute as i64).rem_euclid(BOOKING_RULES.slot_step_minutes) != 0 {
        return Err(AppError::validation(
            "Slot 30 dakikalık takvime uygun değil.",
        ));
    }
    Ok(())
}

/// Seçilen saat gerçekten müsaitlik listesinde sunuluyor mu?
fn assert_slot_offered(
    db: &Db,
    service_id: &str,
    timestamp: i64,
    now: i64,
) -> Result<(), AppError> {
    let availability = build_availability(db, service_id, now)?;
    let offered = availability["days"].as_array().is_some_and(|days| {
        days.iter().any(|day| {
            day["slots"].as_array().is_some_and(|slots| {
                slots
                    .iter()
                    .any(|slot| slot["start"].as_str().and_then(parse_timestamp) == Some(timestamp))
            })
        })
    });
    if !offered {
        return Err(AppError::conflict(
            "Seçilen saat artık sunulmuyor. Lütfen takvimi yenileyin.",
        ));
    }
    Ok(())
}

fn is_valid_email(email: &str) -> bool {
    // JS `/^[^\s@]+@[^\s@]+\.[^\s@]+$/` karşılığı.
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    let clean = |part: &str| {
        !part.is_empty() && !part.contains('@') && !part.chars().any(char::is_whitespace)
    };
    if !clean(local) || !clean(domain) {
        return false;
    }
    match domain.rsplit_once('.') {
        Some((head, tail)) => !head.is_empty() && !tail.is_empty(),
        None => false,
    }
}

fn is_valid_phone(phone: &str) -> bool {
    // JS `/^\+?[0-9]{10,15}$/` karşılığı.
    let digits = phone.strip_prefix('+').unwrap_or(phone);
    (10..=15).contains(&digits.len()) && digits.chars().all(|character| character.is_ascii_digit())
}

// ---- gövde yardımcıları ---------------------------------------------------

fn parse_json(body: &Bytes) -> Result<Value, AppError> {
    if body.is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_slice(body).map_err(|_| AppError::validation("Geçersiz form verisi."))
}

/// JS `String(value || '')` davranışı.
fn as_string(payload: &Value, key: &str) -> String {
    match payload.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => {
            if number.as_f64() == Some(0.0) {
                String::new()
            } else {
                number.to_string()
            }
        }
        Some(Value::Bool(true)) => "true".into(),
        _ => String::new(),
    }
}

/// JS `normalizeText`: ardışık boşlukları teke indirir, kırpar, uzunluk doğrular.
fn normalize_text(
    payload: &Value,
    key: &str,
    min: usize,
    max: usize,
    label: &str,
) -> Result<String, AppError> {
    let text = as_string(payload, key)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let length = text.chars().count();
    if length < min || length > max {
        return Err(AppError::validation(format!(
            "{label} alanı {min}–{max} karakter olmalıdır."
        )));
    }
    Ok(text)
}

fn timestamp_field(payload: &Value, key: &str, label: &str) -> Result<i64, AppError> {
    let raw = as_string(payload, key);
    parse_timestamp(&raw).ok_or_else(|| AppError::validation(format!("{label} geçerli değil.")))
}

// ---- çerezler ve şifre ----------------------------------------------------

fn parse_cookies(header: &str) -> HashMap<String, String> {
    header
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let index = part.find('=')?;
            Some((
                percent_decode(&part[..index]),
                percent_decode(&part[index + 1..]),
            ))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    if !value.contains('%') {
        return value.to_string();
    }
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            output.push(byte);
            index += 3;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn session_cookie(token: &str, production: bool) -> String {
    let mut cookie = format!(
        "{SESSION_COOKIE}={token}; Max-Age={}; Path=/; HttpOnly; SameSite=Strict",
        SESSION_TTL_MS / 1000
    );
    if production {
        cookie.push_str("; Secure");
    }
    cookie
}

fn cleared_cookie(production: bool) -> String {
    let mut cookie = format!(
        "{SESSION_COOKIE}=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; HttpOnly; SameSite=Strict"
    );
    if production {
        cookie.push_str("; Secure");
    }
    cookie
}

/// scrypt ile özet üretir. Node `scryptSync` varsayılanları: N=16384 (log2 = 14),
/// r=8, p=1, 64 bayt. Çıktı uzunluğu hedef tampondan belirlenir.
fn derive_hash(password: &str, salt: &str) -> Option<[u8; 64]> {
    let params = scrypt::Params::new(14, 8, 1).ok()?;
    let mut hash = [0u8; 64];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut hash).ok()?;
    Some(hash)
}

fn encode_hash(hash: &[u8; 64]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Panelden değiştirilmiş şifreyi rastgele tuzla doğrular.
fn stored_password_matches(input: &str, stored: &crate::db::StoredPassword) -> bool {
    let Some(hash) = derive_hash(input, &stored.salt) else {
        return false;
    };
    encode_hash(&hash)
        .as_bytes()
        .ct_eq(stored.hash.as_bytes())
        .into()
}

/// Girdi ve beklenen şifre aynı scrypt parametreleriyle türetilip sabit zamanda
/// karşılaştırılır — uzunluk veya içerik sızdırmaz.
fn password_matches(input: &str, expected: &str, secret: &str) -> bool {
    let salt = format!(
        "cemox-admin:{}",
        secret.chars().take(16).collect::<String>()
    );
    // Node `scryptSync` varsayılanları: N=16384 (log2 = 14), r=8, p=1, 64 bayt.
    // Çıktı uzunluğu (64 bayt) hedef tampondan belirlenir.
    let Ok(params) = scrypt::Params::new(14, 8, 1) else {
        return false;
    };
    let mut input_hash = [0u8; 64];
    let mut expected_hash = [0u8; 64];
    if scrypt::scrypt(input.as_bytes(), salt.as_bytes(), &params, &mut input_hash).is_err()
        || scrypt::scrypt(
            expected.as_bytes(),
            salt.as_bytes(),
            &params,
            &mut expected_hash,
        )
        .is_err()
    {
        return false;
    }
    input_hash.ct_eq(&expected_hash).into()
}

// ---- hız sınırlayıcı ------------------------------------------------------

struct Bucket {
    count: u32,
    reset_at: i64,
}

pub struct RateLimiter {
    window_ms: i64,
    max: u32,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    fn new(window_ms: i64, max: u32) -> Self {
        Self {
            window_ms,
            max,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// `true` = isteğe izin verildi.
    fn check(&self, key: &str, now: i64) -> bool {
        let Ok(mut buckets) = self.buckets.lock() else {
            return true;
        };
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            count: 0,
            reset_at: now + self.window_ms,
        });
        if bucket.reset_at <= now {
            bucket.count = 0;
            bucket.reset_at = now + self.window_ms;
        }
        bucket.count += 1;
        let allowed = bucket.count <= self.max;

        if buckets.len() > 5_000 {
            buckets.retain(|_, bucket| bucket.reset_at > now);
        }
        allowed
    }
}
