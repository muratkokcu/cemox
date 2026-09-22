import { useEffect, useState } from 'react';
import { BookingScheduler } from '../components/BookingScheduler';

const services = [
  { id: 'medical-fitness', name: 'Medical Fitness', description: 'Hareket kalitesini desteklemeye, fiziksel kapasiteyi artırmaya ve egzersize kontrollü şekilde uyum sağlamaya yönelik antrenman planları.' },
  { id: 'kisisel-antrenman', name: 'Kişisel Antrenman', description: 'Bireysel hedeflere, kondisyon seviyesine ve yaşam düzenine göre yapılandırılan fitness programları. Güç, dayanıklılık ve genel fiziksel form odağı.' },
  { id: 'fonksiyonel-antrenman', name: 'Fonksiyonel Antrenman', description: 'Denge, koordinasyon, mobilite ve temel kuvvet gelişimini destekleyen; günlük yaşam ve sportif ihtiyaçlara uyarlanmış egzersiz uygulamaları.' },
  { id: 'performans-gelistirme', name: 'Performans Geliştirme', description: 'Sporcular için fiziksel hazırlık, kuvvet gelişimi ve performans odaklı antrenman planlaması. Branşa ve hedefe göre yapılandırılmış çalışma yaklaşımı.' },
  { id: 'kurek-antrenorlugu', name: 'Kürek Antrenörlüğü', description: 'Kürek branşına yönelik kondisyon, kuvvet ve destekleyici kara antrenmanı planlamaları. Sporcu geçmişi ve kulüp deneyimiyle yapılandırılmış antrenörlük yaklaşımı.' },
  { id: 'korektif-egzersiz', name: 'Korektif Egzersiz Yaklaşımı', description: 'Doğru hareket alışkanlığını destekleyen, mobilite ve dengeyi geliştirmeye odaklanan egzersiz uygulamalarıyla antrenman kalitesini güçlendirme.' }
];

const gallery = ['6619', '6720', '6749', '6773', '6885', '6941', '6965', '7051', '7167', '7314', '7317', '7327', '7331', '7333', '7380', '7462', '7468', '7492']
  .map(code => `/galery/1E7A${code}.webp`);

const achievements = [
  ['2020', '🥇 Birincilik', 'Büyükler Türkiye Kupası ve Akademi Şampiyonası', 'U23 Hafif Kilo Erkekler'],
  ['2018', '🥇 Birincilik', 'Balkan Şampiyonası', 'Genç Erkekler — Uluslararası'],
  ['2018', '🥇 Birincilik', 'Gençler Türkiye Şampiyonası & Federasyon Kupası', 'Genç Erkekler'],
  ['2017', '🥇 Birincilik', 'Gençler Türkiye Şampiyonası', 'Genç Erkekler'],
  ['2016', '🥉 Üçüncülük', 'Balkan Şampiyonası', 'Yıldız Erkekler — Uluslararası']
];

