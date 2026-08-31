import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Ban, CalendarClock, CalendarOff, Check, ChevronDown, CircleCheck, CircleX, LoaderCircle, LogOut, Plus, RefreshCw, ShieldCheck, Trash2, TriangleAlert, X } from 'lucide-react';
import { MonthCalendar } from '../components/MonthCalendar';
import { TimeFormatToggle } from '../components/TimeFormatToggle';
import { api, ApiError } from '../web/api';
import { addDays, addMonths, DAY_MS, formatBlockRange, formatDateTime, formatMonth, formatSelectedDate, formatTime, localDateKey, monthKey, monthRange, timeOptions, toTimestamp } from '../web/date';
import type { AdminSlot, Appointment, AppointmentStatus, AvailabilityBlock, Service } from '../web/types';

/** Takvim görünümünün verisi: gezilen ay ve seçili branşla sınırlı, sayfalanmamış. */
type DashboardData = { slots: AdminSlot[]; monthAppointments: Appointment[]; blocks: AvailabilityBlock[] };
/** Randevu listesi: sunucu tarafında filtrelenir ve sayfalanır. */
type AppointmentList = { items: Appointment[]; total: number };

const PAGE_SIZE = 25;
/** Takvim bir ayı kapsar; sunucu üst sınırı 500. */
const CALENDAR_LIMIT = 500;
type SlotState = 'open' | 'closed' | 'pending' | 'approved' | 'blocked' | 'past';
type DecisionAction = 'approve' | 'reject' | 'cancel';
type BlockDraft = {
  startDate: string; startTime: string;
  endDate: string; endTime: string;
  allDay: boolean; reason: string;
};

const ACTIVE_STATUSES: AppointmentStatus[] = ['PENDING', 'APPROVED'];

/**
 * Taslaktan gerçek zaman aralığını üretir. Kapalı zamanın bitişi dışlayıcı olduğu için
 * "tüm gün" modunda seçilen son gün de kapansın diye bitiş ertesi güne taşınır.
 */
function draftRange(draft: BlockDraft): { startAt: number; endAt: number } {
  if (draft.allDay) {
    return {
      startAt: toTimestamp(draft.startDate, '00:00'),
      endAt: toTimestamp(addDays(draft.endDate, 1), '00:00')
    };
  }
  return {
    startAt: toTimestamp(draft.startDate, draft.startTime),
    endAt: toTimestamp(draft.endDate, draft.endTime)
  };
}

