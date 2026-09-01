import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Ban, BellRing, CalendarClock, CalendarOff, Check, CheckCheck, ChevronDown, CircleCheck, CircleX, Copy as CopyIcon, Hourglass, LoaderCircle, LogOut, Plus, RefreshCw, Search, ShieldCheck, Trash2, TriangleAlert, UserPlus, X } from 'lucide-react';
import { MonthCalendar } from '../components/MonthCalendar';
import { TimeFormatToggle } from '../components/TimeFormatToggle';
import { api, ApiError } from '../web/api';
import { addDays, addMonths, DAY_MS, formatBlockRange, formatDateTime, formatDuration, formatMonth, formatRelative, formatSelectedDate, formatTime, localDateKey, monthDays, monthKey, monthRange, timeOptions, toTimestamp } from '../web/date';
import type { AdminSlot, Appointment, AppointmentStatus, AvailabilityBlock, Service } from '../web/types';

/** Takvim görünümünün verisi: gezilen ay ve seçili branşla sınırlı, sayfalanmamış. */
type DashboardData = { slots: AdminSlot[]; monthAppointments: Appointment[]; blocks: AvailabilityBlock[] };
/** Randevu listesi: sunucu tarafında filtrelenir ve sayfalanır. */
type StatusCounts = Partial<Record<AppointmentStatus, number>>;
type AppointmentList = { items: Appointment[]; total: number; counts: StatusCounts };

/** server/src/config.rs içindeki BOOKING_RULES ile aynı kalmalıdır. */
const SLOT_MS = 20 * 60_000;
const BUFFER_MS = 10 * 60_000;
/** Yönetici tarafından değiştirilemeyen saat durumları. */
const LOCKED_STATES: SlotState[] = ['pending', 'approved', 'blocked', 'past'];
/** Pazartesi'den başlayan gösterim sırası; değerler Date.getUTCDay() karşılıkları. */
const WEEKDAYS: Array<{ label: string; value: number }> = [
  { label: 'Pzt', value: 1 }, { label: 'Sal', value: 2 }, { label: 'Çar', value: 3 },
  { label: 'Per', value: 4 }, { label: 'Cum', value: 5 }, { label: 'Cmt', value: 6 },
  { label: 'Paz', value: 0 }
];

type SlotChange = { startAt: number; open: boolean };
/** Telefonla gelen danışan için elle randevu girişi. */
type ManualDraft = {
  serviceId: string; date: string; time: string;
  name: string; phone: string; email: string; note: string;
};

function validPhone(value: string): boolean {
  const digits = value.replace(/[^0-9]/g, '');
  return digits.length >= 10 && digits.length <= 15;
}

function validEmail(value: string): boolean {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value);
}

/** Tarih ve saat doğrulaması; hem önizleme hem gönderim kontrolü bunu paylaşır. */
function manualTimeError(draft: ManualDraft): string {
  const startAt = toTimestamp(draft.date, draft.time);
  if (!Number.isFinite(startAt)) return 'Geçerli bir tarih ve saat seçin.';
  if (startAt < Date.now()) return 'Geçmiş bir saate randevu oluşturulamaz.';
  return '';
}

/** Gönderimi engelleyen hata; sunucudaki doğrulamanın istemci karşılığı. */
function validateManual(draft: ManualDraft): string {
  const timeError = manualTimeError(draft);
  if (timeError) return timeError;
  if (draft.name.trim().length < 2) return 'Ad soyad en az 2 karakter olmalıdır.';
  if (!validPhone(draft.phone)) return 'Geçerli bir telefon numarası girin.';
  if (draft.email.trim() && !validEmail(draft.email.trim())) return 'Geçerli bir e-posta adresi girin.';
  return '';
}

/**
 * Kullanıcıya gösterilecek hata. Henüz doldurulmamış zorunlu alanlar için uyarı
 * verilmez — form açılır açılmaz hata göstermek yerine buton kapalı bırakılır;
 * uyarı yalnızca girilen değer hatalıysa çıkar.
 */