export function HomePage() {
  const [scrolled, setScrolled] = useState(false);
  const [preferredService, setPreferredService] = useState('');
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);

  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 60);
    onScroll();
    window.addEventListener('scroll', onScroll, { passive: true });
    return () => window.removeEventListener('scroll', onScroll);
  }, []);

  useEffect(() => {
    if (lightboxIndex === null) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setLightboxIndex(null);
      if (event.key === 'ArrowLeft') setLightboxIndex(index => index === null ? null : (index - 1 + gallery.length) % gallery.length);
      if (event.key === 'ArrowRight') setLightboxIndex(index => index === null ? null : (index + 1) % gallery.length);
    };
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    document.addEventListener('keydown', onKey);
    return () => { document.body.style.overflow = previousOverflow; document.removeEventListener('keydown', onKey); };
  }, [lightboxIndex]);

  function book(serviceId = '') {
    if (serviceId) setPreferredService(serviceId);
    document.getElementById('randevu')?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }

  return (
    <div className="legacy-site">
      <nav className={`site-nav ${scrolled ? 'scrolled' : ''}`}>
        <a className="nav-logo" href="#top">CEM<span>.</span>AVAT</a>
        <ul className="nav-links">
          <li><a href="#about">Hakkımda</a></li>
          <li><a href="#services">Hizmetler</a></li>
          <li><a href="#credentials">Yetkinlikler</a></li>
          <li><a href="#achievements">Başarılar</a></li>
          <li><a href="#gallery">Galeri</a></li>
          <li><a href="#contact">İletişim</a></li>
        </ul>
        <button type="button" className="nav-cta" onClick={() => book()}>Antrenman Saati Seç</button>
      </nav>

      <main id="top">
        <section className="hero">
          <div className="hero-content">
            <div className="hero-tag">Fitness • Kürek • Medical Fitness</div>
            <h1>Fitness, Kürek ve Medical Fitness Alanlarında<br /><span>Uzmanlaşmış Antrenör.</span></h1>
            <p className="hero-desc">Cem Avat, bireylerin hareket kalitesini, fiziksel kapasitesini ve performansını geliştirmeye yönelik; fitness, kürek ve medical fitness odaklı antrenman programları sunar.</p>
            <div className="hero-stats">
              <div className="hero-stat"><div className="num">12+</div><div className="label">Yıl Deneyim</div></div>
              <div className="hero-stat"><div className="num">16</div><div className="label">Ulusal Birincilik</div></div>
              <div className="hero-stat"><div className="num">4</div><div className="label">Uluslararası Madalya</div></div>
            </div>
          </div>
          <div className="hero-visual">
            <div className="hero-portrait"><img src="/assets/cem-hero.webp" alt="Cem Avat — Fitness, Kürek ve Medical Fitness Antrenörü" /></div>
            <div className="hero-badge"><div className="label">Fenerbahçe S.K.</div><div className="title">Fitness ve Kürek Antrenörü</div><div className="meta">2022 — Günümüz</div></div>
          </div>
        </section>

        <section id="about">
          <div className="section-tag">Hakkımda</div>
          <div className="section-title">Antrenörlükte disiplin, hareket kalitesi ve performans odağı.</div>
          <div className="about-grid">
            <div className="about-text">
              <p>Cem Avat, fitness, kürek ve medical fitness alanlarında uzmanlaşmış bir antrenör olarak; bireylerin güç, dayanıklılık, hareket kalitesi ve fiziksel performanslarını geliştirmeye yönelik programlar hazırlar. Sporculuk geçmişi ile antrenörlük deneyimini aynı çizgide birleştirerek hem bireysel hem kurumsal düzeyde çalışma yürütür.</p>
              <p>Çalışma yaklaşımında amaç; kişiye uygun egzersiz planlaması, sürdürülebilir gelişim ve doğru hareket alışkanlığı oluşturmaktır. Genel fitness hedeflerinden performans gelişimine, aktif yaşama geçiş sürecinden hareket kalitesini destekleyen antrenmanlara kadar farklı ihtiyaçlara antrenör bakış açısıyla çözüm üretir.</p>
            </div>
            <div className="about-values">
              <div className="about-value"><h4>Sorumluluk Bilinci</h4><p>Hem bireysel danışanlarla hem de ekip içinde verimli ve uyumlu bir şekilde çalışarak, bulunduğum ortama katkı sağlamayı öncelik olarak görüyorum.</p></div>
              <div className="about-value"><h4>Sürekli Gelişim</h4><p>Güncel bilimsel çalışmalar ve yeni yaklaşımlar doğrultusunda bilgi birikimimi artırmaya devam ediyorum.</p></div>
              <div className="about-value"><h4>Sürdürülebilir Gelişim</h4><p>İnsanların hareket kapasitesini, performansını ve egzersiz alışkanlıklarını uzun vadede güçlendiren çalışmalar üretmek.</p></div>
            </div>
          </div>
        </section>

        <section id="services" className="services-section">
          <div className="section-tag">Hizmetler</div><div className="section-title">Uzmanlık Alanları</div>
          <div className="services-grid">
            {services.map((service, index) => <article className="service-card" key={service.id}><div className="service-num">{String(index + 1).padStart(2, '0')}</div><h3>{service.name}</h3><p>{service.description}</p><button type="button" className="service-booking-btn" onClick={() => book(service.id)}>Saat seç →</button></article>)}
          </div>
        </section>

        <section className="booking-section" aria-labelledby="booking-section-title">
          <div className="section-intro"><div><div className="section-tag">Antrenman Takvimi</div><h2 id="booking-section-title">Antrenman saatinizi<br />doğrudan seçin.</h2></div><p>Yalnızca antrenörün branşa özel açtığı saatler görünür. Seçtiğiniz saat çakışmalara karşı tutulur ve onaya gönderilir.</p></div>
          <BookingScheduler preferredServiceId={preferredService} />
        </section>

        <section id="credentials">
          <div className="section-tag">Yetkinlikler</div><div className="section-title">Eğitim & Sertifikalar</div>
          <div className="cred-grid">
            <div className="cred-block"><h3>Akademik & Deneyim</h3><Credential name="Beden Eğitimi ve Spor Öğretmenliği — Lisans" org="Marmara Üniversitesi" meta="2018–2022" /><Credential name="Fitness Antrenörü" org="Fenerbahçe Spor Kulübü" meta="2022–Günümüz" /><Credential name="Kürek Antrenörü" org="Fenerbahçe Spor Kulübü" meta="2019–2022" /></div>
            <div className="cred-block"><h3>Sertifikalar</h3><Credential name="EQF Level 4 — Personal Trainer" org="TREPS / EREPS" meta="Aktif" /><Credential name="Functional Training" org="TREPS / EREPS" meta="Aktif" /><Credential name="Corrective Exercise Specialist" org="FMI" meta="Aktif" /><Credential name="1. & 2. Kademe Kürek Antrenörlüğü" org="Türkiye Kürek Federasyonu" meta="Aktif" /><Credential name="2. Kademe Fitness Antrenörlüğü" org="GSGM" meta="Aktif" /></div>
          </div>
          <div className="references"><h3>Referanslar</h3><div className="refs-row"><Reference name="Erhan Ertürk" title="Türkiye Kürek Federasyonu Başkanı" email="erhan_erturk@hotmail.com" /><Reference name="Yrd. Doç. Orkun Pelvan" title="Marmara Üniversitesi" email="orkunpelvan@gmail.com" /><Reference name="Ozan Bayülken" title="Fenerbahçe Kürek Şube Sorumlusu" email="ozan.bayulken@fenerbahce.org" /></div></div>
        </section>

        <section id="achievements" className="achievements-section">
          <div className="section-tag">Başarılar</div><div className="section-title">Ulusal & Uluslararası Kürek Kariyeri</div>
          <div className="ach-grid">{achievements.map(([year, rank, title, category]) => <article className="ach-card" key={`${year}-${title}`}><div className="year">{year}</div><div className="rank">{rank}</div><h4>{title}</h4><div className="cat">{category}</div></article>)}</div>
          <div className="ach-summary"><div><div className="num">16</div><div className="label">Birincilik</div></div><div><div className="num">4</div><div className="label">Uluslararası Müsabaka</div></div><div><div className="num">7</div><div className="label">Yıl Aktif Sporculuk</div></div><div><div className="num">3</div><div className="label">Farklı Yaş Kategorisi</div></div></div>
        </section>

        <section id="gallery" className="gallery-section">
          <div className="section-tag">Galeri</div><div className="section-title">Antrenmandan Kareler</div>
          <div className="gallery-grid">{gallery.map((src, index) => <button type="button" className="gallery-item" key={src} onClick={() => setLightboxIndex(index)}><img src={src} alt={`Antrenman karesi ${index + 1}`} loading="lazy" /></button>)}</div>
        </section>

        <section id="contact" className="contact-section">
          <div className="section-tag">İletişim</div><div className="section-title">Birlikte Çalışalım</div>
          <div className="contact-cards"><Contact href="https://maps.google.com/?q=Fenerbahçe+Dereağzı+Tesisleri" icon="📍" label="Adres">Fenerbahçe Dereağzı Tesisleri<br />Zühtüpaşa Mah., Kadıköy / İstanbul</Contact><Contact href="mailto:cemavat@gmail.com" icon="📧" label="E-Posta">cemavat@gmail.com</Contact><Contact href="tel:+905512327468" icon="📞" label="Telefon">+90 551 232 74 68</Contact></div>
        </section>
      </main>

      {lightboxIndex !== null && <div className="lightbox active" role="dialog" aria-modal="true" aria-label="Galeri görüntüsü" onMouseDown={event => { if (event.target === event.currentTarget) setLightboxIndex(null); }}><button className="lightbox-close" onClick={() => setLightboxIndex(null)} aria-label="Kapat">✕</button><button className="lightbox-prev" onClick={() => setLightboxIndex((lightboxIndex - 1 + gallery.length) % gallery.length)} aria-label="Önceki">‹</button><img src={gallery[lightboxIndex]} alt={`Antrenman karesi ${lightboxIndex + 1}`} /><button className="lightbox-next" onClick={() => setLightboxIndex((lightboxIndex + 1) % gallery.length)} aria-label="Sonraki">›</button><div className="lightbox-counter">{lightboxIndex + 1} / {gallery.length}</div></div>}

      <WhatsAppButton />

      <footer><div>© 2026 Cem Avat — Medical Fitness & Performance</div><div><a href="/admin">Yönetim</a><a href="#top">Yukarı ↑</a></div></footer>
    </div>
  );
}