/** Sunucudaki doğrulamanın aynısı; kullanıcı hatayı istek göndermeden görür. */
function validateDraft(draft: BlockDraft): string {
  const { startAt, endAt } = draftRange(draft);
  if (!Number.isFinite(startAt) || !Number.isFinite(endAt)) return 'Geçerli bir tarih ve saat seçin.';
  if (endAt <= startAt) return 'Bitiş zamanı başlangıçtan sonra olmalıdır.';
  if (endAt - startAt > 31 * DAY_MS) return 'Tek bir kapalı zaman 31 günden uzun olamaz.';
  return '';
}

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
  const [data, setData] = useState<DashboardData>({ slots: [], monthAppointments: [], blocks: [] });
  const [loading, setLoading] = useState(true);
  const [list, setList] = useState<AppointmentList>({ items: [], total: 0 });
  const [listLoading, setListLoading] = useState(true);
  const [listMoreBusy, setListMoreBusy] = useState(false);
  const [busySlots, setBusySlots] = useState<Set<number>>(new Set());
  const [use24Hour, setUse24Hour] = useState(true);
  const [filter, setFilter] = useState<AppointmentStatus | ''>('');
  const [error, setError] = useState('');
  const [toast, setToast] = useState('');
  const [decision, setDecision] = useState<{ appointment: Appointment; action: DecisionAction } | null>(null);
  const [decisionBusy, setDecisionBusy] = useState(false);
  const [decisionError, setDecisionError] = useState('');
  const [blockDraft, setBlockDraft] = useState<BlockDraft | null>(null);
  const [removingBlock, setRemovingBlock] = useState<AvailabilityBlock | null>(null);
  const [blockBusy, setBlockBusy] = useState(false);
  const [blockError, setBlockError] = useState('');

  const notify = useCallback((message: string) => {
    setToast(message);
    window.setTimeout(() => setToast(''), 2400);
  }, []);

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
      const from = encodeURIComponent(new Date(range.from).toISOString());
      const to = encodeURIComponent(new Date(range.to).toISOString());
      const service = encodeURIComponent(serviceId);
      const [slotResult, appointmentResult, blockResult] = await Promise.all([
        api<{ slots: AdminSlot[] }>(`/api/admin/availability-slots?service=${service}&from=${from}&to=${to}`),
        // Takvim, sayfalanmış listeden bağımsız olarak ayın tamamını çeker.
        // Aksi halde kayıt sayısı sayfa sınırını aşınca dolu bir saat boş görünürdü.
        api<{ appointments: Appointment[] }>(`/api/admin/appointments?service=${service}&from=${from}&to=${to}&limit=${CALENDAR_LIMIT}`),
        // Kapalı zamanlar da gezilen ayla aynı aralıktan gelir; aksi halde
        // takvim başka bir aya gittiğinde saatler yanlışlıkla açık görünür.
        api<{ blocks: AvailabilityBlock[] }>(`/api/admin/blocks?from=${from}&to=${to}`)
      ]);
      setData({ slots: slotResult.slots, monthAppointments: appointmentResult.appointments, blocks: blockResult.blocks });
    } catch (err) { guard(err); }
    finally { setLoading(false); }
  }, [guard, month, serviceId]);

  useEffect(() => { loadData(); }, [loadData]);

  /** Listeyi baştan yükler. `count` mevcut derinliği korumak için kullanılır. */
  const loadAppointments = useCallback(async (count = PAGE_SIZE) => {
    setListLoading(true);
    try {
      const query = new URLSearchParams({ limit: String(Math.min(count, CALENDAR_LIMIT)), offset: '0' });
      if (filter) query.set('status', filter);
      const result = await api<{ appointments: Appointment[]; total: number }>(`/api/admin/appointments?${query}`);
      setList({ items: result.appointments, total: result.total });
    } catch (err) { guard(err); }
    finally { setListLoading(false); }
  }, [filter, guard]);

  // Filtre değiştiğinde liste ilk sayfadan yeniden yüklenir.
  useEffect(() => { loadAppointments(); }, [loadAppointments]);

  async function loadMoreAppointments() {
    setListMoreBusy(true);
    try {
      const query = new URLSearchParams({ limit: String(PAGE_SIZE), offset: String(list.items.length) });
      if (filter) query.set('status', filter);
      const result = await api<{ appointments: Appointment[]; total: number }>(`/api/admin/appointments?${query}`);
      setList(current => ({ items: [...current.items, ...result.appointments], total: result.total }));
    } catch (err) { guard(err); }
    finally { setListMoreBusy(false); }
  }

  /** Karar sonrası hem takvimi hem listeyi tazeler; liste derinliği korunur. */
  const refreshAll = useCallback(async () => {
    await Promise.all([loadData(), loadAppointments(Math.max(PAGE_SIZE, list.items.length))]);
  }, [loadData, loadAppointments, list.items.length]);

  const openSet = useMemo(() => new Set(data.slots.map(slot => slot.start_at)), [data.slots]);
  const dateTones = useMemo(() => {
    const tones = new Map<string, 'open' | 'pending' | 'approved' | 'mixed'>();
    for (const slot of data.slots) tones.set(localDateKey(slot.start_at), 'open');
    for (const appointment of data.monthAppointments.filter(item => ACTIVE_STATUSES.includes(item.status))) {
      const date = localDateKey(appointment.start_at);
      const next = appointment.status === 'PENDING' ? 'pending' : 'approved';
      const current = tones.get(date);
      tones.set(date, current && current !== next ? 'mixed' : next);
    }
    return tones;
  }, [data, serviceId]);

  const selectedService = services.find(service => service.id === serviceId);

  function stateFor(start: number): { state: SlotState; appointment?: Appointment; block?: AvailabilityBlock } {
    const appointment = data.monthAppointments.find(item => item.start_at === start && ACTIVE_STATUSES.includes(item.status));
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
      notify(open ? 'Saat randevuya açıldı.' : 'Saat kapatıldı.');
      await loadData();
    } catch (err) { guard(err); }
    finally { setBusySlots(current => { const next = new Set(current); next.delete(start); return next; }); }
  }

  function openDecision(appointment: Appointment, action: DecisionAction) {
    setDecisionError('');
    setDecision({ appointment, action });
  }

  async function decide(adminNote: string) {
    if (!decision) return;
    setDecisionBusy(true);
    setDecisionError('');
    try {
      await api(`/api/admin/appointments/${encodeURIComponent(decision.appointment.id)}`, {
        method: 'PATCH', body: JSON.stringify({ action: decision.action, adminNote })
      }, csrf);
      const message = decision.action === 'approve' ? 'Randevu onaylandı.' : decision.action === 'reject' ? 'Randevu talebi reddedildi.' : 'Randevu iptal edildi.';
      setDecision(null);
      notify(message);
      await refreshAll();
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) guard(err);
      else setDecisionError(err instanceof Error ? err.message : 'İşlem tamamlanamadı.');
    } finally { setDecisionBusy(false); }
  }

  /** Modalı seçili günden yola çıkarak tüm gün kapalı olacak şekilde açar. */
  function openBlockDraft() {
    setBlockError('');
    setBlockDraft({
      startDate: selectedDate, startTime: '09:00',
      endDate: selectedDate, endTime: '18:00',
      allDay: true, reason: ''
    });
  }

  async function createBlock(draft: BlockDraft) {
    const { startAt, endAt } = draftRange(draft);
    setBlockBusy(true); setBlockError('');
    try {
      await api('/api/admin/blocks', {
        method: 'POST',
        body: JSON.stringify({
          start: new Date(startAt).toISOString(),
          end: new Date(endAt).toISOString(),
          reason: draft.reason.trim()
        })
      }, csrf);
      setBlockDraft(null);
      notify('Kapalı zaman eklendi.');
      // Blok başka bir aya düştüyse takvimi oraya taşı, aksi halde kayıt görünmez.
      const targetMonth = monthKey(localDateKey(startAt));
      if (targetMonth !== month) { setMonth(targetMonth); setSelectedDate(localDateKey(startAt)); }
      else await loadData();
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) guard(err);
      else setBlockError(err instanceof Error ? err.message : 'Kapalı zaman eklenemedi.');
    } finally { setBlockBusy(false); }
  }

  async function removeBlock(block: AvailabilityBlock) {
    setBlockBusy(true); setBlockError('');
    try {
      await api(`/api/admin/blocks/${encodeURIComponent(block.id)}`, { method: 'DELETE' }, csrf);
      setRemovingBlock(null);
      notify('Kapalı zaman kaldırıldı.');
      await loadData();
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) guard(err);
      else setBlockError(err instanceof Error ? err.message : 'Kapalı zaman kaldırılamadı.');
    } finally { setBlockBusy(false); }
  }

  async function logout() {
    try { await api('/api/admin/logout', { method: 'POST' }, csrf); } catch { /* session is cleared locally either way */ }
    onExpired();
  }

  return (
    <div className="admin-page">
      <header className="admin-header">
        <a className="brand" href="/">CEM<span>.</span>AVAT <small>Yönetim</small></a>
        <div><button type="button" onClick={refreshAll}><RefreshCw size={16} /> Yenile</button><button type="button" onClick={logout}><LogOut size={16} /> Çıkış</button></div>
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

        <BlockSection
          blocks={data.blocks}
          month={month}
          loading={loading}
          use24Hour={use24Hour}
          onAdd={openBlockDraft}
          onRemove={block => { setBlockError(''); setRemovingBlock(block); }}
        />

        <section className="appointment-section">
          <header><div><span className="eyebrow">Randevular</span><h2>Talep ve onaylar</h2></div><div className="filter-tabs">{filters.map(value => <button type="button" key={value || 'all'} className={filter === value ? 'active' : ''} onClick={() => setFilter(value)}>{value ? statusLabels[value] : 'Tümü'}</button>)}</div></header>
          <div className="appointment-list">
            {listLoading && <div className="panel-state"><LoaderCircle className="spin" /> Randevular yükleniyor…</div>}
            {!listLoading && !list.items.length && <div className="panel-state">Bu filtrede randevu bulunmuyor.</div>}
            {!listLoading && list.items.map(item => <AppointmentCard key={item.id} appointment={item} onDecide={openDecision} />)}
          </div>
          {!listLoading && list.total > 0 && (
            <footer className="appointment-footer">
              <span>{list.items.length} / {list.total} randevu gösteriliyor</span>
              {list.items.length < list.total && (
                <button type="button" disabled={listMoreBusy} onClick={loadMoreAppointments}>
                  {listMoreBusy
                    ? <><LoaderCircle className="spin" size={15} /> Yükleniyor…</>
                    : <><ChevronDown size={15} /> Daha fazla yükle</>}
                </button>
              )}
            </footer>
          )}
        </section>
      </main>
      {decision && <DecisionModal decision={decision} busy={decisionBusy} error={decisionError} onClose={() => { if (!decisionBusy) setDecision(null); }} onConfirm={decide} />}
      {blockDraft && (
        <BlockModal
          initial={blockDraft}
          busy={blockBusy}
          error={blockError}
          use24Hour={use24Hour}
          onClose={() => { if (!blockBusy) setBlockDraft(null); }}
          onConfirm={createBlock}
        />
      )}
      {removingBlock && (
        <BlockRemoveModal
          block={removingBlock}
          busy={blockBusy}
          error={blockError}
          use24Hour={use24Hour}
          onClose={() => { if (!blockBusy) setRemovingBlock(null); }}
          onConfirm={() => removeBlock(removingBlock)}
        />
      )}
      {toast && <div className="toast" role="status"><Check size={17} /> {toast}</div>}
    </div>
  );
}