function manualVisibleError(draft: ManualDraft): string {
  const timeError = manualTimeError(draft);
  if (timeError) return timeError;
  if (draft.name.trim() && draft.name.trim().length < 2) return 'Ad soyad en az 2 karakter olmalıdır.';
  if (draft.phone.trim() && !validPhone(draft.phone)) return 'Geçerli bir telefon numarası girin.';
  if (draft.email.trim() && !validEmail(draft.email.trim())) return 'Geçerli bir e-posta adresi girin.';
  return '';
}

/** Yeni talep yoklaması. Arka plandaki sekmede tarayıcı zaten kısıtlar. */
const POLL_MS = 60_000;
const PAGE_TITLE = 'Cem Avat · Randevu';

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
  const [list, setList] = useState<AppointmentList>({ items: [], total: 0, counts: {} });
  const [search, setSearch] = useState('');
  /** Her tuş vuruşunda istek atmamak için geciktirilmiş arama metni. */
  const [activeSearch, setActiveSearch] = useState('');
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
  /** Shift ile aralık seçiminde çıpa olarak kullanılan son tıklanan saat. */
  /** Son yüklenen branş+ay; iskelet yalnızca bağlam değişince gösterilir. */
  const loadedContext = useRef('');
  /** Son yüklenen filtre+arama; iskelet yalnızca bunlar değişince gösterilir. */
  const loadedFilter = useRef<string | null>(null);
  /** Kullanıcının en son gördüğü bekleyen talep sayısı; yoklama bunun üstüne bakar. */
  const seenPending = useRef<number | null>(null);
  const [newRequests, setNewRequests] = useState(0);
  const [anchorTime, setAnchorTime] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  const [manual, setManual] = useState<ManualDraft | null>(null);
  const [manualBusy, setManualBusy] = useState(false);
  const [manualError, setManualError] = useState('');

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
    // Ay veya branş değişince eldeki veri geçersizdir ve iskelet gösterilir.
    // Aynı bağlamda tazeleme yapılırken içerik yerinde kalır: liste yanıp sönmez,
    // kullanıcı tıklamak üzereyken düğmeler yer değiştirmez.
    const context = `${serviceId}:${month}`;
    if (loadedContext.current !== context) setLoading(true);
    setError('');
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
    finally { loadedContext.current = context; setLoading(false); }
  }, [guard, month, serviceId]);

  useEffect(() => { loadData(); }, [loadData]);

  // Yazarken her tuşta istek atılmaz; kullanıcı durunca sorgulanır.
  useEffect(() => {
    const timer = window.setTimeout(() => setActiveSearch(search.trim()), 300);
    return () => window.clearTimeout(timer);
  }, [search]);

  /** Listeyi baştan yükler. `count` mevcut derinliği korumak için kullanılır. */
  const loadAppointments = useCallback(async (count = PAGE_SIZE) => {
    const context = `${filter}:${activeSearch}`;
    if (loadedFilter.current !== context) setListLoading(true);
    try {
      const query = new URLSearchParams({ limit: String(Math.min(count, CALENDAR_LIMIT)), offset: '0' });
      if (filter) query.set('status', filter);
      if (activeSearch) query.set('q', activeSearch);
      const result = await api<{ appointments: Appointment[]; total: number; counts: StatusCounts }>(`/api/admin/appointments?${query}`);
      setList({ items: result.appointments, total: result.total, counts: result.counts });
    } catch (err) { guard(err); }
    finally { loadedFilter.current = context; setListLoading(false); }
  }, [activeSearch, filter, guard]);

  // Filtre veya arama değiştiğinde liste ilk sayfadan yeniden yüklenir.
  useEffect(() => { loadAppointments(); }, [loadAppointments]);

  async function loadMoreAppointments() {
    setListMoreBusy(true);
    try {
      const query = new URLSearchParams({ limit: String(PAGE_SIZE), offset: String(list.items.length) });
      if (filter) query.set('status', filter);
      if (activeSearch) query.set('q', activeSearch);
      const result = await api<{ appointments: Appointment[]; total: number; counts: StatusCounts }>(`/api/admin/appointments?${query}`);
      setList(current => ({ items: [...current.items, ...result.appointments], total: result.total, counts: result.counts }));
    } catch (err) { guard(err); }
    finally { setListMoreBusy(false); }
  }

  /**
   * Filtresiz bekleyen talep sayısı. Yoklama sessizdir: hata kullanıcıya
   * gösterilmez, yalnızca oturum düştüyse giriş ekranına dönülür.
   */
  const fetchPendingCount = useCallback(async (): Promise<number | null> => {
    try {
      const result = await api<{ counts: StatusCounts }>('/api/admin/appointments?limit=1');
      return result.counts.PENDING ?? 0;
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) onExpired();
      return null;
    }
  }, [onExpired]);

  /** Bildirimi kapatır ve yeni talep ölçümünü şu ana sabitler. */
  const markPendingSeen = useCallback(async () => {
    const count = await fetchPendingCount();
    if (count !== null) seenPending.current = count;
    setNewRequests(0);
  }, [fetchPendingCount]);

  /** Karar sonrası hem takvimi hem listeyi tazeler; liste derinliği korunur. */
  const refreshAll = useCallback(async () => {
    await Promise.all([loadData(), loadAppointments(Math.max(PAGE_SIZE, list.items.length))]);
    // Karar bekleyen sayısını değiştirir; ölçüm yeniden sabitlenmezse
    // sonraki yeni talep fark edilmezdi.
    await markPendingSeen();
  }, [loadData, loadAppointments, list.items.length, markPendingSeen]);

  // Yeni talepleri yoklar. Liste kendiliğinden değiştirilmez: kullanıcı bir
  // randevuyu onaylamak üzereyken kartlar kayarsa yanlış kayıt onaylanabilir.
  // Bunun yerine bildirim gösterilir, uygulama kararı kullanıcıya bırakılır.
  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      const count = await fetchPendingCount();
      if (cancelled || count === null) return;
      if (seenPending.current === null) { seenPending.current = count; return; }
      setNewRequests(Math.max(0, count - seenPending.current));
    };
    poll();
    const timer = window.setInterval(poll, POLL_MS);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [fetchPendingCount]);

  // Sekme arka plandayken de görünsün diye sayfa başlığına yansıtılır.
  useEffect(() => {
    document.title = newRequests ? `(${newRequests}) ${PAGE_TITLE}` : PAGE_TITLE;
    return () => { document.title = PAGE_TITLE; };
  }, [newRequests]);

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
    const end = start + SLOT_MS;
    const block = data.blocks.find(item => item.start_at < end + BUFFER_MS && item.end_at > start);
    if (block) return { state: 'blocked', block };
    if (start < Date.now()) return { state: 'past' };
    return { state: openSet.has(start) ? 'open' : 'closed' };
  }

  /**
   * Saatleri tek istekte açar/kapatır ve arayüzü hemen günceller.
   * Bir günü açmak 28 istek ve 28 yeniden yükleme demekti; artık tek tur.
   * İstek başarısız olursa takvim sunucudan yeniden yüklenip gerçek duruma döner.
   */
  async function applySlots(changes: SlotChange[], describe: (count: number) => string) {
    if (!changes.length) { notify('Değişecek saat yok.'); return; }
    const touched = new Set(changes.map(change => change.startAt));
    setBusySlots(current => new Set([...current, ...touched]));
    setError('');

    setData(current => {
      const slots = new Map(current.slots.map(slot => [slot.start_at, slot]));
      for (const change of changes) {
        if (change.open) {
          slots.set(change.startAt, {
            id: `optimistic-${change.startAt}`, service_id: serviceId,
            start_at: change.startAt, end_at: change.startAt + SLOT_MS, created_at: Date.now()
          });
        } else {
          slots.delete(change.startAt);
        }
      }
      return { ...current, slots: [...slots.values()].sort((a, b) => a.start_at - b.start_at) };
    });

    try {
      await api('/api/admin/availability-slots/bulk', {
        method: 'PUT',
        body: JSON.stringify({
          serviceId,
          slots: changes.map(change => ({ start: new Date(change.startAt).toISOString(), open: change.open }))
        })
      }, csrf);
      notify(describe(changes.length));
    } catch (err) {
      guard(err);
      await loadData();
    } finally {
      setBusySlots(current => {
        const next = new Set(current);
        for (const start of touched) next.delete(start);
        return next;
      });
    }
  }

  /** Bir günün değiştirilebilir saatleri için hedef durumu hesaplar; değişmeyenleri eler. */
  function daySlotChanges(dateKey: string, target: (startAt: number) => boolean): SlotChange[] {
    return timeOptions().flatMap(time => {
      const startAt = toTimestamp(dateKey, time);
      const state = stateFor(startAt).state;
      if (LOCKED_STATES.includes(state)) return [];
      const open = target(startAt);
      return (state === 'open') === open ? [] : [{ startAt, open }];
    });
  }

  /** Saat satırına tıklama. Shift ile önceki tıklamadan buraya kadarki aralık uygulanır. */
  function onSlotClick(time: string, shiftKey: boolean) {
    const times = timeOptions();
    const index = times.indexOf(time);
    const start = toTimestamp(selectedDate, time);
    const open = stateFor(start).state !== 'open';
    const anchorIndex = anchorTime ? times.indexOf(anchorTime) : -1;
    setAnchorTime(time);

    if (shiftKey && anchorIndex >= 0 && anchorIndex !== index) {
      const [from, to] = anchorIndex < index ? [anchorIndex, index] : [index, anchorIndex];
      const window = new Set(times.slice(from, to + 1));
      const changes = daySlotChanges(selectedDate, startAt => window.has(formatTime(startAt, true)) ? open : (stateFor(startAt).state === 'open'));
      void applySlots(changes, count => open ? `${count} saat açıldı.` : `${count} saat kapatıldı.`);
      return;
    }
    void applySlots([{ startAt: start, open }], () => open ? 'Saat randevuya açıldı.' : 'Saat kapatıldı.');
  }

  function setWholeDay(open: boolean) {
    void applySlots(
      daySlotChanges(selectedDate, () => open),
      count => open ? `Gün açıldı — ${count} saat.` : `Gün kapatıldı — ${count} saat.`
    );
  }

  /**
   * Seçili günün açık saatlerini gezilen ay içindeki hedef günlere birebir kopyalar.
   * Randevusu olan, genel kapalı ve geçmiş saatlere dokunulmaz.
   */
  function planCopy(weekdays: Set<number>): { days: string[]; changes: SlotChange[] } {
    const sourceOpen = new Set(
      timeOptions().filter(time => stateFor(toTimestamp(selectedDate, time)).state === 'open')
    );
    const days: string[] = [];
    const changes: SlotChange[] = [];
    for (const { date, inMonth } of monthDays(month)) {
      if (!inMonth || date === selectedDate) continue;
      if (!weekdays.has(new Date(`${date}T00:00:00Z`).getUTCDay())) continue;
      const dayChanges = daySlotChanges(date, startAt => sourceOpen.has(formatTime(startAt, true)));
      if (dayChanges.length) { days.push(date); changes.push(...dayChanges); }
    }
    return { days, changes };
  }

  async function copyDay(weekdays: Set<number>) {
    const { days, changes } = planCopy(weekdays);
    setCopying(false);
    await applySlots(changes, () => `${days.length} güne kopyalandı — ${changes.length} saat.`);
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

  /**
   * Modalı seçili gün ve branşla ön doldurarak açar. Saat listesinden çağrıldığında
   * o saat de gelir, böylece tarih/saat seçimi yapmadan doğrudan yazmaya başlanır.
   */
  function openManual(time = '10:00') {
    setManualError('');
    setManual({
      serviceId, date: selectedDate, time,
      name: '', phone: '', email: '', note: ''
    });
  }

  async function createManual(draft: ManualDraft) {
    setManualBusy(true); setManualError('');
    try {
      await api('/api/admin/appointments', {
        method: 'POST',
        body: JSON.stringify({
          serviceId: draft.serviceId,
          start: new Date(toTimestamp(draft.date, draft.time)).toISOString(),
          name: draft.name.trim(),
          phone: draft.phone.trim(),
          email: draft.email.trim(),
          note: draft.note.trim()
        })
      }, csrf);
      setManual(null);
      notify('Randevu oluşturuldu.');
      // Kayıt başka bir aya düştüyse takvimi oraya taşı, aksi halde görünmezdi.
      const targetMonth = monthKey(draft.date);
      if (targetMonth !== month) { setMonth(targetMonth); setSelectedDate(draft.date); }
      await refreshAll();
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) guard(err);
      else setManualError(err instanceof Error ? err.message : 'Randevu oluşturulamadı.');
    } finally { setManualBusy(false); }
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
            <div className="day-actions">
              <button type="button" disabled={loading} onClick={() => setWholeDay(true)}><CheckCheck size={14} /> Tümünü aç</button>
              <button type="button" disabled={loading} onClick={() => setWholeDay(false)}><Ban size={14} /> Tümünü kapat</button>
              <button type="button" disabled={loading} onClick={() => setCopying(true)}><CopyIcon size={14} /> Kopyala</button>
            </div>
            <p className="day-hint">Aralık seçmek için bir saate, sonra <kbd>Shift</kbd> ile ikinci saate tıklayın.</p>
            <div className="admin-time-list">
              {loading ? <div className="panel-state"><LoaderCircle className="spin" /> Takvim yükleniyor…</div> : timeOptions().map(time => {
                const start = toTimestamp(selectedDate, time);
                const info = stateFor(start);
                const busy = busySlots.has(start);
                const locked = ['pending', 'approved', 'blocked', 'past'].includes(info.state);
                const label = { open: 'Açık', closed: 'Kapalı', pending: 'Onay bekliyor', approved: 'Onaylandı', blocked: 'Genel kapalı', past: 'Geçmiş' }[info.state];
                // Yalnızca boş ve gelecekteki saatlere elle randevu girilebilir;
                // dolu, kapalı zaman ve geçmiş satırlarda buton gösterilmez.
                const bookable = info.state === 'open' || info.state === 'closed';
                return (
                  <div className={`admin-time-row ${info.state}${anchorTime === time ? ' anchor' : ''}`} key={time}>
                    <button type="button" className="time-main" disabled={locked || busy} onClick={event => onSlotClick(time, event.shiftKey)} title={info.appointment ? `${info.appointment.name} · ${info.appointment.phone}` : info.block?.reason}>
                      <strong>{formatTime(start, use24Hour)}</strong>
                      <span>{info.appointment?.name || label}</span>
                      {busy ? <LoaderCircle className="spin" size={18} /> : <i aria-label={label}><b /></i>}
                    </button>
                    {bookable && (
                      <button
                        type="button"
                        className="time-add"
                        title="Bu saate randevu ekle"
                        aria-label={`${formatTime(start, use24Hour)} saatine randevu ekle`}
                        onClick={() => openManual(time)}
                      ><UserPlus size={14} /></button>
                    )}
                  </div>
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

        <AppointmentSection
          list={list}
          loading={listLoading}
          moreBusy={listMoreBusy}
          filter={filter}
          search={search}
          searching={search.trim() !== activeSearch}
          onFilter={setFilter}
          onSearch={setSearch}
          onLoadMore={loadMoreAppointments}
          onDecide={openDecision}
          onCreate={openManual}
        />
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
      {manual && (
        <ManualModal
          initial={manual}
          services={services}
          busy={manualBusy}
          error={manualError}
          onClose={() => { if (!manualBusy) setManual(null); }}
          onConfirm={createManual}
        />
      )}
      {copying && (
        <CopyDayModal
          sourceDate={selectedDate}
          month={month}
          sourceOpenCount={timeOptions().filter(time => stateFor(toTimestamp(selectedDate, time)).state === 'open').length}
          plan={planCopy}
          busy={busySlots.size > 0}
          onClose={() => setCopying(false)}
          onConfirm={copyDay}
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
      {newRequests > 0 && (
        <button
          type="button"
          className="new-requests"
          onClick={async () => {
            setFilter('PENDING');
            await refreshAll();
            document.querySelector('.appointment-section')?.scrollIntoView({ behavior: 'smooth', block: 'start' });
          }}
        >
          <BellRing size={15} />
          {newRequests === 1 ? '1 yeni randevu talebi' : `${newRequests} yeni randevu talebi`}
          <span>Göster</span>
        </button>
      )}
      {toast && <div className="toast" role="status"><Check size={17} /> {toast}</div>}
    </div>
  );
}

/** Geri sayımların canlı kalması için düzenli aralıkla tazelenen zaman damgası. */
function useTicker(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return now;
}

function AppointmentSection({ list, loading, moreBusy, filter, search, searching, onFilter, onSearch, onLoadMore, onDecide, onCreate }: {
  list: AppointmentList;
  loading: boolean;
  moreBusy: boolean;
  filter: AppointmentStatus | '';
  search: string;
  searching: boolean;
  onFilter: (value: AppointmentStatus | '') => void;
  onSearch: (value: string) => void;
  onLoadMore: () => void;
  onDecide: (appointment: Appointment, action: DecisionAction) => void;
  onCreate: () => void;
}) {
  // Tutma süresi dakika çözünürlüğünde gösterildiği için yarım dakikalık tik yeterli.
  const now = useTicker(30_000);
  // "Tümü" sekmesi durum dağılımının toplamıdır; sayaçlar aramaya göre daralır.
  const totalCount = Object.values(list.counts).reduce((sum, count) => sum + count, 0);
  const countOf = (value: AppointmentStatus | '') => (value ? list.counts[value] ?? 0 : totalCount);

  return (
    <section className="appointment-section">
      <header>
        <div>
          <span className="eyebrow">Randevular</span>
          <h2>Talep ve onaylar</h2>
          {/* Sarmalanmadan geçilirse React tıklama olayını `time` parametresine
              yollar ve varsayılan saat devre dışı kalır. */}
          <button type="button" className="manual-add" onClick={() => onCreate()}>
            <UserPlus size={15} /> Telefonla randevu ekle
          </button>
        </div>
        <div className="appointment-tools">
          <div className="search-field">
            {searching ? <LoaderCircle className="spin" size={15} /> : <Search size={15} />}
            <input
              type="search"
              value={search}
              placeholder="Ad, e-posta veya telefon ara…"
              aria-label="Randevularda ara"
              maxLength={100}
              onChange={event => onSearch(event.target.value)}
            />
            {search && (
              <button type="button" aria-label="Aramayı temizle" onClick={() => onSearch('')}><X size={14} /></button>
            )}
          </div>
          <div className="filter-tabs">
            {filters.map(value => (
              <button type="button" key={value || 'all'} className={filter === value ? 'active' : ''} onClick={() => onFilter(value)}>
                {value ? statusLabels[value] : 'Tümü'}
                <b>{countOf(value)}</b>
              </button>
            ))}
          </div>
        </div>
      </header>
      <div className="appointment-list">
        {loading && <div className="panel-state"><LoaderCircle className="spin" /> Randevular yükleniyor…</div>}
        {!loading && !list.items.length && (
          <div className="panel-state">
            {search ? `“${search}” için randevu bulunamadı.` : 'Bu filtrede randevu bulunmuyor.'}
          </div>
        )}
        {!loading && list.items.map(item => (
          <AppointmentCard key={item.id} appointment={item} now={now} onDecide={onDecide} />
        ))}
      </div>
      {!loading && list.total > 0 && (
        <footer className="appointment-footer">
          <span>{list.items.length} / {list.total} randevu gösteriliyor</span>
          {list.items.length < list.total && (
            <button type="button" disabled={moreBusy} onClick={onLoadMore}>
              {moreBusy
                ? <><LoaderCircle className="spin" size={15} /> Yükleniyor…</>
                : <><ChevronDown size={15} /> Daha fazla yükle</>}
            </button>
          )}
        </footer>
      )}
    </section>
  );
}

function AppointmentCard({ appointment, now, onDecide }: {
  appointment: Appointment;
  now: number;
  onDecide: (appointment: Appointment, action: DecisionAction) => void;
}) {
  const decidable = ['PENDING', 'CONFLICT'].includes(appointment.status);
  // Tutma süresi yalnızca PENDING kayıtlarda işler; diğer durumlar sunucuda süresi dolmaz.
  const holdLeft = appointment.status === 'PENDING' ? appointment.hold_expires_at - now : null;
  const urgency = holdLeft === null ? ''
    : holdLeft <= 3_600_000 ? ' urgent'
    : holdLeft <= 3 * 3_600_000 ? ' soon' : '';

  return (
    <article className="appointment-card">
      <div className="appointment-date">
        <div><CalendarClock /><span>{formatDateTime(appointment.start_at)}</span></div>
        <small title={formatDateTime(appointment.created_at)}>{formatRelative(appointment.created_at, now)} talep edildi</small>
      </div>

      <div className="appointment-person">
        <h3>{appointment.name}</h3>
        <p>{appointment.service_name}</p>
        <p><a href={`tel:${appointment.phone}`}>{appointment.phone}</a> · <a href={`mailto:${appointment.email}`}>{appointment.email}</a></p>
        {appointment.note && <blockquote>{appointment.note}</blockquote>}
        {appointment.admin_note && (
          <div className="admin-note"><strong>Yönetici notu</strong>{appointment.admin_note}</div>
        )}
      </div>

      <div className="appointment-status">
        <span className={`status-badge ${appointment.status.toLowerCase()}`}>{statusLabels[appointment.status]}</span>
        {holdLeft !== null && (
          <span className={`hold-left${urgency}`} title={`Talep ${formatDateTime(appointment.hold_expires_at)} tarihinde otomatik olarak düşer`}>
            <Hourglass size={12} />
            {holdLeft > 0 ? `${formatDuration(holdLeft)} kaldı` : 'Süresi doldu'}
          </span>
        )}
        {appointment.decision_at !== null && (
          <small title={`Karar: ${formatDateTime(appointment.decision_at)}`}>{formatRelative(appointment.decision_at, now)}</small>
        )}
      </div>

      <div className="appointment-actions">
        {decidable && <><button className="approve" onClick={() => onDecide(appointment, 'approve')}>Onayla</button><button onClick={() => onDecide(appointment, 'reject')}>Reddet</button></>}
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

function ManualModal({ initial, services, busy, error, onClose, onConfirm }: {
  initial: ManualDraft;
  services: Service[];
  busy: boolean;
  error: string;
  onClose: () => void;
  onConfirm: (draft: ManualDraft) => void;
}) {
  const [draft, setDraft] = useState(initial);
  const dialogRef = useModalShell(busy, onClose);
  const update = (patch: Partial<ManualDraft>) => setDraft(current => ({ ...current, ...patch }));
  const blocking = validateManual(draft);
  const visibleError = manualVisibleError(draft);
  const startAt = toTimestamp(draft.date, draft.time);

  return (
    <div className="decision-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) onClose(); }}>
      <div className="decision-modal manual-modal" role="dialog" aria-modal="true" aria-labelledby="manual-title" tabIndex={-1} ref={dialogRef}>
        <header className="decision-header">
          <span className="decision-icon"><UserPlus /></span>
          <div><span className="eyebrow">Elle giriş</span><h2 id="manual-title">Telefonla randevu ekle</h2></div>
          <button type="button" className="decision-close" aria-label="Pencereyi kapat" disabled={busy} onClick={onClose}><X size={18} /></button>
        </header>
        <p className="decision-description">
          Kayıt doğrudan onaylı olarak açılır. Yayınlanmamış bir saati de verebilirsiniz;
          yalnızca başka bir randevu veya kapalı zamanla çakışamaz.
        </p>

        <div className="manual-fields">
          <label className="wide">Branş
            <select value={draft.serviceId} onChange={event => update({ serviceId: event.target.value })}>
              {services.map(service => <option key={service.id} value={service.id}>{service.name}</option>)}
            </select>
          </label>
          <label>Tarih
            <input type="date" required value={draft.date} onChange={event => update({ date: event.target.value })} />
          </label>
          <label>Saat
            <select value={draft.time} onChange={event => update({ time: event.target.value })}>
              {timeOptions().map(time => <option key={time} value={time}>{time}</option>)}
            </select>
          </label>
          <label className="wide">Ad soyad
            <input type="text" required maxLength={100} autoFocus value={draft.name} placeholder="Danışanın adı" onChange={event => update({ name: event.target.value })} />
          </label>
          <label>Telefon
            <input type="tel" required value={draft.phone} placeholder="0555 111 22 33" onChange={event => update({ phone: event.target.value })} />
          </label>
          <label>E-posta <span>(isteğe bağlı)</span>
            <input type="email" value={draft.email} placeholder="Girilirse onay e-postası gider" onChange={event => update({ email: event.target.value })} />
          </label>
        </div>

        {!manualTimeError(draft) && (
          <div className="manual-preview">
            <CalendarClock />
            <span>{formatDateTime(startAt)}</span>
          </div>
        )}

        <label className="decision-note">Not <span>(isteğe bağlı)</span>
          <textarea rows={2} maxLength={500} placeholder="Görüşmeye dair kısa bir not…" value={draft.note} onChange={event => update({ note: event.target.value })} />
          <small>{draft.note.length} / 500</small>
        </label>

        {(visibleError || error) && <div className="inline-error">{visibleError || error}</div>}

        <footer className="decision-actions">
          <button type="button" className="decision-secondary" disabled={busy} onClick={onClose}>Vazgeç</button>
          <button type="button" className="decision-primary" disabled={busy || !!blocking} onClick={() => onConfirm(draft)}>
            {busy ? <><LoaderCircle className="spin" size={16} /> Oluşturuluyor…</> : 'Randevuyu oluştur'}
          </button>
        </footer>
      </div>
    </div>
  );
}

function CopyDayModal({ sourceDate, month, sourceOpenCount, plan, busy, onClose, onConfirm }: {
  sourceDate: string;
  month: string;
  sourceOpenCount: number;
  plan: (weekdays: Set<number>) => { days: string[]; changes: SlotChange[] };
  busy: boolean;
  onClose: () => void;
  onConfirm: (weekdays: Set<number>) => void;
}) {
  // Varsayılan hedef, kaynak günle aynı hafta günü: haftalık düzen kurmanın en sık hâli.
  const sourceWeekday = new Date(`${sourceDate}T00:00:00Z`).getUTCDay();
  const [weekdays, setWeekdays] = useState<Set<number>>(new Set([sourceWeekday]));
  const dialogRef = useModalShell(busy, onClose);

  const preview = plan(weekdays);
  const toggle = (value: number) => setWeekdays(current => {
    const next = new Set(current);
    if (!next.delete(value)) next.add(value);
    return next;
  });

  return (
    <div className="decision-backdrop" onMouseDown={event => { if (event.target === event.currentTarget && !busy) onClose(); }}>
      <div className="decision-modal copy-modal" role="dialog" aria-modal="true" aria-labelledby="copy-title" tabIndex={-1} ref={dialogRef}>
        <header className="decision-header">
          <span className="decision-icon"><CopyIcon /></span>
          <div><span className="eyebrow">Müsaitlik planı</span><h2 id="copy-title">Günü kopyala</h2></div>
          <button type="button" className="decision-close" aria-label="Pencereyi kapat" disabled={busy} onClick={onClose}><X size={18} /></button>
        </header>
        <p className="decision-description">
          <strong>{formatSelectedDate(sourceDate)}</strong> gününün {sourceOpenCount} açık saati, {formatMonth(month)} içindeki
          seçili günlere birebir uygulanır. Randevusu olan, genel kapalı ve geçmiş saatlere dokunulmaz.
        </p>

        <div className="copy-weekdays" role="group" aria-label="Hedef günler">
          {WEEKDAYS.map(day => (
            <button
              type="button"
              key={day.value}
              className={weekdays.has(day.value) ? 'active' : ''}
              aria-pressed={weekdays.has(day.value)}
              onClick={() => toggle(day.value)}
            >{day.label}</button>
          ))}
        </div>

        <div className={`copy-preview ${preview.changes.length ? '' : 'empty'}`}>
          <CalendarClock />
          <span>
            {preview.changes.length
              ? `${preview.days.length} gün · ${preview.changes.length} saat değişecek`
              : 'Seçili günlerde değişecek saat yok'}
          </span>
        </div>

        <footer className="decision-actions">
          <button type="button" className="decision-secondary" disabled={busy} onClick={onClose}>Vazgeç</button>
          <button type="button" className="decision-primary" disabled={busy || !preview.changes.length} onClick={() => onConfirm(weekdays)}>
            {busy ? <><LoaderCircle className="spin" size={16} /> Uygulanıyor…</> : 'Saatleri kopyala'}
          </button>
        </footer>
      </div>
    </div>
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
