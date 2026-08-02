import fs from 'node:fs';
import path from 'node:path';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';

const ACTIVE_STATUSES = "(status = 'PENDING' AND hold_expires_at > ?) OR status = 'APPROVED'";

export function createDatabase(databasePath) {
  if (databasePath !== ':memory:') fs.mkdirSync(path.dirname(databasePath), { recursive: true });
  const sqlite = new DatabaseSync(databasePath);
  sqlite.exec('PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;');
  sqlite.exec(`
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

    CREATE TABLE IF NOT EXISTS admin_sessions (
      token_hash TEXT PRIMARY KEY,
      csrf_token TEXT NOT NULL,
      expires_at INTEGER NOT NULL,
      created_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_sessions_expiry ON admin_sessions(expires_at);
  `);

  function expirePending(now = Date.now()) {
    const expired = sqlite.prepare("SELECT * FROM appointments WHERE status = 'PENDING' AND hold_expires_at <= ?").all(now);
    if (expired.length) {
      sqlite.prepare("UPDATE appointments SET status = 'EXPIRED', decision_at = ? WHERE status = 'PENDING' AND hold_expires_at <= ?")
        .run(now, now);
    }
    return expired.map(item => ({ ...item, status: 'EXPIRED', decision_at: now }));
  }

  function getBusyRanges(from, to, now = Date.now()) {
    expirePending(now);
    const appointments = sqlite.prepare(`
      SELECT id, start_at AS start, end_at AS end, status, 'appointment' AS kind
      FROM appointments
      WHERE (${ACTIVE_STATUSES}) AND start_at < ? AND end_at > ?
    `).all(now, to, from);
    const blocks = sqlite.prepare(`
      SELECT id, start_at AS start, end_at AS end, 'BLOCKED' AS status, 'block' AS kind
      FROM availability_blocks WHERE start_at < ? AND end_at > ?
    `).all(to, from);
    return [...appointments, ...blocks];
  }

  function isRangeFree(start, end, now = Date.now(), excludeAppointmentId = '') {
    expirePending(now);
    const appointment = sqlite.prepare(`
      SELECT id FROM appointments
      WHERE (${ACTIVE_STATUSES}) AND id != ? AND start_at < ? AND (end_at + 600000) > ? LIMIT 1
    `).get(now, excludeAppointmentId, end, start);
    if (appointment) return false;
    return !sqlite.prepare('SELECT id FROM availability_blocks WHERE start_at < ? AND end_at > ? LIMIT 1').get(end, start);
  }

  function createAppointment(input, now = Date.now()) {
    sqlite.exec('BEGIN IMMEDIATE');
    try {
      expirePending(now);
      const existing = sqlite.prepare(`
        SELECT id FROM appointments
        WHERE status = 'PENDING' AND hold_expires_at > ? AND (lower(email) = lower(?) OR phone = ?) LIMIT 1
      `).get(now, input.email, input.phone);
      if (existing) throw conflictError('Bu e-posta veya telefon numarasıyla zaten bekleyen bir talep var.');
      if (!isRangeFree(input.startAt, input.endAt + 10 * 60 * 1000, now)) {
        throw conflictError('Bu saat artık müsait değil. Lütfen başka bir saat seçin.');
      }

      const id = randomUUID();
      const holdExpiresAt = now + 24 * 60 * 60 * 1000;
      sqlite.prepare(`
        INSERT INTO appointments
          (id, service_id, service_name, start_at, end_at, name, email, phone, note, status, hold_expires_at, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'PENDING', ?, ?)
      `).run(id, input.serviceId, input.serviceName, input.startAt, input.endAt, input.name, input.email, input.phone, input.note, holdExpiresAt, now);
      sqlite.exec('COMMIT');
      return getAppointment(id);
    } catch (error) {
      sqlite.exec('ROLLBACK');
      throw error;
    }
  }

  function getAppointment(id) {
    return sqlite.prepare('SELECT * FROM appointments WHERE id = ?').get(id);
  }

  function listAppointments({ status = '', limit = 100, offset = 0 } = {}) {
    expirePending();
    if (status) {
      return sqlite.prepare('SELECT * FROM appointments WHERE status = ? ORDER BY start_at ASC LIMIT ? OFFSET ?').all(status, limit, offset);
    }
    return sqlite.prepare('SELECT * FROM appointments ORDER BY CASE status WHEN \'PENDING\' THEN 0 ELSE 1 END, start_at ASC LIMIT ? OFFSET ?').all(limit, offset);
  }

  function decideAppointment(id, action, adminNote = '', now = Date.now()) {
    sqlite.exec('BEGIN IMMEDIATE');
    try {
      expirePending(now);
      const appointment = getAppointment(id);
      if (!appointment) throw notFoundError('Randevu talebi bulunamadı.');

      if (action === 'approve') {
        if (!['PENDING', 'CONFLICT'].includes(appointment.status)) throw conflictError('Yalnızca bekleyen veya çakışan talepler onaylanabilir.');
        if (!isRangeFree(appointment.start_at, appointment.end_at + 10 * 60 * 1000, now, id)) {
          sqlite.prepare("UPDATE appointments SET status = 'CONFLICT', decision_at = ?, admin_note = ? WHERE id = ?")
            .run(now, adminNote || 'Onay sırasında saat çakışması oluştu.', id);
          sqlite.exec('COMMIT');
          return { appointment: getAppointment(id), conflict: true };
        }
        sqlite.prepare("UPDATE appointments SET status = 'APPROVED', decision_at = ?, admin_note = ? WHERE id = ?")
          .run(now, adminNote, id);
      } else if (action === 'reject') {
        if (!['PENDING', 'CONFLICT'].includes(appointment.status)) throw conflictError('Bu talep reddedilemez.');
        sqlite.prepare("UPDATE appointments SET status = 'REJECTED', decision_at = ?, admin_note = ? WHERE id = ?")
          .run(now, adminNote, id);
      } else if (action === 'cancel') {
        if (appointment.status !== 'APPROVED') throw conflictError('Yalnızca onaylı randevular iptal edilebilir.');
        sqlite.prepare("UPDATE appointments SET status = 'CANCELLED', decision_at = ?, admin_note = ? WHERE id = ?")
          .run(now, adminNote, id);
      } else {
        throw validationError('Geçersiz işlem.');
      }
      sqlite.exec('COMMIT');
      return { appointment: getAppointment(id), conflict: false };
    } catch (error) {
      try { sqlite.exec('ROLLBACK'); } catch {}
      throw error;
    }
  }

  function listBlocks(from = Date.now(), to = Date.now() + 90 * 86400000) {
    return sqlite.prepare('SELECT * FROM availability_blocks WHERE start_at < ? AND end_at > ? ORDER BY start_at ASC').all(to, from);
  }

  function createBlock({ startAt, endAt, reason }, now = Date.now()) {
    const id = randomUUID();
    sqlite.prepare('INSERT INTO availability_blocks (id, start_at, end_at, reason, created_at) VALUES (?, ?, ?, ?, ?)')
      .run(id, startAt, endAt, reason, now);
    return sqlite.prepare('SELECT * FROM availability_blocks WHERE id = ?').get(id);
  }

  function deleteBlock(id) {
    return sqlite.prepare('DELETE FROM availability_blocks WHERE id = ?').run(id).changes > 0;
  }

  function createSession(ttlMs = 12 * 60 * 60 * 1000, now = Date.now()) {
    const token = randomBytes(32).toString('base64url');
    const tokenHash = hashToken(token);
    const csrfToken = randomBytes(24).toString('base64url');
    sqlite.prepare('DELETE FROM admin_sessions WHERE expires_at <= ?').run(now);
    sqlite.prepare('INSERT INTO admin_sessions (token_hash, csrf_token, expires_at, created_at) VALUES (?, ?, ?, ?)')
      .run(tokenHash, csrfToken, now + ttlMs, now);
    return { token, csrfToken, expiresAt: now + ttlMs };
  }

  function getSession(token, now = Date.now()) {
    if (!token) return null;
    return sqlite.prepare('SELECT * FROM admin_sessions WHERE token_hash = ? AND expires_at > ?').get(hashToken(token), now) || null;
  }

  function deleteSession(token) {
    if (!token) return;
    sqlite.prepare('DELETE FROM admin_sessions WHERE token_hash = ?').run(hashToken(token));
  }

  return {
    sqlite, expirePending, getBusyRanges, isRangeFree, createAppointment, getAppointment,
    listAppointments, decideAppointment, listBlocks, createBlock, deleteBlock,
    createSession, getSession, deleteSession, close: () => sqlite.close()
  };
}

function hashToken(token) {
  return createHash('sha256').update(token).digest('hex');
}

function typedError(message, statusCode, code) {
  const error = new Error(message);
  error.statusCode = statusCode;
  error.code = code;
  return error;
}

export const validationError = message => typedError(message, 400, 'VALIDATION_ERROR');
export const conflictError = message => typedError(message, 409, 'CONFLICT');
export const notFoundError = message => typedError(message, 404, 'NOT_FOUND');
