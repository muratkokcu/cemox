import path from 'node:path';

export const SERVICES = Object.freeze({
  'medical-fitness': 'Medical Fitness',
  'kisisel-antrenman': 'Kişisel Antrenman',
  'fonksiyonel-antrenman': 'Fonksiyonel Antrenman',
  'performans-gelistirme': 'Performans Geliştirme',
  'kurek-antrenorlugu': 'Kürek Antrenörlüğü',
  'korektif-egzersiz': 'Korektif Egzersiz Yaklaşımı'
});

export const BOOKING_RULES = Object.freeze({
  timezone: 'Europe/Istanbul',
  utcOffsetMinutes: 180,
  slotMinutes: 20,
  bufferMinutes: 10,
  slotStepMinutes: 30,
  minimumNoticeHours: 24,
  horizonDays: 30,
  holdHours: 24
});

export function loadConfig(env = process.env) {
  const production = env.NODE_ENV === 'production';
  const adminPassword = env.ADMIN_PASSWORD || (production ? '' : 'development-password-change-me');
  const sessionSecret = env.SESSION_SECRET || (production ? '' : 'development-session-secret-change-me-now');

  if (adminPassword.length < 12) throw new Error('ADMIN_PASSWORD en az 12 karakter olmalıdır.');
  if (sessionSecret.length < 32) throw new Error('SESSION_SECRET en az 32 karakter olmalıdır.');

  return {
    env: env.NODE_ENV || 'development',
    production,
    port: Number(env.PORT || 4100),
    appOrigin: String(env.APP_ORIGIN || (production ? '' : 'http://localhost:5100')).replace(/\/$/, ''),
    databasePath: path.resolve(env.DATABASE_PATH || './data/appointments.sqlite'),
    adminEmail: env.ADMIN_EMAIL || 'cemavat@gmail.com',
    adminPassword,
    sessionSecret,
    smtp: {
      host: env.SMTP_HOST || '',
      port: Number(env.SMTP_PORT || 587),
      secure: String(env.SMTP_SECURE || 'false') === 'true',
      user: env.SMTP_USER || '',
      pass: env.SMTP_PASS || '',
      from: env.SMTP_FROM || 'Cem Avat Randevu <randevu@localhost>'
    }
  };
}
