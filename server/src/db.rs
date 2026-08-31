//! `src/db.js` karşılığı. Şema, sorgular ve `BEGIN IMMEDIATE` transaction sınırları
//! Node sürümüyle birebir aynıdır.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, params};
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
    pub from: i64,
    pub to: i64,
    pub limit: i64,
    pub offset: i64,
}

pub struct AppointmentPage {
    pub appointments: Vec<Appointment>,
    pub total: i64,
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

        let mut clauses: Vec<&str> = Vec::new();
        let mut filters: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if !query.status.is_empty() {
            clauses.push("status = ?");
            filters.push(Box::new(query.status.clone()));
        }
        if !query.service_id.is_empty() {
            clauses.push("service_id = ?");
            filters.push(Box::new(query.service_id.clone()));
        }
        if query.to > query.from {
            clauses.push("start_at < ? AND end_at > ?");
            filters.push(Box::new(query.to));
            filters.push(Box::new(query.from));
        }
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        // Durum filtresi yokken bekleyenler başa alınır; `id` sayfalar arası kararlılık için.
        let order = if query.status.is_empty() {
            "CASE status WHEN 'PENDING' THEN 0 ELSE 1 END, start_at ASC, id ASC"
        } else {
            "start_at ASC, id ASC"
        };

        let filter_refs: Vec<&dyn rusqlite::ToSql> =
            filters.iter().map(|value| value.as_ref()).collect();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM appointments {where_clause}"),
            filter_refs.as_slice(),
            |row| row.get(0),
        )?;

        let mut paged = filter_refs.clone();
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

    pub fn set_availability_slot(
        &self,
        service_id: &str,
        start_at: i64,
        end_at: i64,
        open: bool,
        now: i64,
    ) -> Result<Option<AvailabilitySlot>, AppError> {
        let conn = self.conn()?;
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

fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("işletim sistemi rastgele sayı üreteci kullanılamıyor");
    URL_SAFE_NO_PAD.encode(buffer)
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
