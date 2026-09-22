// Ön yüz ile Rust API'sinin birbirine bağlı kaldığını doğrular.
// Derleyicinin yakalayamadığı sözleşmeler burada sabitlenir.
import fs from 'node:fs';
import assert from 'node:assert/strict';

const read = path => fs.readFileSync(path, 'utf8');

const indexHtml = read('index.html');
const entryCode = read('src/main.tsx');
const bookingCode = read('src/components/BookingScheduler.tsx');
const adminCode = read('src/pages/AdminPage.tsx');
const configRs = read('server/src/config.rs');
const appRs = read('server/src/app.rs');

assert.match(indexHtml, /<html lang="tr">/);
assert.match(entryCode, /<AdminPage \/>/);
assert.match(entryCode, /<HomePage \/>/);

// Ön yüzün çağırdığı uçlar sunucuda tanımlı olmalı.
const routes = [
  ['/api/appointments', bookingCode],
  ['/api/admin/availability-slots', adminCode],
  ['/api/admin/availability-slots/bulk', adminCode],
  ['/api/admin/appointments', adminCode],
  ['/api/admin/blocks', adminCode]
];
for (const [route, clientCode] of routes) {
  assert.ok(clientCode.includes(route), `Ön yüz ${route} çağırmalı`);
  assert.ok(appRs.includes(`"${route}"`), `Sunucuda ${route} rotası tanımlı olmalı`);
}

assert.equal(
  [...configRs.matchAll(/^ {4}\("[a-z-]+", "/gm)].length, 6,
  'Altı hizmet yapılandırılmış olmalı.'
);

// Güvenlik katmanları yerinde mi?
assert.match(appRs, /fn require_admin/);
assert.match(appRs, /x-csrf-token/);
assert.match(appRs, /fn require_same_origin/);
assert.match(appRs, /Content-Security-Policy|CONTENT_SECURITY_POLICY/);

// SPA ve statik sunum.
assert.match(appRs, /"no-cache"/);
assert.match(appRs, /ServeDir::new\(dist\.join\("assets"\)\)/);

// Randevu kuralları yalnızca Rust tarafında tanımlı olmalı (Node sürümü kaldırıldı).
assert.ok(!fs.existsSync('src/config.js'), 'src/config.js kaldırılmış olmalı');
assert.ok(!fs.existsSync('server.js'), 'server.js kaldırılmış olmalı');

console.log('React + Rust integration checks: OK');
