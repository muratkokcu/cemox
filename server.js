import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { scryptSync, timingSafeEqual } from 'node:crypto';
import express from 'express';
import { loadConfig, SERVICES, BOOKING_RULES } from './src/config.js';
import { createDatabase, validationError, conflictError } from './src/db.js';
import { createEmailService } from './src/email.js';

const rootDir = path.dirname(fileURLToPath(import.meta.url));

export function createApp({ config = loadConfig(), database = null, emailService = null, logger = console } = {}) {
  const db = database || createDatabase(config.databasePath);
  const email = emailService || createEmailService(config, logger);
  const app = express();
  app.disable('x-powered-by');
  app.set('trust proxy', config.production ? 1 : false);
  app.use(express.json({ limit: '24kb' }));
  app.use(securityHeaders(config));

  const publicLimiter = createRateLimiter({ windowMs: 15 * 60 * 1000, max: 120 });
  const bookingLimiter = createRateLimiter({ windowMs: 60 * 60 * 1000, max: 8 });
  const loginLimiter = createRateLimiter({ windowMs: 15 * 60 * 1000, max: 5 });
  app.use('/api', publicLimiter);

  app.get('/health', (_req, res) => res.json({ ok: true }));
  app.get('/api/services', (_req, res) => {
    res.json({ services: Object.entries(SERVICES).map(([id, name]) => ({ id, name })) });
  });

  app.get('/api/availability', (req, res, next) => {
    try {
      const serviceId = String(req.query.service || '');
      assertService(serviceId);
      res.json(buildAvailability(db, serviceId));
    } catch (error) { next(error); }
  });

  app.post('/api/appointments', bookingLimiter, requireSameOrigin(config), async (req, res, next) => {
    try {
      const payload = validateBookingPayload(req.body);
      assertSlotOffered(db, payload.serviceId, payload.startAt);
      const appointment = db.createAppointment(payload);
      res.status(201).json({
        appointment: { id: appointment.id, status: appointment.status, holdExpiresAt: appointment.hold_expires_at }
      });
      email.requestReceived(appointment).catch(error => logger.error('Request email failed', error));
    } catch (error) { next(error); }
  });

  app.post('/api/admin/login', loginLimiter, requireSameOrigin(config), (req, res, next) => {
    try {
      const password = String(req.body?.password || '');
      if (!passwordMatches(password, config.adminPassword, config.sessionSecret)) {
        const error = new Error('Şifre hatalı.');
        error.statusCode = 401;
        error.code = 'UNAUTHORIZED';
        throw error;
      }
      const session = db.createSession();
      setSessionCookie(res, session.token, config);
      res.json({ ok: true, csrfToken: session.csrfToken, expiresAt: session.expiresAt });
    } catch (error) { next(error); }
  });

  app.get('/api/admin/session', requireAdmin(db), (req, res) => {
    res.json({ authenticated: true, csrfToken: req.adminSession.csrf_token, expiresAt: req.adminSession.expires_at });
  });

  app.post('/api/admin/logout', requireAdmin(db), requireCsrf, (req, res) => {
    db.deleteSession(req.adminToken);
    clearSessionCookie(res, config);
    res.json({ ok: true });
  });

  app.get('/api/admin/appointments', requireAdmin(db), (req, res, next) => {
    try {
      const allowedStatuses = ['', 'PENDING', 'APPROVED', 'REJECTED', 'EXPIRED', 'CANCELLED', 'CONFLICT'];
      const status = String(req.query.status || '').toUpperCase();
      if (!allowedStatuses.includes(status)) throw validationError('Geçersiz durum filtresi.');
      res.json({ appointments: db.listAppointments({ status, limit: 200 }) });
    } catch (error) { next(error); }
  });

  app.patch('/api/admin/appointments/:id', requireAdmin(db), requireCsrf, async (req, res, next) => {
    try {
      const action = String(req.body?.action || '');
      const adminNote = normalizeText(req.body?.adminNote, 0, 500, 'Yönetici notu');
      const result = db.decideAppointment(req.params.id, action, adminNote);
      if (result.conflict) return res.status(409).json({ error: { code: 'CONFLICT', message: 'Seçilen saat başka bir kayıtla çakışıyor.' }, appointment: result.appointment });
      res.json({ appointment: result.appointment });
      const method = action === 'approve' ? 'appointmentApproved' : action === 'reject' ? 'appointmentRejected' : 'appointmentCancelled';
      email[method](result.appointment).catch(error => logger.error('Status email failed', error));
    } catch (error) { next(error); }
  });

  app.get('/api/admin/blocks', requireAdmin(db), (req, res, next) => {
    try {
      const now = Date.now();
      const from = req.query.from ? parseTimestamp(req.query.from, 'Başlangıç') : now;
      const to = req.query.to ? parseTimestamp(req.query.to, 'Bitiş') : now + 90 * 86400000;
      if (to <= from || to - from > 366 * 86400000) throw validationError('Kapalı zaman aralığı geçerli değil.');
      res.json({ blocks: db.listBlocks(from, to) });
    } catch (error) { next(error); }
  });

  app.post('/api/admin/blocks', requireAdmin(db), requireCsrf, (req, res, next) => {
    try {
      const startAt = parseTimestamp(req.body?.start, 'Başlangıç');
      const endAt = parseTimestamp(req.body?.end, 'Bitiş');
      if (endAt <= startAt) throw validationError('Bitiş zamanı başlangıçtan sonra olmalıdır.');
      if (endAt - startAt > 31 * 86400000) throw validationError('Tek bir kapalı zaman 31 günden uzun olamaz.');
      const reason = normalizeText(req.body?.reason, 0, 200, 'Açıklama');
      res.status(201).json({ block: db.createBlock({ startAt, endAt, reason }) });
    } catch (error) { next(error); }
  });

  app.delete('/api/admin/blocks/:id', requireAdmin(db), requireCsrf, (req, res, next) => {
    try {
      if (!db.deleteBlock(req.params.id)) {
        const error = new Error('Kapalı zaman kaydı bulunamadı.');
        error.statusCode = 404;
        throw error;
      }
      res.status(204).end();
    } catch (error) { next(error); }
  });

  app.get('/api/admin/availability-slots', requireAdmin(db), (req, res, next) => {
    try {
      const serviceId = String(req.query.service || '');
      assertService(serviceId);
      const from = parseTimestamp(req.query.from, 'Başlangıç');
      const to = parseTimestamp(req.query.to, 'Bitiş');
      if (to <= from || to - from > 32 * 86400000) throw validationError('Takvim aralığı geçerli değil.');
      res.json({ slots: db.listAvailabilitySlots({ serviceId, from, to }) });
    } catch (error) { next(error); }
  });

  app.put('/api/admin/availability-slots', requireAdmin(db), requireCsrf, (req, res, next) => {
    try {
      const serviceId = String(req.body?.serviceId || '');
      assertService(serviceId);
      const startAt = parseTimestamp(req.body?.start, 'Slot zamanı');
      assertSlotGrid(startAt);
      if (startAt < Date.now()) throw validationError('Geçmiş bir saat değiştirilemez.');
      const open = req.body?.open === true;
      const slot = db.setAvailabilitySlot({
        serviceId, startAt, endAt: startAt + BOOKING_RULES.slotMinutes * 60000, open
      });
      res.json({ open: Boolean(slot), slot });
    } catch (error) { next(error); }
  });

  app.use('/assets', express.static(path.join(rootDir, 'assets'), { maxAge: config.production ? '30d' : 0, immutable: config.production }));
  app.use('/galery', express.static(path.join(rootDir, 'galery'), { maxAge: config.production ? '30d' : 0, immutable: config.production }));
  const webRoot = path.join(rootDir, 'dist');
  if (fs.existsSync(webRoot)) {
    app.use('/assets', express.static(path.join(webRoot, 'assets'), { maxAge: config.production ? '1y' : 0, immutable: config.production }));
    const sendWebApp = (_req, res) => {
      res.set('Cache-Control', 'no-cache');
      res.sendFile(path.join(webRoot, 'index.html'));
    };
    app.get('/', sendWebApp);
    app.get('/admin', sendWebApp);
  }

  app.use('/api', (_req, res) => res.status(404).json({ error: { code: 'NOT_FOUND', message: 'API yolu bulunamadı.' } }));
  app.use((error, _req, res, _next) => {
    const status = Number(error.statusCode || 500);
    if (status >= 500) logger.error(error);
    res.status(status).json({ error: { code: error.code || 'INTERNAL_ERROR', message: status >= 500 ? 'Beklenmeyen bir hata oluştu.' : error.message } });
  });

  async function runMaintenance(now = Date.now()) {
    const expired = db.expirePending(now);
    await Promise.allSettled(expired.map(appointment => email.appointmentExpired(appointment)));
    return { expired: expired.length };
  }

  return { app, db, email, runMaintenance };
}