function Credential({ name, org, meta }: { name: string; org: string; meta: string }) { return <div className="cred-item"><div><div className="name">{name}</div><div className="org">{org}</div></div><div className="meta">{meta}</div></div>; }
function Reference({ name, title, email }: { name: string; title: string; email: string }) { return <div className="ref-card"><div className="ref-name">{name}</div><div className="ref-title">{title}</div><div className="ref-email">{email}</div></div>; }
function Contact({ href, icon, label, children }: { href: string; icon: string; label: string; children: React.ReactNode }) { return <a className="contact-card" href={href} target={href.startsWith('http') ? '_blank' : undefined} rel={href.startsWith('http') ? 'noopener' : undefined}><div className="icon">{icon}</div><div className="label">{label}</div><div className="value">{children}</div></a>; }

/** Numara ve hazır metin tek yerde; e-postalardaki bağlantıyla aynı hat. */
const WHATSAPP_NUMBER = '905512327468';
const WHATSAPP_MESSAGE = 'Merhaba, antrenman hakkında bilgi almak istiyorum.';

/**
 * Görüşmeler WhatsApp üzerinden yürüdüğü için düğme sayfanın her yerinde
 * erişilebilir kalır. Lightbox açıkken gizlenir: modal görüntünün üstünde
 * duran yüzer bir düğme hem kapatma tuşunu gölgeler hem odak sırasını bozar.
 */
