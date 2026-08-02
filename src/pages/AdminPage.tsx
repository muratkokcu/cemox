import { useCallback, useEffect, useMemo, useState } from 'react';
import { CalendarClock, Check, Clock3, LoaderCircle, LogOut, RefreshCw, ShieldCheck, X } from 'lucide-react';
import { MonthCalendar } from '../components/MonthCalendar';
import { TimeFormatToggle } from '../components/TimeFormatToggle';
import { api, ApiError } from '../web/api';
import { addMonths, formatDateTime, formatSelectedDate, formatTime, localDateKey, monthKey, monthRange, timeOptions, toTimestamp } from '../web/date';
import type { AdminSlot, Appointment, AppointmentStatus, AvailabilityBlock, Service } from '../web/types';

type DashboardData = { slots: AdminSlot[]; appointments: Appointment[]; blocks: AvailabilityBlock[] };
type SlotState = 'open' | 'closed' | 'pending' | 'approved' | 'blocked' | 'past';

const statusLabels: Record<AppointmentStatus, string> = {
  PENDING: 'Onay bekliyor', APPROVED: 'Onaylandı', REJECTED: 'Reddedildi', EXPIRED: 'Süresi doldu', CANCELLED: 'İptal', CONFLICT: 'Çakışma'
};
const filters: Array<AppointmentStatus | ''> = ['', 'PENDING', 'APPROVED', 'CONFLICT', 'REJECTED', 'EXPIRED', 'CANCELLED'];

export function AdminPage() {
  const [session, setSession] = useState<'checking' | 'guest' | 'ready'>('checking');
  const [csrf, setCsrf] = useState('');
  const [sessionMessage, setSessionMessage] = useState('');

  useEffect(() => {
    api<{ csrfToken: string }>('/api/admin/session')
      .then(result => { setCsrf(result.csrfToken); setSession('ready'); })
      .catch(() => setSession('guest'));
  }, []);

  if (session === 'checking') return <div className="admin-loading"><LoaderCircle className="spin" /> Oturum kontrol ediliyor…</div>;
  if (session === 'guest') return <AdminLogin message={sessionMessage} onLogin={token => { setCsrf(token); setSession('ready'); setSessionMessage(''); }} />;

  return <AdminDashboard csrf={csrf} onExpired={() => { setCsrf(''); setSessionMessage('Oturumunuz sona erdi. Lütfen yeniden giriş yapın.'); setSession('guest'); }} />;
}

function AdminLogin({ message, onLogin }: { message: string; onLogin: (csrf: string) => void }) {
  const [password, setPassword] = useState('');
  const [error, setError] = useState(message);
  const [busy, setBusy] = useState(false);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true); setError('');
    try {
      const result = await api<{ csrfToken: string }>('/api/admin/login', { method: 'POST', body: JSON.stringify({ password }) });
      setPassword('');
      onLogin(result.csrfToken);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Giriş yapılamadı.');
    } finally { setBusy(false); }
  }

  return (
    <main className="admin-login">
      <form onSubmit={submit}>
        <a className="brand" href="/">CEM<span>.</span>AVAT</a>
        <span className="login-icon"><ShieldCheck /></span>
        <h1>Randevu yönetimi</h1>
        <p>Branş müsaitliğini ve randevu onaylarını tek takvimden yönetin.</p>
        {error && <div className="inline-error">{error}</div>}
        <label>Yönetici şifresi<input autoFocus required type="password" autoComplete="current-password" value={password} onChange={event => setPassword(event.target.value)} /></label>
        <button className="submit-button" disabled={busy}>{busy ? 'Giriş yapılıyor…' : 'Giriş yap'}</button>
      </form>
    </main>
  );
}

