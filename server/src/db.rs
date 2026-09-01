//! `src/db.js` karşılığı. Şema, sorgular ve `BEGIN IMMEDIATE` transaction sınırları
//! Node sürümüyle birebir aynıdır.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, ToSql, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::BOOKING_RULES;
use crate::error::AppError;
use crate::time::now_ms;

/// Bir saati "dolu" sayan durumlar: süresi dolmamış PENDING veya APPROVED.
/// İlk `?` her zaman `now` parametresidir.
const ACTIVE_STATUSES: &str =
    "((status = 'PENDING' AND hold_expires_at > ?) OR status = 'APPROVED')";

/// Şemadaki CHECK kısıtıyla aynı sıra; sekme sayaçlarında sıfırlar da yer alsın diye.
pub const STATUSES: [&str; 6] = [
    "PENDING",
    "APPROVED",
    "REJECTED",
    "EXPIRED",
    "CANCELLED",
    "CONFLICT",
];

const APPOINTMENT_COLUMNS: &str = "id, service_id, service_name, start_at, end_at, name, email, phone, \
     note, status, hold_expires_at, created_at, decision_at, admin_note";

#[derive(Debug, Clone, Serialize)]
pub struct Appointment {
    pub id: String,
    pub service_id: String,
    pub service_name: String,
    pub start_at: i64,
    pub end_at: i64,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub note: String,
    pub status: String,
    pub hold_expires_at: i64,
    pub created_at: i64,
    pub decision_at: Option<i64>,
    pub admin_note: String,
}

