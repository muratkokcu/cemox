import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { createApp } from '../server.js';
import { createDatabase } from '../src/db.js';
import { loadConfig } from '../src/config.js';

function createEmailStub() {
  return {
    requestReceived: async () => {},
    appointmentApproved: async () => {},
    appointmentRejected: async () => {},
    appointmentCancelled: async () => {},
    appointmentExpired: async () => {}
  };
}

async function createTestServer() {
  const config = loadConfig({
    NODE_ENV: 'test',
    APP_ORIGIN: 'http://127.0.0.1',
    DATABASE_PATH: ':memory:',
    ADMIN_EMAIL: 'admin@example.com',
    ADMIN_PASSWORD: 'test-admin-password',
    SESSION_SECRET: 'test-session-secret-at-least-32-characters'
  });
  const database = createDatabase(':memory:');
  const localNow = new Date(Date.now() + 180 * 60000);
  const dateFrom = localNow.toISOString().slice(0, 10);
  const dateTo = new Date(localNow.getTime() + 30 * 86400000).toISOString().slice(0, 10);
  for (const serviceId of ['medical-fitness', 'kisisel-antrenman']) {
    database.createAvailabilityRule({
      serviceId, dateFrom, dateTo, weekdays: [1, 2, 3, 4, 5], startMinute: 600, endMinute: 1080
    });
  }
  const { app } = createApp({ config, database, emailService: createEmailStub(), logger: { info() {}, error() {} } });
  const server = app.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    database,
    close: async () => {
      server.close();
      await once(server, 'close');
      database.close();
    }
  };
}

async function jsonRequest(baseUrl, route, options = {}) {
  const response = await fetch(baseUrl + route, {
    ...options,
    headers: { 'Content-Type': 'application/json', ...(options.headers || {}) }
  });
  const body = response.status === 204 ? null : await response.json();
  return { response, body };
}

test('public booking and admin approval flow', async t => {
  const runtime = await createTestServer();
  t.after(runtime.close);

  const services = await jsonRequest(runtime.baseUrl, '/api/services');
  assert.equal(services.response.status, 200);
  assert.equal(services.body.services.length, 6);

  const availability = await jsonRequest(runtime.baseUrl, '/api/availability?service=medical-fitness');
  assert.equal(availability.response.status, 200);
  assert.ok(availability.body.days.length > 0);
  const slot = availability.body.days[0].slots[0].start;

  const bookingPayload = {
    serviceId: 'medical-fitness',
    start: slot,
    name: 'Test Kullanıcı',
    email: 'test@example.com',
    phone: '+905551112233',
    note: 'Hareket kalitesi hakkında görüşmek istiyorum.',
    consent: true,
    website: '',
    startedAt: Date.now() - 5000
  };
  const created = await jsonRequest(runtime.baseUrl, '/api/appointments', { method: 'POST', body: JSON.stringify(bookingPayload) });
  assert.equal(created.response.status, 201);
  assert.equal(created.body.appointment.status, 'PENDING');
  const appointmentId = created.body.appointment.id;

  const duplicateContact = await jsonRequest(runtime.baseUrl, '/api/appointments', {
    method: 'POST', body: JSON.stringify({ ...bookingPayload, start: availability.body.days[0].slots[1].start })
  });
  assert.equal(duplicateContact.response.status, 409);

  const duplicateSlot = await jsonRequest(runtime.baseUrl, '/api/appointments', {
    method: 'POST', body: JSON.stringify({ ...bookingPayload, email: 'other@example.com', phone: '+905559998877' })
  });
  assert.equal(duplicateSlot.response.status, 409);

  const unauthorized = await jsonRequest(runtime.baseUrl, '/api/admin/appointments');
  assert.equal(unauthorized.response.status, 401);

  const wrongLogin = await jsonRequest(runtime.baseUrl, '/api/admin/login', { method: 'POST', body: JSON.stringify({ password: 'wrong-password' }) });
  assert.equal(wrongLogin.response.status, 401);
  assert.equal(wrongLogin.body.error.message, 'Şifre hatalı.');

  const login = await jsonRequest(runtime.baseUrl, '/api/admin/login', { method: 'POST', body: JSON.stringify({ password: 'test-admin-password' }) });
  assert.equal(login.response.status, 200);
  const cookie = login.response.headers.get('set-cookie').split(';')[0];
  const csrf = login.body.csrfToken;

  const csrfFailure = await jsonRequest(runtime.baseUrl, `/api/admin/appointments/${appointmentId}`, {
    method: 'PATCH', headers: { Cookie: cookie }, body: JSON.stringify({ action: 'approve', adminNote: '' })
  });
  assert.equal(csrfFailure.response.status, 403);

  const approved = await jsonRequest(runtime.baseUrl, `/api/admin/appointments/${appointmentId}`, {
    method: 'PATCH', headers: { Cookie: cookie, 'x-csrf-token': csrf }, body: JSON.stringify({ action: 'approve', adminNote: '' })
  });
  assert.equal(approved.response.status, 200);
  assert.equal(approved.body.appointment.status, 'APPROVED');

  const refreshed = await jsonRequest(runtime.baseUrl, '/api/availability?service=medical-fitness');
  const allSlots = refreshed.body.days.flatMap(day => day.slots.map(item => item.start));
  assert.equal(allSlots.includes(slot), false);
});