function AdminDashboard({ csrf, onExpired }: { csrf: string; onExpired: () => void }) {
  const [services, setServices] = useState<Service[]>([]);
  const [serviceId, setServiceId] = useState('');
  const [month, setMonth] = useState(monthKey(localDateKey()));
  const [selectedDate, setSelectedDate] = useState(localDateKey());
  const [data, setData] = useState<DashboardData>({ slots: [], appointments: [], blocks: [] });
  const [loading, setLoading] = useState(true);
  const [busySlots, setBusySlots] = useState<Set<number>>(new Set());
  const [use24Hour, setUse24Hour] = useState(true);
  const [filter, setFilter] = useState<AppointmentStatus | ''>('');
  const [error, setError] = useState('');
  const [toast, setToast] = useState('');

  const guard = useCallback((err: unknown) => {
    if (err instanceof ApiError && err.status === 401) { onExpired(); return true; }
    setError(err instanceof Error ? err.message : 'İşlem tamamlanamadı.');
    return false;
  }, [onExpired]);

  useEffect(() => {
    api<{ services: Service[] }>('/api/services')
      .then(result => { setServices(result.services); setServiceId(current => current || result.services[0]?.id || ''); })
      .catch(guard);
  }, [guard]);

  const loadData = useCallback(async () => {
    if (!serviceId) return;
    setLoading(true); setError('');
    const range = monthRange(`${month}-01`);
    try {
      const [slotResult, appointmentResult, blockResult] = await Promise.all([
        api<{ slots: AdminSlot[] }>(`/api/admin/availability-slots?service=${encodeURIComponent(serviceId)}&from=${encodeURIComponent(new Date(range.from).toISOString())}&to=${encodeURIComponent(new Date(range.to).toISOString())}`),
        api<{ appointments: Appointment[] }>('/api/admin/appointments'),
        api<{ blocks: AvailabilityBlock[] }>('/api/admin/blocks')
      ]);
      setData({ slots: slotResult.slots, appointments: appointmentResult.appointments, blocks: blockResult.blocks });
    } catch (err) { guard(err); }
    finally { setLoading(false); }
  }, [guard, month, serviceId]);

  useEffect(() => { loadData(); }, [loadData]);

  const openSet = useMemo(() => new Set(data.slots.map(slot => slot.start_at)), [data.slots]);
  const dateTones = useMemo(() => {
    const tones = new Map<string, 'open' | 'pending' | 'approved' | 'mixed'>();
    for (const slot of data.slots) tones.set(localDateKey(slot.start_at), 'open');
    for (const appointment of data.appointments.filter(item => item.service_id === serviceId && ['PENDING', 'APPROVED'].includes(item.status))) {
      const date = localDateKey(appointment.start_at);
      const next = appointment.status === 'PENDING' ? 'pending' : 'approved';
      const current = tones.get(date);
      tones.set(date, current && current !== next ? 'mixed' : next);
    }
    return tones;
  }, [data, serviceId]);

  const selectedService = services.find(service => service.id === serviceId);
  const listedAppointments = data.appointments.filter(item => !filter || item.status === filter);

  function stateFor(start: number): { state: SlotState; appointment?: Appointment; block?: AvailabilityBlock } {
    const appointment = data.appointments.find(item => item.service_id === serviceId && item.start_at === start && ['PENDING', 'APPROVED'].includes(item.status));
    if (appointment) return { state: appointment.status === 'APPROVED' ? 'approved' : 'pending', appointment };
    const end = start + 20 * 60_000;
    const block = data.blocks.find(item => item.start_at < end + 10 * 60_000 && item.end_at > start);
    if (block) return { state: 'blocked', block };
    if (start < Date.now()) return { state: 'past' };
    return { state: openSet.has(start) ? 'open' : 'closed' };
  }

  async function toggleSlot(start: number, open: boolean) {
    setBusySlots(current => new Set(current).add(start));
    setError('');
    try {
      await api('/api/admin/availability-slots', {
        method: 'PUT', body: JSON.stringify({ serviceId, start: new Date(start).toISOString(), open })
      }, csrf);
      setToast(open ? 'Saat randevuya açıldı.' : 'Saat kapatıldı.');
      window.setTimeout(() => setToast(''), 2400);
      await loadData();
    } catch (err) { guard(err); }
    finally { setBusySlots(current => { const next = new Set(current); next.delete(start); return next; }); }
  }

  async function decide(id: string, action: 'approve' | 'reject' | 'cancel') {
    let adminNote = '';
    if (action === 'approve') {
      if (!window.confirm('Bu randevuyu onaylamak istiyor musunuz?')) return;
    } else {
      const note = window.prompt(action === 'reject' ? 'Ret nedenini yazabilirsiniz:' : 'İptal açıklamasını yazabilirsiniz:', '');
      if (note === null) return;
      adminNote = note;
    }
    try {
      await api(`/api/admin/appointments/${encodeURIComponent(id)}`, {
        method: 'PATCH', body: JSON.stringify({ action, adminNote })
      }, csrf);
      setToast('Randevu güncellendi.');
      window.setTimeout(() => setToast(''), 2400);
      await loadData();
    } catch (err) { guard(err); }
  }

  async function logout() {
    try { await api('/api/admin/logout', { method: 'POST' }, csrf); } catch { /* session is cleared locally either way */ }
    onExpired();
  }

  return (
    <div className="admin-page">
      <header className="admin-header">
        <a className="brand" href="/">CEM<span>.</span>AVAT <small>Yönetim</small></a>
        <div><button type="button" onClick={loadData}><RefreshCw size={16} /> Yenile</button><button type="button" onClick={logout}><LogOut size={16} /> Çıkış</button></div>
      </header>
      <main className="admin-content">
        <div className="admin-title"><div><span className="eyebrow">Müsaitlik planı</span><h1>Takvim</h1><p>Branşı, günü ve saati seçerek randevuya açın veya kapatın.</p></div><div className="legend"><span className="open">Açık</span><span className="pending">Bekliyor</span><span className="approved">Onaylı</span></div></div>
        {error && <div className="page-error">{error}<button onClick={() => setError('')}><X size={16} /></button></div>}

        <div className="admin-scheduler">
          <aside className="admin-service-panel">
            <span className="eyebrow">Branş grubu</span>
            <h2>{selectedService?.name || 'Branş seçin'}</h2>
            <p>Açtığınız saatler yalnızca seçili branşın randevu ekranında görünür.</p>
            <div className="service-options">
              {services.map(service => <button type="button" className={service.id === serviceId ? 'active' : ''} key={service.id} onClick={() => setServiceId(service.id)}><span>{service.name}</span>{service.id === serviceId && <Check size={16} />}</button>)}
            </div>
          </aside>
          <MonthCalendar
            month={month}
            selectedDate={selectedDate}
            dateTones={dateTones}
            onSelectDate={setSelectedDate}
            onMonthChange={direction => {
              const next = monthKey(addMonths(`${month}-01`, direction));
              setMonth(next); setSelectedDate(`${next}-01`);
            }}
          />
          <section className="admin-times-panel">
            <header className="times-heading"><div><h2>{formatSelectedDate(selectedDate)}</h2><span>{selectedService?.name}</span></div><TimeFormatToggle use24Hour={use24Hour} onChange={setUse24Hour} /></header>
            <div className="admin-time-list">
              {loading ? <div className="panel-state"><LoaderCircle className="spin" /> Takvim yükleniyor…</div> : timeOptions().map(time => {
                const start = toTimestamp(selectedDate, time);
                const info = stateFor(start);
                const busy = busySlots.has(start);
                const locked = ['pending', 'approved', 'blocked', 'past'].includes(info.state);
                const label = { open: 'Açık', closed: 'Kapalı', pending: 'Onay bekliyor', approved: 'Onaylandı', blocked: 'Genel kapalı', past: 'Geçmiş' }[info.state];
                return (
                  <button type="button" className={`admin-time-row ${info.state}`} key={time} disabled={locked || busy} onClick={() => toggleSlot(start, info.state !== 'open')} title={info.appointment ? `${info.appointment.name} · ${info.appointment.phone}` : info.block?.reason}>
                    <strong>{formatTime(start, use24Hour)}</strong>
                    <span>{info.appointment?.name || label}</span>
                    {busy ? <LoaderCircle className="spin" size={18} /> : <i aria-label={label}><b /></i>}
                  </button>
                );
              })}
            </div>
          </section>
        </div>

        <section className="appointment-section">
          <header><div><span className="eyebrow">Randevular</span><h2>Talep ve onaylar</h2></div><div className="filter-tabs">{filters.map(value => <button type="button" key={value || 'all'} className={filter === value ? 'active' : ''} onClick={() => setFilter(value)}>{value ? statusLabels[value] : 'Tümü'}</button>)}</div></header>
          <div className="appointment-list">
            {!listedAppointments.length && <div className="panel-state">Bu filtrede randevu bulunmuyor.</div>}
            {listedAppointments.map(item => <AppointmentCard key={item.id} appointment={item} onDecide={decide} />)}
          </div>
        </section>
      </main>
      {toast && <div className="toast"><Check size={17} /> {toast}</div>}
    </div>
  );
}

function AppointmentCard({ appointment, onDecide }: { appointment: Appointment; onDecide: (id: string, action: 'approve' | 'reject' | 'cancel') => void }) {
  const pending = ['PENDING', 'CONFLICT'].includes(appointment.status);
  return (
    <article className="appointment-card">
      <div className="appointment-date"><CalendarClock /><span>{formatDateTime(appointment.start_at)}</span></div>
      <div className="appointment-person"><h3>{appointment.name}</h3><p>{appointment.service_name}</p><p><a href={`tel:${appointment.phone}`}>{appointment.phone}</a> · <a href={`mailto:${appointment.email}`}>{appointment.email}</a></p>{appointment.note && <blockquote>{appointment.note}</blockquote>}</div>
      <span className={`status-badge ${appointment.status.toLowerCase()}`}>{statusLabels[appointment.status]}</span>
      <div className="appointment-actions">
        {pending && <><button className="approve" onClick={() => onDecide(appointment.id, 'approve')}>Onayla</button><button onClick={() => onDecide(appointment.id, 'reject')}>Reddet</button></>}
        {appointment.status === 'APPROVED' && <button onClick={() => onDecide(appointment.id, 'cancel')}>İptal et</button>}
      </div>
    </article>
  );
}
