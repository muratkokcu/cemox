# Cem Avat — Full-Custom Randevu Sistemi

Harici randevu servisi kullanmadan çalışan React, TypeScript ve SQLite tabanlı web uygulaması.
Backend Rust'a (axum + rusqlite) taşınmaktadır; geçiş süresince Node/Express sürümü de yerinde durur
ve iki backend aynı veritabanı üzerinde birebir değiştirilebilir.

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
npm run dev        # Node backend + Vite
npm run dev:rust   # Rust backend + Vite
```

Rust backend için ayrıca [Rust toolchain](https://rustup.rs) (1.90+) gerekir.

Vite geliştirme arayüzü: `http://localhost:5100`

Yönetim paneli: `http://localhost:5100/admin`

Express API geliştirme sırasında `http://localhost:4100` adresinde çalışır ve Vite tarafından proxy’lenir. Üretim derlemesinde hem site hem API `4100` portundan sunulur.

`.env` içinde özellikle şu değerleri değiştirin:

- `ADMIN_PASSWORD`: en az 12 karakterli güçlü yönetici şifresi
- `SESSION_SECRET`: en az 32 karakterli rastgele değer
- `APP_ORIGIN`: üretimde sitenin HTTPS adresi
- `SMTP_*`: e-posta sağlayıcısının SMTP bilgileri

SMTP tanımlanmadan development ortamında randevu işlemleri çalışır; e-posta gönderimleri maskelenmiş biçimde konsola yazılır.

## Backend'ler

| | Node (mevcut) | Rust (yeni) |
| --- | --- | --- |
| Kaynak | `server.js`, `src/db.js`, `src/email.js`, `src/config.js` | `server/src/` |
| Geliştirme | `npm run dev` | `npm run dev:rust` |
| Derleme | — | `npm run build:rs` |
| Çalıştırma | `npm start` | `npm run start:rs` |
| Test | `npm test` | `npm run test:rs` |
| Docker | `Dockerfile` | `Dockerfile.rust` |

İkisi de aynı API sözleşmesini ve aynı SQLite şemasını kullanır; `DATABASE_PATH` aynı dosyayı
gösterdiğinde oturumlar, randevular ve müsaitlikler karşılıklı geçerlidir. Rust sürümü iki noktada
bilinçli olarak farklıdır ve her ikisinde de daha doğrudur:

- Bulunamayan kapalı zaman silinirken hata kodu `NOT_FOUND` döner (Node `INTERNAL_ERROR` döndürüyordu).
- Bozuk JSON gövdesinde ayrıştırıcının iç mesajı sızdırılmaz; `Geçersiz form verisi.` döner.

Rust backend'i kök dizinden çalıştırın: statik dosyalar (`assets/`, `galery/`, `dist/`) çalışma
dizinine göre çözülür, gerekirse `APP_ROOT` ile geçersiz kılınabilir.

## Test ve kontroller

```bash
npm run check      # TypeScript + Node backend söz dizimi
npm test           # Node backend testleri
npm run test:rs    # Rust backend testleri
npm run check:rs   # cargo fmt --check + clippy
npm run build
npm audit
```

## Docker ile çalıştırma

```bash
cp .env.example .env
docker compose up -d --build
```

Rust backend imajı için:

```bash
docker build -f Dockerfile.rust -t cemox-rust .
```

SQLite verisi `cemox-data` volume’unda saklanır. Üretimde uygulamanın önüne HTTPS sağlayan bir reverse proxy yerleştirin ve volume’u düzenli yedekleyin.

## Randevu kuralları

Randevu süresi, tampon süre, minimum bildirim ve rezervasyon ufku `BOOKING_RULES` üzerinden yönetilir.
Hizmet adları aynı yapılandırmadaki `SERVICES` listesindedir; branşların açık saatleri admin takviminden belirlenir.

Kurallar her iki backend'de de tanımlıdır ve **birlikte güncellenmelidir**:

- Node: [src/config.js](src/config.js)
- Rust: [server/src/config.rs](server/src/config.rs)
