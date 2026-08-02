# Cem Avat — Full-Custom Randevu Sistemi

Harici randevu servisi kullanmadan çalışan React, TypeScript, Express ve SQLite tabanlı web uygulaması.

## Özellikler

- Altı hizmet için 20 dakikalık telefon ön görüşmesi
- Cal.com benzeri ay takvimi ve seçili güne ait dikey saat listesi
- Admin takviminde branş bazında 30 dakikalık saatleri açık/kapalı yapma
- React 19, Vite ve Tailwind CSS ile responsive kullanıcı ve yönetim ekranları
- Seçilen saati 24 saat tutan yönetici onay akışı ve eşzamanlı çakışma koruması
- Şifreli yönetim panelinden onay, ret ve iptal
- Yönetim panelinden yayınlanmış müsaitliklere ek olarak istisnai tarih/saat kapatma
- SMTP üzerinden rezervasyon ve durum e-postaları
- SQLite transaction, admin session, CSRF, origin kontrolü ve rate limiting

## Yerel kurulum

Node.js 24 veya üzeri gereklidir.

```bash
npm install
cp .env.example .env
npm run dev
```

Vite geliştirme arayüzü: `http://localhost:5173`

Yönetim paneli: `http://localhost:5173/admin`

Express API geliştirme sırasında `http://localhost:4100` adresinde çalışır ve Vite tarafından proxy’lenir. Üretim derlemesinde hem site hem API `4100` portundan sunulur.

`.env` içinde özellikle şu değerleri değiştirin:

- `ADMIN_PASSWORD`: en az 12 karakterli güçlü yönetici şifresi
- `SESSION_SECRET`: en az 32 karakterli rastgele değer
- `APP_ORIGIN`: üretimde sitenin HTTPS adresi
- `SMTP_*`: e-posta sağlayıcısının SMTP bilgileri

SMTP tanımlanmadan development ortamında randevu işlemleri çalışır; e-posta gönderimleri maskelenmiş biçimde konsola yazılır.

## Test ve kontroller

```bash
npm run check
npm run build
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

Randevu süresi, tampon süre, minimum bildirim ve rezervasyon ufku [src/config.js](src/config.js) içindeki `BOOKING_RULES` üzerinden yönetilir. Hizmet adları aynı dosyadaki `SERVICES` yapılandırmasındadır; branşların açık saatleri admin takviminden belirlenir.
