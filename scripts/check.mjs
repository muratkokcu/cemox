import fs from 'node:fs';
import assert from 'node:assert/strict';

const serverCode = fs.readFileSync('server.js', 'utf8');
const configCode = fs.readFileSync('src/config.js', 'utf8');
const bookingCode = fs.readFileSync('src/components/BookingScheduler.tsx', 'utf8');
const adminCode = fs.readFileSync('src/pages/AdminPage.tsx', 'utf8');
const entryCode = fs.readFileSync('src/main.tsx', 'utf8');
const indexHtml = fs.readFileSync('index.html', 'utf8');

assert.match(indexHtml, /<html lang="tr">/);
assert.match(entryCode, /<AdminPage \/>/);
assert.match(entryCode, /<HomePage \/>/);
assert.match(bookingCode, /\/api\/appointments/);
assert.match(adminCode, /\/api\/admin\/availability-slots/);

assert.equal([...configCode.matchAll(/^  '[^']+':/gm)].length, 6, 'Altı hizmet yapılandırılmış olmalı.');
assert.match(serverCode, /requireCsrf/);
assert.match(serverCode, /requireSameOrigin/);
assert.match(serverCode, /Cache-Control', 'no-cache/);
assert.match(serverCode, /express\.static\(path\.join\(webRoot, 'assets'\)/);

console.log('React integration checks: OK');
