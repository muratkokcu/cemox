import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, Check, Clock3, Dumbbell, Globe2, LoaderCircle, ShieldCheck } from 'lucide-react';
import { api, ApiError } from '../web/api';
import { addMonths, formatSelectedDate, formatTime, localDateKey, monthKey } from '../web/date';
import type { Availability, BookingRules, Service, Slot } from '../web/types';
import { MonthCalendar } from './MonthCalendar';
import { TimeFormatToggle } from './TimeFormatToggle';

type FormState = {
  name: string; email: string; phone: string; note: string; consent: boolean; website: string;
};

const emptyForm: FormState = { name: '', email: '', phone: '', note: '', consent: false, website: '' };

export function BookingScheduler({ preferredServiceId = '' }: { preferredServiceId?: string }) {
  const [services, setServices] = useState<Service[]>([]);
  const [serviceId, setServiceId] = useState('');
  const [availability, setAvailability] = useState<Availability | null>(null);
  const [month, setMonth] = useState(monthKey(localDateKey()));
  const [selectedDate, setSelectedDate] = useState('');
  const [selectedSlot, setSelectedSlot] = useState<Slot | null>(null);
  const [step, setStep] = useState<'times' | 'details' | 'done'>('times');
  const [use24Hour, setUse24Hour] = useState(true);
  const [form, setForm] = useState<FormState>(emptyForm);
  const [rules, setRules] = useState<BookingRules>({ sessionMinutes: 90, stepMinutes: 90 });
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');
  const startedAt = useRef(Date.now());

  useEffect(() => {
    api<{ services: Service[]; rules?: BookingRules }>('/api/services')
      .then(result => {
        setServices(result.services);
        if (result.rules) setRules(result.rules);
        setServiceId(result.services[0]?.id || '');
      })
      .catch(err => setError(err.message));
  }, []);

  useEffect(() => {
    if (preferredServiceId && services.some(service => service.id === preferredServiceId)) setServiceId(preferredServiceId);
  }, [preferredServiceId, services]);

  useEffect(() => {
    if (!serviceId) return;
    setLoading(true);
    setError('');
    setSelectedSlot(null);
    setStep('times');
    startedAt.current = Date.now();
    api<Availability>(`/api/availability?service=${encodeURIComponent(serviceId)}`)
      .then(result => {
        setAvailability(result);
        const firstDate = result.days[0]?.date || '';
        setSelectedDate(firstDate);
        if (firstDate) setMonth(monthKey(firstDate));
      })
      .catch(err => setError(err.message))
      .finally(() => setLoading(false));
  }, [serviceId]);

  const enabledDates = useMemo(() => new Set(availability?.days.map(day => day.date) || []), [availability]);
  const selectedDay = availability?.days.find(day => day.date === selectedDate);
  const currentService = services.find(service => service.id === serviceId);

  function chooseDate(date: string) {
    setSelectedDate(date);
    setSelectedSlot(null);
    setStep('times');
  }

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!selectedSlot) return;
    setSubmitting(true);
    setError('');
    try {
      await api('/api/appointments', {
        method: 'POST',
        body: JSON.stringify({
          serviceId,
          start: selectedSlot.start,
          ...form,
          startedAt: startedAt.current
        })
      });
      setStep('done');
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Randevu oluşturulamadı.');
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="booking-shell" id="randevu">
      <aside className="event-panel">
        <div className="trainer">
          <img src="/galery/1E7A6619.webp" alt="Cem Avat" />
          <div><span>Antrenör</span><strong>Cem Avat</strong></div>
        </div>
        <span className="eyebrow">Antrenman saati seç</span>
        <h1>{currentService?.name || 'Antrenman'}</h1>
        <div className="event-facts">
          <p><Clock3 size={18} /><span>{formatSessionLength(rules.sessionMinutes)}</span></p>
          <p><Dumbbell size={18} /><span>Birebir antrenman</span></p>
          <p><Globe2 size={18} /><span>Europe / Istanbul</span></p>
        </div>
        <label className="service-field">
          <span>Branş / hizmet</span>
          <select value={serviceId} onChange={event => setServiceId(event.target.value)}>
            {services.map(service => <option value={service.id} key={service.id}>{service.name}</option>)}
          </select>
        </label>
        <p className="approval-note"><ShieldCheck size={17} /> Seçtiğiniz saat antrenör onayından sonra kesinleşir.</p>
      </aside>

      <MonthCalendar
        month={month}
        selectedDate={selectedDate}
        enabledDates={enabledDates}
        minMonth={availability?.rangeStart}
        maxMonth={availability?.rangeEnd}
        onSelectDate={chooseDate}
        onMonthChange={direction => setMonth(monthKey(addMonths(`${month}-01`, direction)))}
      />

      <section className="times-panel" aria-live="polite">
        {step === 'done' ? (
          <SuccessState onReset={() => {
            setForm(emptyForm);
            setSelectedSlot(null);
            setStep('times');
            startedAt.current = Date.now();
          }} />
        ) : step === 'details' && selectedSlot ? (
          <BookingForm
            form={form}
            serviceName={currentService?.name || ''}
            selectedSlot={selectedSlot}
            submitting={submitting}
            error={error}
            onBack={() => { setStep('times'); setError(''); }}
            onChange={setForm}
            onSubmit={submit}
          />
        ) : (
          <>
            <header className="times-heading">
              <h2>{selectedDate ? formatSelectedDate(selectedDate) : 'Saat seçin'}</h2>
              <TimeFormatToggle use24Hour={use24Hour} onChange={setUse24Hour} />
            </header>
            <div className="time-list">
              {loading && <div className="panel-state"><LoaderCircle className="spin" /> Müsaitlik yükleniyor…</div>}
              {!loading && error && <div className="inline-error">{error}</div>}
              {!loading && !error && !selectedDay && <div className="panel-state">Bu ay için açık antrenman saati bulunmuyor.</div>}
              {!loading && selectedDay?.slots.map(slot => {
                const active = selectedSlot?.start === slot.start;
                return (
                  <div className={`time-choice ${active ? 'chosen' : ''}`} key={slot.start}>
                    <button type="button" onClick={() => setSelectedSlot(slot)}>
                      {formatTime(slot.start, use24Hour)}
                    </button>
                    {active && <button type="button" className="next-button" onClick={() => setStep('details')}>Devam</button>}
                  </div>
                );
              })}
            </div>
          </>
        )}
      </section>
    </div>
  );
}