fn map_appointment(row: &Row<'_>) -> rusqlite::Result<Appointment> {
    Ok(Appointment {
        id: row.get(0)?,
        service_id: row.get(1)?,
        service_name: row.get(2)?,
        start_at: row.get(3)?,
        end_at: row.get(4)?,
        name: row.get(5)?,
        email: row.get(6)?,
        phone: row.get(7)?,
        note: row.get(8)?,
        status: row.get(9)?,
        hold_expires_at: row.get(10)?,
        created_at: row.get(11)?,
        decision_at: row.get(12)?,
        admin_note: row.get(13)?,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct AvailabilityBlock {
    pub id: String,
    pub start_at: i64,
    pub end_at: i64,
    pub reason: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AvailabilitySlot {
    pub id: String,
    pub service_id: String,
    pub start_at: i64,
    pub end_at: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct AdminSession {
    pub csrf_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct CreatedSession {
    pub token: String,
    pub csrf_token: String,
    pub expires_at: i64,
}

/// Meşgul aralık: randevu veya genel kapalı zaman.
#[derive(Debug, Clone)]
pub struct BusyRange {
    pub start: i64,
    pub end: i64,
    pub is_appointment: bool,
}

/// `list_appointments` filtreleri. Boş alanlar o koşulu tamamen atlar.
#[derive(Debug, Default, Clone)]
pub struct AppointmentQuery {
    pub status: String,
    pub service_id: String,
    /// Ad, e-posta veya telefonda geçen serbest metin.
    pub search: String,
    pub from: i64,
    pub to: i64,
    pub limit: i64,
    pub offset: i64,
}

pub struct AppointmentPage {
    pub appointments: Vec<Appointment>,
    /// Durum filtresi dahil, sayfalanmamış toplam.
    pub total: i64,
    /// Durum filtresi hariç, duruma göre dağılım; sekme sayaçlarını besler.
    pub counts: BTreeMap<String, i64>,
}

/// Toplu yazımda tek bir saatin hedef durumu.
pub struct SlotChange {
    pub start_at: i64,
    pub end_at: i64,
    pub open: bool,
}

pub struct NewAppointment {
    pub service_id: String,
    pub service_name: String,
    pub start_at: i64,
    pub end_at: i64,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub note: String,
}

#[derive(Clone)]
pub struct Db {
    pool: Pool<SqliteConnectionManager>,
    /// `:memory:` modunda paylaşımlı bellek veritabanını canlı tutar.
    _keepalive: Option<Arc<Mutex<Connection>>>,
}

impl Db {
    pub fn new(database_path: &str) -> Result<Self, AppError> {
        let (manager, keepalive) = if database_path == ":memory:" {
            // Havuzdaki her bağlantı ayrı bir bellek veritabanı açmasın diye
            // paylaşımlı önbellekli URI kullanılır.
            let uri = format!("file:cemox-{}?mode=memory&cache=shared", Uuid::new_v4());
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI;
            let keepalive = Connection::open_with_flags(&uri, flags).map_err(|error| {
                AppError::internal(format!("bellek veritabanı açılamadı: {error}"))
            })?;
            (
                SqliteConnectionManager::file(&uri).with_flags(flags),
                Some(keepalive),
            )
        } else {
            if let Some(parent) = Path::new(database_path).parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    AppError::internal(format!("veri dizini oluşturulamadı: {error}"))
                })?;
            }
            (SqliteConnectionManager::file(database_path), None)
        };

        let manager = manager.with_init(|conn: &mut Connection| {
            conn.busy_timeout(Duration::from_millis(5_000))?;
            // Arama, SQLite'ın ASCII-only LIKE'ı yerine bu katlamayı kullanır.
            conn.create_scalar_function(
                "fold",
                1,
                FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
                |context| Ok(fold_text(&context.get::<String>(0)?)),
            )?;
            // journal_mode satır döndürdüğü için pragma_update yerine query_row gerekir.
            let _mode: String =
                conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
            conn.pragma_update(None, "foreign_keys", "ON")?;
            Ok(())
        });

        let pool = Pool::builder()
            .max_size(8)
            .build(manager)
            .map_err(|error| AppError::internal(format!("havuz kurulamadı: {error}")))?;

        let db = Self {
            pool,
            _keepalive: keepalive.map(|conn| Arc::new(Mutex::new(conn))),
        };
        db.migrate()?;
        Ok(db)
    }

    fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, AppError> {
        self.pool.get().map_err(AppError::from)
    }

    fn migrate(&self) -> Result<(), AppError> {
        self.conn()?.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS appointments (
              id TEXT PRIMARY KEY,
              service_id TEXT NOT NULL,
              service_name TEXT NOT NULL,
              start_at INTEGER NOT NULL,
              end_at INTEGER NOT NULL,
              name TEXT NOT NULL,
              email TEXT NOT NULL,
              phone TEXT NOT NULL,
              note TEXT NOT NULL DEFAULT '',
              status TEXT NOT NULL CHECK(status IN ('PENDING','APPROVED','REJECTED','EXPIRED','CANCELLED','CONFLICT')),
              hold_expires_at INTEGER NOT NULL,
              created_at INTEGER NOT NULL,
              decision_at INTEGER,
              admin_note TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_appointments_time ON appointments(start_at, end_at);
            CREATE INDEX IF NOT EXISTS idx_appointments_status ON appointments(status, hold_expires_at);
            CREATE INDEX IF NOT EXISTS idx_appointments_contact ON appointments(email, phone, status);

            CREATE TABLE IF NOT EXISTS availability_blocks (
              id TEXT PRIMARY KEY,
              start_at INTEGER NOT NULL,
              end_at INTEGER NOT NULL,
              reason TEXT NOT NULL DEFAULT '',
              created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_blocks_time ON availability_blocks(start_at, end_at);

            CREATE TABLE IF NOT EXISTS availability_slots (
              id TEXT PRIMARY KEY,
              service_id TEXT NOT NULL,
              start_at INTEGER NOT NULL,
              end_at INTEGER NOT NULL,
              created_at INTEGER NOT NULL,
              UNIQUE(service_id, start_at)
            );
            CREATE INDEX IF NOT EXISTS idx_slots_service_time ON availability_slots(service_id, start_at, end_at);

            CREATE TABLE IF NOT EXISTS admin_sessions (
              token_hash TEXT PRIMARY KEY,
              csrf_token TEXT NOT NULL,
              expires_at INTEGER NOT NULL,
              created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_expiry ON admin_sessions(expires_at);
            "#,
        )?;
        Ok(())
    }

    // ---- randevular -------------------------------------------------------

    pub fn expire_pending(&self, now: i64) -> Result<Vec<Appointment>, AppError> {
        let conn = self.conn()?;
        expire_pending_conn(&conn, now)
    }

    /// Panelden elle oluşturulan randevu. Kamuya açık akıştan farkları:
    /// doğrudan APPROVED yazılır (yönetici kararını telefonda vermiştir),
    /// mükerrer iletişim kontrolü uygulanmaz (o kontrol formu spam'a karşı korur)
    /// ve saatin yayınlanmış müsaitlikte olması gerekmez — yönetici kapalı bir
    /// saati de verebilir. Çakışma ve kapalı zaman kontrolü yine geçerlidir.
    pub fn create_manual_appointment(
        &self,
        input: &NewAppointment,
        admin_note: &str,
        now: i64,
    ) -> Result<Appointment, AppError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        expire_pending_conn(&tx, now)?;

        if !is_range_free_conn(
            &tx,
            input.start_at,
            input.end_at + BOOKING_RULES.buffer_ms(),
            now,
            "",
        )? {
            return Err(AppError::conflict(
                "Bu saat başka bir randevu veya kapalı zamanla çakışıyor.",
            ));
        }

        let id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO appointments \
               (id, service_id, service_name, start_at, end_at, name, email, phone, note, \
                status, hold_expires_at, created_at, decision_at, admin_note) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'APPROVED', ?, ?, ?, ?)",
            params![
                id,
                input.service_id,
                input.service_name,
                input.start_at,
                input.end_at,
                input.name,
                input.email,
                input.phone,
                input.note,
                now,
                now,
                now,
                admin_note
            ],
        )?;
        let appointment = get_appointment_conn(&tx, &id)?;
        tx.commit()?;
        appointment.ok_or_else(|| AppError::internal("randevu kaydedilemedi"))
    }

    pub fn get_appointment(&self, id: &str) -> Result<Option<Appointment>, AppError> {
        let conn = self.conn()?;
        get_appointment_conn(&conn, id)
    }

    /// Randevuları filtreler ve sayfalar. `total` filtreye uyan tüm kayıtların sayısıdır;
    /// istemci "daha fazla var mı" bilgisini buradan alır.
    /// `from`/`to` verildiğinde aralıkla kesişen kayıtlar döner (takvim görünümü bunu kullanır).
    pub fn list_appointments(&self, query: &AppointmentQuery) -> Result<AppointmentPage, AppError> {
        let conn = self.conn()?;
        expire_pending_conn(&conn, now_ms())?;

        // Durum dışındaki koşullar; sekme sayıları bunları paylaşır, böylece
        // "Onay bekliyor (3)" o anki arama ve aralık içindeki sayıyı gösterir.
        let mut base_clauses: Vec<String> = Vec::new();
        let mut base_params: Vec<Box<dyn ToSql>> = Vec::new();
        if !query.service_id.is_empty() {
            base_clauses.push("service_id = ?".into());
            base_params.push(Box::new(query.service_id.clone()));
        }
        if query.to > query.from {
            base_clauses.push("start_at < ? AND end_at > ?".into());
            base_params.push(Box::new(query.to));
            base_params.push(Box::new(query.from));
        }
        if !query.search.is_empty() {
            let pattern = like_pattern(&query.search);
            let mut branches = vec![
                "fold(name) LIKE ? ESCAPE '\\'".to_string(),
                "fold(email) LIKE ? ESCAPE '\\'".to_string(),
                "phone LIKE ? ESCAPE '\\'".to_string(),
            ];
            base_params.push(Box::new(pattern.clone()));
            base_params.push(Box::new(pattern.clone()));
            base_params.push(Box::new(pattern));
            // Telefon aranırken kullanıcı boşluk veya tire koyabilir; kayıtlar
            // yalnızca rakam ve '+' içerdiği için sorgu da o biçime indirgenir.
            let digits: String = query.search.chars().filter(char::is_ascii_digit).collect();
            if digits.len() >= 3 {
                branches.push("phone LIKE ?".to_string());
                base_params.push(Box::new(format!("%{digits}%")));
            }
            base_clauses.push(format!("({})", branches.join(" OR ")));
        }
        let base_where = if base_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", base_clauses.join(" AND "))
        };
        let base_refs: Vec<&dyn ToSql> = base_params.iter().map(|value| value.as_ref()).collect();

        // Sekme sayıları: durum filtresi uygulanmadan, tek geçişte.
        let mut counts: BTreeMap<String, i64> = STATUSES
            .iter()
            .map(|status| ((*status).to_string(), 0))
            .collect();
        let mut statement = conn.prepare(&format!(
            "SELECT status, COUNT(*) FROM appointments {base_where} GROUP BY status"
        ))?;
        for row in statement.query_map(base_refs.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (status, count) = row?;
            counts.insert(status, count);
        }
        drop(statement);

        // Liste ve toplam, durum filtresi de dahil.
        let mut list_where = base_clauses.clone();
        let mut list_refs = base_refs.clone();
        if !query.status.is_empty() {
            list_where.push("status = ?".into());
            list_refs.push(&query.status);
        }
        let where_clause = if list_where.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", list_where.join(" AND "))
        };

        // Bekleyen talepler tutma süresi dolmadan karara bağlanmalıdır; bu yüzden
        // randevu tarihine değil, önce süresi dolacak olana göre sıralanırlar.
        // (hold_expires_at = created_at + 24 sa olduğundan bu aynı zamanda geliş sırasıdır.)
        // Karara bağlanmış kayıtlar randevu tarihine göre kalır.
        // `id` sayfalar arası kararlılık için son ölçüttür.
        let order = match query.status.as_str() {
            "" => {
                "CASE status WHEN 'PENDING' THEN 0 ELSE 1 END, \
                 CASE status WHEN 'PENDING' THEN hold_expires_at ELSE start_at END ASC, id ASC"
            }
            "PENDING" => "hold_expires_at ASC, id ASC",
            _ => "start_at ASC, id ASC",
        };

        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM appointments {where_clause}"),
            list_refs.as_slice(),
            |row| row.get(0),
        )?;

        let mut paged = list_refs.clone();
        paged.push(&query.limit);
        paged.push(&query.offset);
        let sql = format!(
            "SELECT {APPOINTMENT_COLUMNS} FROM appointments {where_clause} ORDER BY {order} LIMIT ? OFFSET ?"
        );
        let mut statement = conn.prepare(&sql)?;
        let appointments = statement
            .query_map(paged.as_slice(), map_appointment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(AppointmentPage {
            appointments,
            total,
            counts,
        })
    }

    pub fn get_busy_ranges(
        &self,
        from: i64,
        to: i64,
        now: i64,
    ) -> Result<Vec<BusyRange>, AppError> {
        let conn = self.conn()?;
        expire_pending_conn(&conn, now)?;

        let sql = format!(
            "SELECT start_at, end_at FROM appointments WHERE {ACTIVE_STATUSES} AND start_at < ? AND end_at > ?"
        );
        let mut statement = conn.prepare(&sql)?;
        let mut ranges: Vec<BusyRange> = statement
            .query_map(params![now, to, from], |row| {
                Ok(BusyRange {
                    start: row.get(0)?,
                    end: row.get(1)?,
                    is_appointment: true,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut statement = conn.prepare(
            "SELECT start_at, end_at FROM availability_blocks WHERE start_at < ? AND end_at > ?",
        )?;
        let blocks = statement
            .query_map(params![to, from], |row| {
                Ok(BusyRange {
                    start: row.get(0)?,
                    end: row.get(1)?,
                    is_appointment: false,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ranges.extend(blocks);
        Ok(ranges)
    }

    pub fn create_appointment(
        &self,
        input: &NewAppointment,
        now: i64,
    ) -> Result<Appointment, AppError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        expire_pending_conn(&tx, now)?;

        // Aynı kişiden aynı anda birden fazla bekleyen seçim olamaz.
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM appointments \
                 WHERE status = 'PENDING' AND hold_expires_at > ? AND (lower(email) = lower(?) OR phone = ?) LIMIT 1",
                params![now, input.email, input.phone],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(AppError::conflict(
                "Bu e-posta veya telefon numarasıyla zaten bekleyen bir randevu seçimi var.",
            ));
        }

        if !is_range_free_conn(
            &tx,
            input.start_at,
            input.end_at + BOOKING_RULES.buffer_ms(),
            now,
            "",
        )? {
            return Err(AppError::conflict(
                "Bu saat artık müsait değil. Lütfen başka bir saat seçin.",
            ));
        }

        let id = Uuid::new_v4().to_string();
        let hold_expires_at = now + BOOKING_RULES.hold_hours * 3_600_000;
        tx.execute(
            "INSERT INTO appointments \
               (id, service_id, service_name, start_at, end_at, name, email, phone, note, status, hold_expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'PENDING', ?, ?)",
            params![
                id,
                input.service_id,
                input.service_name,
                input.start_at,
                input.end_at,
                input.name,
                input.email,
                input.phone,
                input.note,
                hold_expires_at,
                now
            ],
        )?;
        let appointment = get_appointment_conn(&tx, &id)?;
        tx.commit()?;
        appointment.ok_or_else(|| AppError::internal("randevu kaydedilemedi"))
    }

    /// `decideAppointment` karşılığı. İkinci dönüş değeri çakışma bayrağıdır.
    pub fn decide_appointment(
        &self,
        id: &str,
        action: &str,
        admin_note: &str,
        now: i64,
    ) -> Result<(Appointment, bool), AppError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        expire_pending_conn(&tx, now)?;

        let appointment = get_appointment_conn(&tx, id)?
            .ok_or_else(|| AppError::not_found("Randevu talebi bulunamadı."))?;

        match action {
            "approve" => {
                if !matches!(appointment.status.as_str(), "PENDING" | "CONFLICT") {
                    return Err(AppError::conflict(
                        "Yalnızca bekleyen veya çakışan talepler onaylanabilir.",
                    ));
                }
                let free = is_range_free_conn(
                    &tx,
                    appointment.start_at,
                    appointment.end_at + BOOKING_RULES.buffer_ms(),
                    now,
                    id,
                )?;
                if !free {
                    // Onay anında saat kapandıysa talep CONFLICT olarak işaretlenir.
                    let note = if admin_note.is_empty() {
                        "Onay sırasında saat çakışması oluştu."
                    } else {
                        admin_note
                    };
                    tx.execute(
                        "UPDATE appointments SET status = 'CONFLICT', decision_at = ?, admin_note = ? WHERE id = ?",
                        params![now, note, id],
                    )?;
                    let updated = get_appointment_conn(&tx, id)?;
                    tx.commit()?;
                    return updated
                        .map(|appointment| (appointment, true))
                        .ok_or_else(|| AppError::internal("randevu okunamadı"));
                }
                tx.execute(
                    "UPDATE appointments SET status = 'APPROVED', decision_at = ?, admin_note = ? WHERE id = ?",
                    params![now, admin_note, id],
                )?;
            }
            "reject" => {
                if !matches!(appointment.status.as_str(), "PENDING" | "CONFLICT") {
                    return Err(AppError::conflict("Bu talep reddedilemez."));
                }
                tx.execute(
                    "UPDATE appointments SET status = 'REJECTED', decision_at = ?, admin_note = ? WHERE id = ?",
                    params![now, admin_note, id],
                )?;
            }
            "cancel" => {
                if appointment.status != "APPROVED" {
                    return Err(AppError::conflict(
                        "Yalnızca onaylı randevular iptal edilebilir.",
                    ));
                }
                tx.execute(
                    "UPDATE appointments SET status = 'CANCELLED', decision_at = ?, admin_note = ? WHERE id = ?",
                    params![now, admin_note, id],
                )?;
            }
            _ => return Err(AppError::validation("Geçersiz işlem.")),
        }

        let updated = get_appointment_conn(&tx, id)?;
        tx.commit()?;
        updated
            .map(|appointment| (appointment, false))
            .ok_or_else(|| AppError::internal("randevu okunamadı"))
    }

    // ---- kapalı zamanlar --------------------------------------------------

    pub fn list_blocks(&self, from: i64, to: i64) -> Result<Vec<AvailabilityBlock>, AppError> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT id, start_at, end_at, reason, created_at FROM availability_blocks \
             WHERE start_at < ? AND end_at > ? ORDER BY start_at ASC",
        )?;
        let rows = statement.query_map(params![to, from], map_block)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn create_block(
        &self,
        start_at: i64,
        end_at: i64,
        reason: &str,
        now: i64,
    ) -> Result<AvailabilityBlock, AppError> {
        let conn = self.conn()?;
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO availability_blocks (id, start_at, end_at, reason, created_at) VALUES (?, ?, ?, ?, ?)",
            params![id, start_at, end_at, reason, now],
        )?;
        conn.query_row(
            "SELECT id, start_at, end_at, reason, created_at FROM availability_blocks WHERE id = ?",
            params![id],
            map_block,
        )
        .map_err(AppError::from)
    }

    pub fn delete_block(&self, id: &str) -> Result<bool, AppError> {
        let conn = self.conn()?;
        Ok(conn.execute("DELETE FROM availability_blocks WHERE id = ?", params![id])? > 0)
    }

    // ---- yayınlanmış saatler ----------------------------------------------

    pub fn list_availability_slots(
        &self,
        service_id: &str,
        from: i64,
        to: i64,
    ) -> Result<Vec<AvailabilitySlot>, AppError> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT id, service_id, start_at, end_at, created_at FROM availability_slots \
             WHERE service_id = ? AND start_at >= ? AND start_at < ? ORDER BY start_at",
        )?;
        let rows = statement.query_map(params![service_id, from, to], map_slot)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Birden çok saati tek transaction'da açar/kapatır.
    /// Toplu gün ve kopyalama işlemleri bunu kullanır; tek tek çağrı yapılmaz.
    pub fn set_availability_slots(
        &self,
        service_id: &str,
        changes: &[SlotChange],
        now: i64,
    ) -> Result<usize, AppError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for change in changes {
            set_availability_slot_conn(
                &tx,
                service_id,
                change.start_at,
                change.end_at,
                change.open,
                now,
            )?;
        }
        tx.commit()?;
        Ok(changes.len())
    }

    pub fn set_availability_slot(
        &self,
        service_id: &str,
        start_at: i64,
        end_at: i64,
        open: bool,
        now: i64,
    ) -> Result<Option<AvailabilitySlot>, AppError> {
        let conn = self.conn()?;
        set_availability_slot_conn(&conn, service_id, start_at, end_at, open, now)
    }

    // ---- yönetici oturumları ----------------------------------------------

    pub fn create_session(&self, ttl_ms: i64, now: i64) -> Result<CreatedSession, AppError> {
        let token = random_token(32);
        let csrf_token = random_token(24);
        let expires_at = now + ttl_ms;
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM admin_sessions WHERE expires_at <= ?",
            params![now],
        )?;
        conn.execute(
            "INSERT INTO admin_sessions (token_hash, csrf_token, expires_at, created_at) VALUES (?, ?, ?, ?)",
            params![hash_token(&token), csrf_token, expires_at, now],
        )?;
        Ok(CreatedSession {
            token,
            csrf_token,
            expires_at,
        })
    }

    pub fn get_session(&self, token: &str, now: i64) -> Result<Option<AdminSession>, AppError> {
        if token.is_empty() {
            return Ok(None);
        }
        let conn = self.conn()?;
        conn.query_row(
            "SELECT csrf_token, expires_at FROM admin_sessions WHERE token_hash = ? AND expires_at > ?",
            params![hash_token(token), now],
            |row| Ok(AdminSession { csrf_token: row.get(0)?, expires_at: row.get(1)? }),
        )
        .optional()
        .map_err(AppError::from)
    }

    pub fn delete_session(&self, token: &str) -> Result<(), AppError> {
        if token.is_empty() {
            return Ok(());
        }
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM admin_sessions WHERE token_hash = ?",
            params![hash_token(token)],
        )?;
        Ok(())
    }
}