function WhatsAppButton() {
  const href = `https://wa.me/${WHATSAPP_NUMBER}?text=${encodeURIComponent(WHATSAPP_MESSAGE)}`;
  return (
    <a
      className="whatsapp-fab"
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      aria-label="WhatsApp ile yazın"
    >
      <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <path d="M17.47 14.38c-.3-.15-1.76-.87-2.03-.97-.27-.1-.47-.15-.67.15-.2.3-.77.96-.94 1.16-.17.2-.35.22-.65.07-.3-.15-1.26-.46-2.4-1.48-.89-.79-1.49-1.77-1.66-2.07-.17-.3-.02-.46.13-.61.14-.13.3-.35.45-.52.15-.17.2-.3.3-.5.1-.2.05-.37-.02-.52-.08-.15-.67-1.61-.92-2.21-.24-.58-.49-.5-.67-.51h-.57c-.2 0-.52.07-.8.37-.27.3-1.04 1.02-1.04 2.48s1.07 2.88 1.22 3.08c.15.2 2.1 3.2 5.08 4.49.71.3 1.26.49 1.69.63.71.22 1.36.19 1.87.12.57-.09 1.76-.72 2.01-1.41.25-.7.25-1.29.17-1.42-.07-.13-.27-.2-.57-.35z" />
        <path d="M12.04 2C6.58 2 2.13 6.45 2.13 11.91c0 1.75.46 3.46 1.32 4.96L2 22l5.25-1.38a9.87 9.87 0 0 0 4.79 1.22h.01c5.46 0 9.91-4.45 9.91-9.91C21.96 6.45 17.5 2 12.04 2zm0 18.15h-.01a8.23 8.23 0 0 1-4.19-1.15l-.3-.18-3.12.82.83-3.04-.2-.31a8.2 8.2 0 0 1-1.26-4.38c0-4.54 3.7-8.23 8.25-8.23 2.2 0 4.27.86 5.83 2.42a8.19 8.19 0 0 1 2.41 5.82c0 4.54-3.7 8.23-8.24 8.23z" />
      </svg>
      <span>WhatsApp</span>
    </a>
  );
}