export function buildAvailability(db, serviceId, now = Date.now()) {
  const minStart = now + BOOKING_RULES.minimumNoticeHours * 3600000;
  const horizon = now + BOOKING_RULES.horizonDays * 86400000;
  const localNow = new Date(now + BOOKING_RULES.utcOffsetMinutes * 60000);
  const year = localNow.getUTCFullYear();
  const month = localNow.getUTCMonth();
  const day = localNow.getUTCDate();
  const busy = db.getBusyRanges(now, horizon + 86400000, now);
  const rangeStart = new Date(Date.UTC(year, month, day)).toISOString().slice(0, 10);
  const rangeEnd = new Date(Date.UTC(year, month, day + BOOKING_RULES.horizonDays)).toISOString().slice(0, 10);
  const publishedSlots = db.listAvailabilitySlots({ serviceId, from: now, to: horizon + 86400000 });
  const days = [];

  for (let offset = 0; offset <= BOOKING_RULES.horizonDays; offset++) {
    const localDay = new Date(Date.UTC(year, month, day + offset));
    const dateKey = localDay.toISOString().slice(0, 10);
    const dayStart = Date.UTC(localDay.getUTCFullYear(), localDay.getUTCMonth(), localDay.getUTCDate()) - BOOKING_RULES.utcOffsetMinutes * 60000;
    const dayEnd = dayStart + 86400000;
    const slots = publishedSlots.filter(slot => slot.start_at >= dayStart && slot.start_at < dayEnd).map(slot => {
      const start = slot.start_at;
      const end = slot.end_at;
      const protectedEnd = end + BOOKING_RULES.bufferMinutes * 60000;
      if (start < minStart || start > horizon) return null;
      const overlaps = busy.some(range => range.start < protectedEnd && (range.kind === 'appointment' ? range.end + BOOKING_RULES.bufferMinutes * 60000 : range.end) > start);
      return overlaps ? null : { start: new Date(start).toISOString(), label: formatTime(start) };
    }).filter(Boolean).sort((a, b) => a.start.localeCompare(b.start));
    if (slots.length) days.push({ date: localDay.toISOString().slice(0, 10), label: formatDay(localDay), slots });
  }
  return { timezone: BOOKING_RULES.timezone, generatedAt: new Date(now).toISOString(), rangeStart, rangeEnd, days };
}