function AppointmentCard({ appointment, onDecide }: { appointment: Appointment; onDecide: (appointment: Appointment, action: DecisionAction) => void }) {
  const pending = ['PENDING', 'CONFLICT'].includes(appointment.status);
  return (
    <article className="appointment-card">
      <div className="appointment-date"><CalendarClock /><span>{formatDateTime(appointment.start_at)}</span></div>
      <div className="appointment-person"><h3>{appointment.name}</h3><p>{appointment.service_name}</p><p><a href={`tel:${appointment.phone}`}>{appointment.phone}</a> · <a href={`mailto:${appointment.email}`}>{appointment.email}</a></p>{appointment.note && <blockquote>{appointment.note}</blockquote>}</div>
      <span className={`status-badge ${appointment.status.toLowerCase()}`}>{statusLabels[appointment.status]}</span>
      <div className="appointment-actions">
        {pending && <><button className="approve" onClick={() => onDecide(appointment, 'approve')}>Onayla</button><button onClick={() => onDecide(appointment, 'reject')}>Reddet</button></>}
        {appointment.status === 'APPROVED' && <button onClick={() => onDecide(appointment, 'cancel')}>İptal et</button>}
      </div>
    </article>
  );
}

/** Modal açıkken sayfa kaydırmasını kilitler, Escape ile kapanmayı bağlar ve odağı alır. */
function useModalShell(busy: boolean, onClose: () => void) {
  const dialogRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previousOverflow = document.body.style.overflow;
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === 'Escape' && !busy) onClose(); };
    document.body.style.overflow = 'hidden';
    document.addEventListener('keydown', closeOnEscape);
    dialogRef.current?.focus();
    return () => { document.body.style.overflow = previousOverflow; document.removeEventListener('keydown', closeOnEscape); };
  }, [busy, onClose]);
  return dialogRef;
}

