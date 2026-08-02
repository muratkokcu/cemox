import fs from 'node:fs';
import assert from 'node:assert/strict';

const serverCode = fs.readFileSync('server.js', 'utf8');
const configCode = fs.readFileSync('src/config.js', 'utf8');
const bookingCode = fs.readFileSync('public/booking.js', 'utf8');
new Function(bookingCode);

for (const file of ['cem-avat-website.html', 'admin.html']) {
  const html = fs.readFileSync(file, 'utf8');
  for (const match of html.matchAll(/<script>([\s\S]*?)<\/script>/g)) new Function(match[1]);
  assert.match(html, /<html lang="tr">/);
}

const site = fs.readFileSync('cem-avat-website.html', 'utf8');
const serviceIds = [...site.matchAll(/data-service="([^"]+)"/g)].map(match => match[1]);
assert.equal(serviceIds.length, 6, 'Sitede altı hizmet butonu bulunmalı.');
assert.equal(new Set(serviceIds).size, 6, 'Hizmet kimlikleri benzersiz olmalı.');
for (const id of serviceIds) assert.match(configCode, new RegExp(`'${id}'\\s*:`));
assert.match(serverCode, /requireCsrf/);
assert.match(serverCode, /requireSameOrigin/);

console.log('Static integration checks: OK');