test('admin can add and remove an availability block', async t => {
  const runtime = await createTestServer();
  t.after(runtime.close);
  const availability = await jsonRequest(runtime.baseUrl, '/api/availability?service=kisisel-antrenman');
  const start = availability.body.days[0].slots[0].start;
  const end = new Date(new Date(start).getTime() + 60 * 60000).toISOString();

  const login = await jsonRequest(runtime.baseUrl, '/api/admin/login', { method: 'POST', body: JSON.stringify({ password: 'test-admin-password' }) });
  const headers = { Cookie: login.response.headers.get('set-cookie').split(';')[0], 'x-csrf-token': login.body.csrfToken };
  const created = await jsonRequest(runtime.baseUrl, '/api/admin/blocks', {
    method: 'POST', headers, body: JSON.stringify({ start, end, reason: 'Test engeli' })
  });
  assert.equal(created.response.status, 201);

  const refreshed = await jsonRequest(runtime.baseUrl, '/api/availability?service=kisisel-antrenman');
  assert.equal(refreshed.body.days.flatMap(day => day.slots.map(item => item.start)).includes(start), false);

  const removed = await jsonRequest(runtime.baseUrl, `/api/admin/blocks/${created.body.block.id}`, { method: 'DELETE', headers });
  assert.equal(removed.response.status, 204);
});

test('admin publishes service-specific availability rules', async t => {
  const runtime = await createTestServer();
  t.after(runtime.close);
  const before = await jsonRequest(runtime.baseUrl, '/api/availability?service=fonksiyonel-antrenman');
  assert.equal(before.body.days.length, 0);

  const login = await jsonRequest(runtime.baseUrl, '/api/admin/login', { method: 'POST', body: JSON.stringify({ password: 'test-admin-password' }) });
  const headers = { Cookie: login.response.headers.get('set-cookie').split(';')[0], 'x-csrf-token': login.body.csrfToken };
  const localNow = new Date(Date.now() + 180 * 60000);
  const dateFrom = localNow.toISOString().slice(0, 10);
  const dateTo = new Date(localNow.getTime() + 30 * 86400000).toISOString().slice(0, 10);
  const created = await jsonRequest(runtime.baseUrl, '/api/admin/availability-rules', {
    method: 'POST', headers, body: JSON.stringify({
      serviceId: 'fonksiyonel-antrenman', dateFrom, dateTo,
      weekdays: [1, 3, 5], startTime: '10:00', endTime: '14:00'
    })
  });
  assert.equal(created.response.status, 201);

  const after = await jsonRequest(runtime.baseUrl, '/api/availability?service=fonksiyonel-antrenman');
  assert.ok(after.body.days.length > 0);
  assert.ok(after.body.days.every(day => [1, 3, 5].includes(new Date(day.date + 'T00:00:00Z').getUTCDay())));

  const removed = await jsonRequest(runtime.baseUrl, `/api/admin/availability-rules/${created.body.rule.id}`, { method: 'DELETE', headers });
  assert.equal(removed.response.status, 204);
  const emptyAgain = await jsonRequest(runtime.baseUrl, '/api/availability?service=fonksiyonel-antrenman');
  assert.equal(emptyAgain.body.days.length, 0);
});

test('pending holds expire after 24 hours', () => {
  const database = createDatabase(':memory:');
  try {
    const now = Date.now();
    const appointment = database.createAppointment({
      serviceId: 'medical-fitness', serviceName: 'Medical Fitness',
      startAt: now + 48 * 3600000, endAt: now + 48 * 3600000 + 20 * 60000,
      name: 'Süre Testi', email: 'expiry@example.com', phone: '+905551234567', note: ''
    }, now);
    assert.equal(appointment.status, 'PENDING');
    const expired = database.expirePending(now + 24 * 3600000 + 1);
    assert.equal(expired.length, 1);
    assert.equal(database.getAppointment(appointment.id).status, 'EXPIRED');
  } finally {
    database.close();
  }
});