function BlockSection({ blocks, month, loading, use24Hour, onAdd, onRemove }: {
  blocks: AvailabilityBlock[];
  month: string;
  loading: boolean;
  use24Hour: boolean;
  onAdd: () => void;
  onRemove: (block: AvailabilityBlock) => void;
}) {
  const ordered = [...blocks].sort((a, b) => a.start_at - b.start_at);
  return (
    <section className="block-section">
      <header>
        <div>
          <span className="eyebrow">Kapalı zamanlar</span>
          <h2>{formatMonth(month)}</h2>
          <p>Bu aralıklar tüm branşlara kapalıdır ve açık saatlerin önüne geçer. Tatil, izin ve seyahat için kullanın.</p>
        </div>
        <button type="button" className="block-add" onClick={onAdd}><Plus size={15} /> Kapalı zaman ekle</button>
      </header>
      <div className="block-list">
        {loading && <div className="panel-state"><LoaderCircle className="spin" /> Yükleniyor…</div>}
        {!loading && !ordered.length && <div className="panel-state">Bu ayda kapalı zaman yok.</div>}
        {!loading && ordered.map(block => (
          <article className="block-card" key={block.id}>
            <span className="block-icon"><CalendarOff /></span>
            <div>
              <strong>{formatBlockRange(block.start_at, block.end_at, use24Hour)}</strong>
              <p>{block.reason || 'Açıklama girilmedi'}</p>
            </div>
            <button type="button" onClick={() => onRemove(block)}>
              <Trash2 size={14} /> Kaldır
            </button>
          </article>
        ))}
      </div>
    </section>
  );
}

