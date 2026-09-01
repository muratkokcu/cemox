# Cem Avat — Full-Custom Randevu Sistemi

Harici randevu servisi kullanmadan çalışan web uygulaması: React + TypeScript ön yüz,
Rust (axum + rusqlite) API, SQLite veritabanı.

## Özellikler

- Altı hizmet için 20 dakikalık telefon ön görüşmesi
- Cal.com benzeri ay takvimi ve seçili güne ait dikey saat listesi
- Admin takviminde branş bazında 30 dakikalık saatleri açık/kapalı yapma
- Günün tamamını tek işlemde açma/kapatma, Shift ile aralık seçimi ve günü hafta günlerine kopyalama
- React 19, Vite ve Tailwind CSS ile responsive kullanıcı ve yönetim ekranları
- Seçilen saati 24 saat tutan yönetici onay akışı ve eşzamanlı çakışma koruması
- Şifreli yönetim panelinden onay, ret ve iptal
- Yönetim panelinden tatil/izin için tarih ve saat kapatma
- Sunucu tarafında filtrelenen ve sayfalanan randevu listesi
- SMTP üzerinden rezervasyon ve durum e-postaları
- SQLite transaction, admin session, CSRF, origin kontrolü ve rate limiting

## Yerel kurulum

Ön yüz derlemesi için Node.js 24+, API için [Rust](https://rustup.rs) 1.90+ gereklidir.

```bash
npm install
cp .env.example .env
npm run dev
```

`npm run dev` API'yi ve Vite'ı birlikte çalıştırır. Rust kodunu değiştirdiğinizde API'yi
yeniden başlatmak gerekir; otomatik yeniden başlatma için `cargo install cargo-watch`
kurup `dev:api` betiğini `cargo watch -x run` ile değiştirebilirsiniz.

Vite geliştirme arayüzü: `http://localhost:5100`

Yönetim paneli: `http://localhost:5100/admin`

API geliştirme sırasında `http://localhost:4100` adresinde çalışır ve Vite tarafından proxy’lenir. Üretim derlemesinde hem site hem API `4100` portundan sunulur.

`.env` içinde özellikle şu değerleri değiştirin:

- `ADMIN_PASSWORD`: en az 12 karakterli güçlü yönetici şifresi
- `SESSION_SECRET`: en az 32 karakterli rastgele değer
- `APP_ORIGIN`: üretimde sitenin HTTPS adresi
- `SMTP_*`: e-posta sağlayıcısının SMTP bilgileri

SMTP tanımlanmadan development ortamında randevu işlemleri çalışır; e-posta gönderimleri maskelenmiş biçimde konsola yazılır.

## Proje düzeni

| Yol | İçerik |
| --- | --- |
| `src/` | React + TypeScript ön yüz |
| `server/src/` | Rust API (axum + rusqlite) |
| `server/tests/` | API bütünleşik testleri |
| `scripts/check.mjs` | Ön yüz ile API sözleşmesinin bağlı kaldığını doğrular |

API'yi kök dizinden çalıştırın: statik dosyalar (`assets/`, `galery/`, `dist/`) çalışma
dizinine göre çözülür, gerekirse `APP_ROOT` ile geçersiz kılınabilir.

## Test ve kontroller

```bash
npm run check   # tsc + cargo fmt --check + clippy + entegrasyon kontrolleri
npm test        # API testleri
npm run build   # ön yüz + release API ikilisi
npm audit
```

## Docker ile çalıştırma

```bash
cp .env.example .env
docker compose up -d --build
```

İmaj çok aşamalıdır: Rust API'si ve ön yüz derlenir, sonuç `debian:bookworm-slim`
üzerine kopyalanır. SQLite ikiliye gömülüdür, ek sistem paketi gerekmez.

SQLite verisi `cemox-data` volume’unda saklanır. Üretimde uygulamanın önüne HTTPS sağlayan bir reverse proxy yerleştirin ve volume’u düzenli yedekleyin.

## Randevu kuralları

Randevu süresi, tampon süre, minimum bildirim ve rezervasyon ufku
[server/src/config.rs](server/src/config.rs) içindeki `BOOKING_RULES` üzerinden yönetilir.
Hizmet adları aynı dosyadaki `SERVICES` listesindedir; branşların açık saatleri admin
takviminden belirlenir.
