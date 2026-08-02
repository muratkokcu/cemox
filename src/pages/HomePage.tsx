import { ArrowDown, ArrowUpRight, Dumbbell, HeartPulse, Menu, MoveRight, X } from 'lucide-react';
import { useState } from 'react';
import { BookingScheduler } from '../components/BookingScheduler';

export function HomePage() {
  const [menuOpen, setMenuOpen] = useState(false);
  return (
    <div className="site-page">
      <header className="site-header">
        <a className="brand" href="#top" aria-label="Cem Avat ana sayfa">CEM<span>.</span>AVAT</a>
        <nav className={menuOpen ? 'open' : ''}>
          <a href="#yaklasim" onClick={() => setMenuOpen(false)}>Yaklaşım</a>
          <a href="#hizmetler" onClick={() => setMenuOpen(false)}>Hizmetler</a>
          <a href="#randevu" onClick={() => setMenuOpen(false)}>Randevu</a>
          <a href="tel:+905512327468">İletişim</a>
        </nav>
        <a className="header-cta" href="#randevu">Saat seç <MoveRight size={16} /></a>
        <button className="menu-button" aria-label="Menüyü aç/kapat" onClick={() => setMenuOpen(value => !value)}>
          {menuOpen ? <X /> : <Menu />}
        </button>
      </header>

      <main id="top">
        <section className="hero-section">
          <div className="hero-copy">
            <span className="eyebrow">Bilim temelli · kişiye özel</span>
            <h1>Daha iyi hareket et.<br /><em>Daha güçlü yaşa.</em></h1>
            <p>Hareket kapasitenizi, performansınızı ve yaşam kalitenizi birlikte geliştiren sürdürülebilir bir antrenman yaklaşımı.</p>
            <a href="#randevu">Takvimden saat seç <ArrowDown size={18} /></a>
          </div>
          <div className="hero-image"><img src="/assets/cem-hero.webp" alt="Cem Avat antrenman sırasında" /></div>
        </section>

        <section className="booking-section">
          <div className="section-intro">
            <div><span className="eyebrow">Online randevu</span><h2>Size uygun zamanı<br />doğrudan seçin.</h2></div>
            <p>Yalnızca antrenörün branşa özel açtığı saatler görünür. Seçiminiz çakışmalara karşı tutulur ve onaya gönderilir.</p>
          </div>
          <BookingScheduler />
        </section>

        <section className="approach-section" id="yaklasim">
          <div className="approach-lead"><span className="eyebrow">Yaklaşım</span><h2>Hazır program değil,<br />size ait bir sistem.</h2></div>
          <div className="approach-grid">
            <article><span>01</span><HeartPulse /><h3>Değerlendir</h3><p>Hareket kapasitesi, ihtiyaçlar ve hedefler bütüncül olarak ele alınır.</p></article>
            <article><span>02</span><Dumbbell /><h3>Planla</h3><p>Günlük yaşamınıza ve gelişim hızınıza uyum sağlayan bir yol haritası kurulur.</p></article>
            <article><span>03</span><ArrowUpRight /><h3>Geliştir</h3><p>Süreç ölçülür, gerektiğinde güncellenir ve sürdürülebilir hale getirilir.</p></article>
          </div>
        </section>

        <section className="services-band" id="hizmetler">
          <span className="eyebrow">Çalışma alanları</span>
          <div className="service-lines">
            {['Medical Fitness', 'Kişisel Antrenman', 'Fonksiyonel Antrenman', 'Performans Geliştirme', 'Kürek Antrenörlüğü', 'Korektif Egzersiz'].map((item, index) => (
              <a href="#randevu" key={item}><span>{String(index + 1).padStart(2, '0')}</span><strong>{item}</strong><ArrowUpRight /></a>
            ))}
          </div>
        </section>
      </main>

      <footer><a className="brand" href="#top">CEM<span>.</span>AVAT</a><p>Hareket, performans ve sürdürülebilir gelişim.</p><a href="/admin">Yönetim</a></footer>
    </div>
  );
}
