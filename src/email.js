import nodemailer from 'nodemailer';

export function createEmailService(config, logger = console) {
  const enabled = Boolean(config.smtp.host && config.smtp.user && config.smtp.pass);
  const transport = enabled ? nodemailer.createTransport({
    host: config.smtp.host,
    port: config.smtp.port,
    secure: config.smtp.secure,
    auth: { user: config.smtp.user, pass: config.smtp.pass },
    disableFileAccess: true,
    disableUrlAccess: true
  }) : null;

  async function send({ to, subject, html }) {
    if (!enabled) {
      logger.info(`[email disabled] ${subject} -> ${maskEmail(to)}`);
      return { skipped: true };
    }
    return transport.sendMail({ from: config.smtp.from, to, subject, html });
  }

  return {
    enabled,
    async requestReceived(appointment) {
      const when = formatDateTime(appointment.start_at);
      await Promise.allSettled([
        send({
          to: appointment.email,
          subject: 'Randevu talebiniz alındı',
          html: `<p>Merhaba ${escapeHtml(appointment.name)},</p><p><strong>${escapeHtml(appointment.service_name)}</strong> için <strong>${when}</strong> tarihli telefon ön görüşmesi talebiniz alındı.</p><p>Seçtiğiniz saat 24 saat boyunca sizin için tutulacak. Talebiniz sonuçlandığında e-posta ile bilgilendirileceksiniz.</p><p>Talep numarası: ${escapeHtml(appointment.id)}</p>`
        }),
        send({
          to: config.adminEmail,
          subject: `Yeni randevu talebi — ${appointment.service_name}`,
          html: `<p><strong>${escapeHtml(appointment.name)}</strong>, ${when} için talep oluşturdu.</p><p>Telefon: ${escapeHtml(appointment.phone)}<br>E-posta: ${escapeHtml(appointment.email)}<br>Not: ${escapeHtml(appointment.note || '—')}</p><p><a href="${config.appOrigin}/admin">Yönetim panelini açın</a></p>`
        })
      ]);
    },
    async appointmentApproved(appointment) {
      return send({
        to: appointment.email,
        subject: 'Telefon ön görüşmeniz onaylandı',
        html: `<p>Merhaba ${escapeHtml(appointment.name)},</p><p><strong>${escapeHtml(appointment.service_name)}</strong> için telefon ön görüşmeniz <strong>${formatDateTime(appointment.start_at)}</strong> tarihine onaylandı.</p><p>Cem Avat randevu saatinde kayıt sırasında verdiğiniz telefon numarasından sizi arayacaktır.</p><p>İptal veya değişiklik için en geç 12 saat öncesinde <a href="tel:+905512327468">telefon</a> ya da <a href="https://wa.me/905512327468">WhatsApp</a> üzerinden iletişime geçin.</p>`
      });
    },
    async appointmentRejected(appointment) {
      const note = appointment.admin_note ? `<p>Not: ${escapeHtml(appointment.admin_note)}</p>` : '';
      return send({
        to: appointment.email,
        subject: 'Randevu talebiniz sonuçlandı',
        html: `<p>Merhaba ${escapeHtml(appointment.name)},</p><p><strong>${escapeHtml(appointment.service_name)}</strong> için oluşturduğunuz randevu talebi uygunluk nedeniyle onaylanamadı.</p>${note}<p>Web sitesinden farklı bir saat için yeniden talep oluşturabilirsiniz.</p>`
      });
    },
    async appointmentCancelled(appointment) {
      return send({
        to: appointment.email,
        subject: 'Randevunuz iptal edildi',
        html: `<p>Merhaba ${escapeHtml(appointment.name)},</p><p><strong>${formatDateTime(appointment.start_at)}</strong> tarihli telefon ön görüşmeniz iptal edildi.</p><p>${escapeHtml(appointment.admin_note || '')}</p><p>Yeni bir saat için web sitesinden tekrar talep oluşturabilirsiniz.</p>`
      });
    },
    async appointmentExpired(appointment) {
      return send({
        to: appointment.email,
        subject: 'Randevu talebinizin süresi doldu',
        html: `<p>Merhaba ${escapeHtml(appointment.name)},</p><p><strong>${escapeHtml(appointment.service_name)}</strong> için oluşturduğunuz talep 24 saat içinde sonuçlandırılamadığı için süresi doldu ve seçilen saat yeniden müsait hale geldi.</p><p>Web sitesinden yeni bir talep oluşturabilirsiniz.</p>`
      });
    }
  };
}

function formatDateTime(timestamp) {
  return new Intl.DateTimeFormat('tr-TR', {
    timeZone: 'Europe/Istanbul', dateStyle: 'long', timeStyle: 'short'
  }).format(new Date(timestamp));
}

function escapeHtml(value) {
  return String(value || '').replace(/[&<>'"]/g, char => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', "'": '&#39;', '"': '&quot;'
  })[char]);
}

function maskEmail(email) {
  const [name, domain] = String(email).split('@');
  return domain ? `${name.slice(0, 2)}***@${domain}` : '***';
}