function validateBookingPayload(body) {
  if (!body || typeof body !== 'object') throw validationError('Geçersiz form verisi.');
  if (String(body.website || '').trim()) throw validationError('Rezervasyon doğrulanamadı.');
  const startedAt = Number(body.startedAt || 0);
  if (!startedAt || Date.now() - startedAt < 3000) throw validationError('Form çok hızlı gönderildi. Lütfen tekrar deneyin.');
  const serviceId = String(body.serviceId || '');
  assertService(serviceId);
  const name = normalizeText(body.name, 2, 100, 'Ad soyad');
  const email = String(body.email || '').trim().toLowerCase();
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email) || email.length > 160) throw validationError('Geçerli bir e-posta adresi girin.');
  const phone = String(body.phone || '').replace(/[^0-9+]/g, '');
  if (!/^\+?[0-9]{10,15}$/.test(phone)) throw validationError('Geçerli bir telefon numarası girin.');
  const note = normalizeText(body.note, 0, 500, 'Kısa not');
  if (body.consent !== true) throw validationError('Veri kullanım açıklamasını onaylamalısınız.');
  const startAt = parseTimestamp(body.start, 'Randevu saati');
  assertSlot(startAt);
  return { serviceId, serviceName: SERVICES[serviceId], startAt, endAt: startAt + BOOKING_RULES.slotMinutes * 60000, name, email, phone, note };
}

function assertSlot(timestamp, now = Date.now()) {
  if (timestamp % 60000 !== 0) throw validationError('Geçerli bir randevu saati seçin.');
  if (timestamp < now + BOOKING_RULES.minimumNoticeHours * 3600000 || timestamp > now + BOOKING_RULES.horizonDays * 86400000) throw validationError('Seçilen saat rezervasyon aralığının dışında.');
  const local = new Date(timestamp + BOOKING_RULES.utcOffsetMinutes * 60000);
  const minute = local.getUTCMinutes();
  if (minute % BOOKING_RULES.slotStepMinutes !== 0) throw validationError('Seçilen saat geçerli bir zaman dilimi değil.');
}

function assertSlotGrid(timestamp) {
  if (timestamp % 60000 !== 0) throw validationError('Slot zamanı geçerli değil.');
  const local = new Date(timestamp + BOOKING_RULES.utcOffsetMinutes * 60000);
  if (local.getUTCMinutes() % BOOKING_RULES.slotStepMinutes !== 0) throw validationError('Slot 30 dakikalık takvime uygun değil.');
}

function assertSlotOffered(db, serviceId, timestamp) {
  const availability = buildAvailability(db, serviceId);
  const offered = availability.days.some(day => day.slots.some(slot => Date.parse(slot.start) === timestamp));
  if (!offered) throw conflictError('Seçilen saat artık sunulmuyor. Lütfen takvimi yenileyin.');
}