// ---- bağlantı düzeyinde yardımcılar (transaction içinden de çağrılır) -----

fn get_appointment_conn(conn: &Connection, id: &str) -> Result<Option<Appointment>, AppError> {
    let sql = format!("SELECT {APPOINTMENT_COLUMNS} FROM appointments WHERE id = ?");
    conn.query_row(&sql, params![id], map_appointment)
        .optional()
        .map_err(AppError::from)
}

/// Süresi dolan PENDING kayıtları EXPIRED yapar ve etkilenen kayıtları döndürür.
fn expire_pending_conn(conn: &Connection, now: i64) -> Result<Vec<Appointment>, AppError> {
    let sql = format!(
        "SELECT {APPOINTMENT_COLUMNS} FROM appointments WHERE status = 'PENDING' AND hold_expires_at <= ?"
    );
    let mut statement = conn.prepare(&sql)?;
    let expired = statement
        .query_map(params![now], map_appointment)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    if !expired.is_empty() {
        conn.execute(
            "UPDATE appointments SET status = 'EXPIRED', decision_at = ? \
             WHERE status = 'PENDING' AND hold_expires_at <= ?",
            params![now, now],
        )?;
    }

    Ok(expired
        .into_iter()
        .map(|mut appointment| {
            appointment.status = "EXPIRED".into();
            appointment.decision_at = Some(now);
            appointment
        })
        .collect())
}

