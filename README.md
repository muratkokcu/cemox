# Cem Avat — Full-Custom Randevu Sistemi

Harici randevu servisi kullanmadan çalışan Node.js, SQLite ve özel yönetim paneli tabanlı web uygulaması.

## Özellikler

- Altı hizmet için 20 dakikalık telefon ön görüşmesi
- Hafta içi 10:00–18:00, 24 saat minimum bildirim, 30 günlük rezervasyon penceresi
- Talep geldiğinde saati 24 saat tutma ve eşzamanlı rezervasyon koruması
- Şifreli yönetim panelinden onay, ret ve iptal
- Yönetim panelinden tarih/saat aralığı kapatma
- SMTP üzerinden talep ve durum e-postaları
- SQLite transaction, admin session, CSRF, origin kontrolü ve rate limiting

## Yerel kurulum

Node.js 24 veya üzeri gereklidir.

```bash
npm install
cp .env.example .env
npm run dev
```

Site: `http://localhost:4100`

Yönetim paneli: `http://localhost:4100/admin`

`.env` içinde özellikle şu değerleri değiştirin:

- `ADMIN_PASSWORD`: en az 12 karakterli güçlü yönetici şifresi
- `SESSION_SECRET`: en az 32 karakterli rastgele değer
- `APP_ORIGIN`: üretimde sitenin HTTPS adresi
- `SMTP_*`: e-posta sağlayıcısının SMTP bilgileri

SMTP tanımlanmadan development ortamında randevu işlemleri çalışır; e-posta gönderimleri maskelenmiş biçimde konsola yazılır.

## Test ve kontroller

```bash
npm run check
npm test
npm audit
```

## Docker ile çalıştırma

```bash
cp .env.example .env
docker compose up -d --build
```

SQLite verisi `cemox-data` volume’unda saklanır. Üretimde uygulamanın önüne HTTPS sağlayan bir reverse proxy yerleştirin ve volume’u düzenli yedekleyin.

## Randevu kuralları

Kurallar [src/config.js](src/config.js) içindeki `BOOKING_RULES` üzerinden yönetilir. Hizmet adları aynı dosyadaki `SERVICES` yapılandırmasındadır.