function BookingForm({ form, selectedSlot, serviceName, submitting, error, onBack, onChange, onSubmit }: {
  form: FormState;
  selectedSlot: Slot;
  serviceName: string;
  submitting: boolean;
  error: string;
  onBack: () => void;
  onChange: (value: FormState) => void;
  onSubmit: (event: React.FormEvent) => void;
}) {
  const field = (key: keyof FormState, value: string | boolean) => onChange({ ...form, [key]: value });
  return (
    <form className="booking-form" onSubmit={onSubmit}>
      <button type="button" className="back-button" onClick={onBack}><ArrowLeft size={17} /> Saatlere dön</button>
      <h2>Bilgilerinizi girin</h2>
      <p className="selection-summary">{serviceName}<br /><strong>{formatDateForSummary(selectedSlot.start)}</strong></p>
      {error && <div className="inline-error">{error}</div>}
      <label>Ad soyad<input required minLength={2} maxLength={100} autoComplete="name" value={form.name} onChange={e => field('name', e.target.value)} /></label>
      <label>E-posta<input required type="email" maxLength={160} autoComplete="email" value={form.email} onChange={e => field('email', e.target.value)} /></label>
      <label>Telefon<input required type="tel" autoComplete="tel" placeholder="+90 5xx xxx xx xx" value={form.phone} onChange={e => field('phone', e.target.value)} /></label>
      <label>Kısa not <span>(isteğe bağlı)</span><textarea maxLength={500} rows={3} value={form.note} onChange={e => field('note', e.target.value)} /></label>
      <label className="consent"><input required type="checkbox" checked={form.consent} onChange={e => field('consent', e.target.checked)} /><span>Bilgilerimin antrenman planlaması için kullanılmasını kabul ediyorum.</span></label>
      <input className="honey" tabIndex={-1} autoComplete="off" value={form.website} onChange={e => field('website', e.target.value)} />
      <button className="submit-button" disabled={submitting}>{submitting ? 'Gönderiliyor…' : 'Antrenman saatini talep et'}</button>
    </form>
  );
}

function SuccessState({ onReset }: { onReset: () => void }) {
  return (
    <div className="success-state">
      <span><Check size={28} /></span>
      <h2>Talebiniz alındı</h2>
      <p>Seçtiğiniz saat 24 saat boyunca sizin için tutulacak. Antrenmanınız, antrenör onayından sonra kesinleşecek.</p>
      <button type="button" onClick={onReset}>Başka bir saat seç</button>
    </div>
  );
}

function formatDateForSummary(value: string): string {
  return new Intl.DateTimeFormat('tr-TR', {
    weekday: 'long', day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit', timeZone: 'Europe/Istanbul'
  }).format(new Date(value));
}

/** 90 → "1 saat 30 dakika"; süre sunucudan geldiği için burada sabit yazılmaz. */
function formatSessionLength(minutes: number): string {
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (!hours) return `${rest} dakika`;
  return rest ? `${hours} saat ${rest} dakika` : `${hours} saat`;
}