/// Aralık, tampon süre dahil boş mu? `exclude_id` onay sırasında kaydın kendisini eler.
fn is_range_free_conn(
    conn: &Connection,
    start: i64,
    end: i64,
    now: i64,
    exclude_id: &str,
) -> Result<bool, AppError> {
    expire_pending_conn(conn, now)?;

    let sql = format!(
        "SELECT id FROM appointments WHERE {ACTIVE_STATUSES} AND id != ? AND start_at < ? AND (end_at + ?) > ? LIMIT 1"
    );
    let clash: Option<String> = conn
        .query_row(
            &sql,
            params![now, exclude_id, end, BOOKING_RULES.buffer_ms(), start],
            |row| row.get(0),
        )
        .optional()?;
    if clash.is_some() {
        return Ok(false);
    }

    let blocked: Option<String> = conn
        .query_row(
            "SELECT id FROM availability_blocks WHERE start_at < ? AND end_at > ? LIMIT 1",
            params![end, start],
            |row| row.get(0),
        )
        .optional()?;
    Ok(blocked.is_none())
}

fn set_availability_slot_conn(
    conn: &Connection,
    service_id: &str,
    start_at: i64,
    end_at: i64,
    open: bool,
    now: i64,
) -> Result<Option<AvailabilitySlot>, AppError> {
    if open {
        conn.execute(
            "INSERT INTO availability_slots (id, service_id, start_at, end_at, created_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(service_id, start_at) DO UPDATE SET end_at = excluded.end_at",
            params![
                Uuid::new_v4().to_string(),
                service_id,
                start_at,
                end_at,
                now
            ],
        )?;
    } else {
        conn.execute(
            "DELETE FROM availability_slots WHERE service_id = ? AND start_at = ?",
            params![service_id, start_at],
        )?;
    }
    conn.query_row(
        "SELECT id, service_id, start_at, end_at, created_at FROM availability_slots \
         WHERE service_id = ? AND start_at = ?",
        params![service_id, start_at],
        map_slot,
    )
    .optional()
    .map_err(AppError::from)
}