function BlockModal({ initial, busy, error, use24Hour, onClose, onConfirm }: {
  initial: BlockDraft;
  busy: boolean;
  error: string;
  use24Hour: boolean;
  onClose: () => void;
  onConfirm: (draft: BlockDraft) => void;
}) {
  const [draft, setDraft] = useState(initial);
  const [clashCount, setClashCount] = useState(0);
  const dialogRef = useModalShell(busy, onClose);
  const update = (patch: Partial<BlockDraft>) => setDraft(current => ({ ...current, ...patch }));

  const { startAt, endAt } = draftRange(draft);
  const localError = validateDraft(draft);
  const valid = !localError;

  // Kapalı zaman eklemek mevcut randevuları iptal etmez, bu yüzden çakışanları önceden gösteririz.
  // Sayım seçilen aralığın tamamı için sunucudan gelir: aralık gezilen ayın dışına
  // taşabildiği için takvim verisiyle sayılsaydı eksik çıkardı.
  const firstCheck = useRef(true);
  useEffect(() => {
    if (!valid) { setClashCount(0); return; }
    let cancelled = false;
    // İlk kontrol beklemeden yapılır: modal açılırken uyarının sonradan belirip
    // alttaki butonları aşağı itmesini önler. Sonraki düzenlemeler geciktirilir.
    const delay = firstCheck.current ? 0 : 250;
    firstCheck.current = false;
    const timer = window.setTimeout(async () => {
      const query = new URLSearchParams({
        from: new Date(startAt).toISOString(),
        to: new Date(endAt).toISOString(),
        limit: String(CALENDAR_LIMIT)
      });
      try {
        const result = await api<{ appointments: Appointment[] }>(`/api/admin/appointments?${query}`);
        if (!cancelled) setClashCount(result.appointments.filter(item => ACTIVE_STATUSES.includes(item.status)).length);
      } catch {
        // Uyarı tamamlayıcıdır; sorgulanamazsa asıl akış engellenmez.
        if (!cancelled) setClashCount(0);
      }
    }, delay);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [valid, startAt, endAt]);

  return (
    <div className="decision-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) onClose(); }}>
      <div className="decision-modal block-modal" role="dialog" aria-modal="true" aria-labelledby="block-title" tabIndex={-1} ref={dialogRef}>
        <header className="decision-header">
          <span className="decision-icon"><CalendarOff /></span>
          <div><span className="eyebrow">Müsaitlik istisnası</span><h2 id="block-title">Kapalı zaman ekle</h2></div>
          <button type="button" className="decision-close" aria-label="Pencereyi kapat" disabled={busy} onClick={onClose}><X size={18} /></button>
        </header>
        <p className="decision-description">Seçilen aralıkta hiçbir branş randevuya açık olmaz; danışanlar bu saatleri göremez.</p>

        <div className="block-fields">
          <label className="block-allday">
            <input type="checkbox" checked={draft.allDay} onChange={event => update({ allDay: event.target.checked })} />
            <span>Tüm gün</span>
          </label>
          <label>Başlangıç
            <input type="date" required value={draft.startDate} onChange={event => update({ startDate: event.target.value })} />
          </label>
          {!draft.allDay && (
            <label>Saat
              <input type="time" required step={1800} value={draft.startTime} onChange={event => update({ startTime: event.target.value })} />
            </label>
          )}
          <label>{draft.allDay ? 'Son gün' : 'Bitiş'}
            <input type="date" required value={draft.endDate} onChange={event => update({ endDate: event.target.value })} />
          </label>
          {!draft.allDay && (
            <label>Saat
              <input type="time" required step={1800} value={draft.endTime} onChange={event => update({ endTime: event.target.value })} />
            </label>
          )}
        </div>

        {valid && (
          <div className="block-preview">
            <CalendarClock />
            <span>{formatBlockRange(startAt, endAt, use24Hour)}</span>
          </div>
        )}

        <label className="decision-note">Açıklama <span>(isteğe bağlı)</span>
          <textarea rows={2} maxLength={200} placeholder="Örn. yıllık izin, seminer, seyahat…" value={draft.reason} onChange={event => update({ reason: event.target.value })} />
          <small>{draft.reason.length} / 200</small>
        </label>

        {clashCount > 0 && (
          <div className="block-warning">
            <TriangleAlert size={16} />
            <div>
              <strong>Bu aralıkta {clashCount} aktif randevu var.</strong>
              <p>Kapalı zaman eklemek onları iptal etmez; gerekirse randevular listesinden ayrıca iptal edin.</p>
            </div>
          </div>
        )}
        {(localError || error) && <div className="inline-error">{localError || error}</div>}

        <footer className="decision-actions">
          <button type="button" className="decision-secondary" disabled={busy} onClick={onClose}>Vazgeç</button>
          <button type="button" className="decision-primary" disabled={busy || !valid} onClick={() => onConfirm(draft)}>
            {busy ? <><LoaderCircle className="spin" size={16} /> Ekleniyor…</> : 'Kapalı zamanı ekle'}
          </button>
        </footer>
      </div>
    </div>
  );
}

function BlockRemoveModal({ block, busy, error, use24Hour, onClose, onConfirm }: {
  block: AvailabilityBlock;
  busy: boolean;
  error: string;
  use24Hour: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const dialogRef = useModalShell(busy, onClose);
  return (
    <div className="decision-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) onClose(); }}>
      <div className="decision-modal cancel" role="dialog" aria-modal="true" aria-labelledby="block-remove-title" tabIndex={-1} ref={dialogRef}>
        <header className="decision-header">
          <span className="decision-icon"><Trash2 /></span>
          <div><span className="eyebrow">Kapalı zaman</span><h2 id="block-remove-title">Kaydı kaldır</h2></div>
          <button type="button" className="decision-close" aria-label="Pencereyi kapat" disabled={busy} onClick={onClose}><X size={18} /></button>
        </header>
        <p className="decision-description">Bu aralık yeniden açılacak; branşlarda daha önce açtığınız saatler tekrar randevuya sunulacak.</p>
        <div className="decision-summary">
          <div className="decision-time"><CalendarClock /><span>{formatBlockRange(block.start_at, block.end_at, use24Hour)}</span></div>
          <div><strong>{block.reason || 'Açıklama girilmedi'}</strong></div>
        </div>
        {error && <div className="inline-error">{error}</div>}
        <footer className="decision-actions">
          <button type="button" className="decision-secondary" disabled={busy} onClick={onClose}>Vazgeç</button>
          <button type="button" className="decision-primary" autoFocus disabled={busy} onClick={onConfirm}>
            {busy ? <><LoaderCircle className="spin" size={16} /> Kaldırılıyor…</> : 'Kaydı kaldır'}
          </button>
        </footer>
      </div>
    </div>
  );
}

function DecisionModal({ decision, busy, error, onClose, onConfirm }: {
  decision: { appointment: Appointment; action: DecisionAction };
  busy: boolean;
  error: string;
  onClose: () => void;
  onConfirm: (adminNote: string) => void;
}) {
  const [adminNote, setAdminNote] = useState('');
  const dialogRef = useModalShell(busy, onClose);
  const { appointment, action } = decision;
  const config = {
    approve: { eyebrow: 'Randevu onayı', title: 'Randevuyu onayla', description: 'Bu saat kesinleştirilecek ve danışana onay e-postası gönderilecek.', button: 'Randevuyu onayla', icon: CircleCheck },
    reject: { eyebrow: 'Talep sonucu', title: 'Talebi reddet', description: 'Seçilen saat yeniden müsait olacak ve danışana bilgilendirme gönderilecek.', button: 'Talebi reddet', icon: CircleX },
    cancel: { eyebrow: 'Randevu iptali', title: 'Randevuyu iptal et', description: 'Onaylanmış randevu iptal edilecek ve saat yeniden kullanılabilir olacak.', button: 'Randevuyu iptal et', icon: Ban }
  }[action];
  const Icon = config.icon;

  return (
    <div className="decision-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) onClose(); }}>
      <div className={`decision-modal ${action}`} role="dialog" aria-modal="true" aria-labelledby="decision-title" tabIndex={-1} ref={dialogRef}>
        <header className="decision-header">
          <span className="decision-icon"><Icon /></span>
          <div><span className="eyebrow">{config.eyebrow}</span><h2 id="decision-title">{config.title}</h2></div>
          <button type="button" className="decision-close" aria-label="Pencereyi kapat" disabled={busy} onClick={onClose}><X size={18} /></button>
        </header>
        <p className="decision-description">{config.description}</p>
        <div className="decision-summary">
          <div className="decision-time"><CalendarClock /><span>{formatDateTime(appointment.start_at)}</span></div>
          <div><strong>{appointment.name}</strong><span>{appointment.service_name}</span></div>
          <div className="decision-contact"><a href={`tel:${appointment.phone}`}>{appointment.phone}</a><a href={`mailto:${appointment.email}`}>{appointment.email}</a></div>
        </div>
        {action !== 'approve' && <label className="decision-note">{action === 'reject' ? 'Ret açıklaması' : 'İptal açıklaması'} <span>(isteğe bağlı)</span><textarea autoFocus rows={4} maxLength={500} placeholder="Danışana iletilecek kısa bir açıklama yazın…" value={adminNote} onChange={event => setAdminNote(event.target.value)} /><small>{adminNote.length} / 500</small></label>}
        {error && <div className="inline-error">{error}</div>}
        <footer className="decision-actions">
          <button type="button" className="decision-secondary" disabled={busy} onClick={onClose}>Vazgeç</button>
          <button type="button" className="decision-primary" autoFocus={action === 'approve'} disabled={busy} onClick={() => onConfirm(adminNote.trim())}>{busy ? <><LoaderCircle className="spin" size={16} /> İşleniyor…</> : config.button}</button>
        </footer>
      </div>
    </div>
  );
}