function requireAdmin(db) {
  return (req, res, next) => {
    const token = parseCookies(req.headers.cookie || '').cemox_admin || '';
    const session = db.getSession(token);
    if (!session) return res.status(401).json({ error: { code: 'UNAUTHORIZED', message: 'Oturum açmanız gerekiyor.' } });
    req.adminToken = token;
    req.adminSession = session;
    next();
  };
}

function requireCsrf(req, res, next) {
  if (req.get('x-csrf-token') !== req.adminSession.csrf_token) {
    return res.status(403).json({ error: { code: 'CSRF_ERROR', message: 'Güvenlik doğrulaması başarısız.' } });
  }
  next();
}

function requireSameOrigin(config) {
  return (req, res, next) => {
    const origin = req.get('origin');
    if (origin && origin !== config.appOrigin) return res.status(403).json({ error: { code: 'ORIGIN_ERROR', message: 'İstek kaynağına izin verilmiyor.' } });
    next();
  };
}

function createRateLimiter({ windowMs, max }) {
  const buckets = new Map();
  return (req, res, next) => {
    const now = Date.now();
    const key = `${req.ip}:${req.path}`;
    let bucket = buckets.get(key);
    if (!bucket || bucket.resetAt <= now) bucket = { count: 0, resetAt: now + windowMs };
    bucket.count++;
    buckets.set(key, bucket);
    if (bucket.count > max) return res.status(429).json({ error: { code: 'RATE_LIMITED', message: 'Çok fazla istek gönderildi. Lütfen daha sonra tekrar deneyin.' } });
    if (buckets.size > 5000) for (const [entryKey, value] of buckets) if (value.resetAt <= now) buckets.delete(entryKey);
    next();
  };
}

function securityHeaders(config) {
  return (_req, res, next) => {
    res.set({
      'Content-Security-Policy': "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src https://fonts.gstatic.com; script-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'self'; base-uri 'self'; form-action 'self'",
      'Referrer-Policy': 'strict-origin-when-cross-origin',
      'X-Content-Type-Options': 'nosniff',
      'X-Frame-Options': 'SAMEORIGIN',
      'Permissions-Policy': 'camera=(), microphone=(), geolocation=()',
      ...(config.production ? { 'Strict-Transport-Security': 'max-age=31536000; includeSubDomains' } : {})
    });
    next();
  };
}

function passwordMatches(input, expected, secret) {
  const salt = `cemox-admin:${secret.slice(0, 16)}`;
  const inputHash = scryptSync(input, salt, 64);
  const expectedHash = scryptSync(expected, salt, 64);
  return timingSafeEqual(inputHash, expectedHash);
}

function setSessionCookie(res, token, config) {
  res.cookie('cemox_admin', token, { httpOnly: true, secure: config.production, sameSite: 'strict', maxAge: 12 * 60 * 60 * 1000, path: '/' });
}

function clearSessionCookie(res, config) {
  res.clearCookie('cemox_admin', { httpOnly: true, secure: config.production, sameSite: 'strict', path: '/' });
}

function parseCookies(header) {
  return Object.fromEntries(header.split(';').map(part => part.trim()).filter(Boolean).map(part => {
    const index = part.indexOf('=');
    return [decodeURIComponent(part.slice(0, index)), decodeURIComponent(part.slice(index + 1))];
  }));
}

function parseTimestamp(value, label) {
  const date = new Date(String(value || ''));
  if (Number.isNaN(date.getTime())) throw validationError(`${label} geçerli değil.`);
  return date.getTime();
}

function normalizeText(value, min, max, label) {
  const text = String(value || '').replace(/\s+/g, ' ').trim();
  if (text.length < min || text.length > max) throw validationError(`${label} alanı ${min}–${max} karakter olmalıdır.`);
  return text;
}

function assertService(serviceId) {
  if (!Object.hasOwn(SERVICES, serviceId)) throw validationError('Geçerli bir hizmet seçin.');
}

function formatDay(localDate) {
  return new Intl.DateTimeFormat('tr-TR', { weekday: 'long', day: 'numeric', month: 'long', timeZone: 'UTC' }).format(localDate);
}

function formatTime(timestamp) {
  return new Intl.DateTimeFormat('tr-TR', { hour: '2-digit', minute: '2-digit', timeZone: BOOKING_RULES.timezone }).format(new Date(timestamp));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const envPath = path.join(rootDir, '.env');
  if (fs.existsSync(envPath)) process.loadEnvFile(envPath);
  const config = loadConfig();
  const { app, db, runMaintenance } = createApp({ config });
  const server = app.listen(config.port, () => console.log(`Cemox API listening on http://localhost:${config.port}`));
  const maintenanceTimer = setInterval(() => runMaintenance().catch(error => console.error('Maintenance failed', error)), 15 * 60 * 1000);
  maintenanceTimer.unref();
  const shutdown = () => server.close(() => { clearInterval(maintenanceTimer); db.close(); process.exit(0); });
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}