fn map_block(row: &Row<'_>) -> rusqlite::Result<AvailabilityBlock> {
    Ok(AvailabilityBlock {
        id: row.get(0)?,
        start_at: row.get(1)?,
        end_at: row.get(2)?,
        reason: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn map_slot(row: &Row<'_>) -> rusqlite::Result<AvailabilitySlot> {
    Ok(AvailabilitySlot {
        id: row.get(0)?,
        service_id: row.get(1)?,
        start_at: row.get(2)?,
        end_at: row.get(3)?,
        created_at: row.get(4)?,
    })
}

/// Arama için harf katlama: Türkçe karakterler ASCII karşılıklarına indirgenir,
/// böylece "sule" araması "Şule" kaydını da bulur. SQLite'ın `lower()` işlevi
/// yalnızca ASCII katladığı için bu işlev SQL'e ayrıca tanıtılır.
fn fold_text(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            'ç' | 'Ç' => folded.push('c'),
            'ğ' | 'Ğ' => folded.push('g'),
            'ı' | 'I' | 'İ' | 'i' => folded.push('i'),
            'ö' | 'Ö' => folded.push('o'),
            'ş' | 'Ş' => folded.push('s'),
            'ü' | 'Ü' => folded.push('u'),
            other => folded.extend(other.to_lowercase()),
        }
    }
    folded
}

/// LIKE kalıbındaki joker karakterleri kaçırır; aksi halde "%" tüm kayıtları eşlerdi.
fn like_pattern(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len() + 2);
    escaped.push('%');
    for character in fold_text(term).chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("işletim sistemi rastgele sayı üreteci kullanılamıyor");
    URL_SAFE_NO_PAD.encode(buffer)
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
