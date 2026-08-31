export const TIME_ZONE = 'Europe/Istanbul';
export const DAY_MS = 86_400_000;

export function localDateKey(value: number | Date = Date.now()): string {
  const parts = new Intl.DateTimeFormat('en-GB', {
    timeZone: TIME_ZONE,
    year: 'numeric', month: '2-digit', day: '2-digit'
  }).formatToParts(new Date(value));
  const map = Object.fromEntries(parts.map(part => [part.type, part.value]));
  return `${map.year}-${map.month}-${map.day}`;
}

export function monthKey(dateKey: string): string {
  return dateKey.slice(0, 7);
}

export function monthStart(value: string): string {
  return `${value.slice(0, 7)}-01`;
}

export function addMonths(value: string, amount: number): string {
  const date = new Date(`${monthStart(value)}T00:00:00Z`);
  date.setUTCMonth(date.getUTCMonth() + amount);
  return date.toISOString().slice(0, 10);
}

export function addDays(value: string, amount: number): string {
  const date = new Date(`${value}T00:00:00Z`);
  date.setUTCDate(date.getUTCDate() + amount);
  return date.toISOString().slice(0, 10);
}

export function monthRange(value: string): { from: number; to: number } {
  const fromKey = monthStart(value);
  const toKey = addMonths(fromKey, 1);
  return { from: toTimestamp(fromKey, '00:00'), to: toTimestamp(toKey, '00:00') };
}

export function monthDays(value: string): Array<{ date: string; inMonth: boolean }> {
  const first = monthStart(value);
  const firstDate = new Date(`${first}T00:00:00Z`);
  const mondayOffset = (firstDate.getUTCDay() + 6) % 7;
  const gridStart = addDays(first, -mondayOffset);
  return Array.from({ length: 42 }, (_, index) => {
    const date = addDays(gridStart, index);
    return { date, inMonth: monthKey(date) === monthKey(first) };
  });
}

export function toTimestamp(dateKey: string, time: string): number {
  return Date.parse(`${dateKey}T${time}:00+03:00`);
}

export function formatMonth(value: string): string {
  const text = new Intl.DateTimeFormat('tr-TR', { month: 'long', year: 'numeric', timeZone: 'UTC' })
    .format(new Date(`${monthStart(value)}T00:00:00Z`));
  return text.charAt(0).toLocaleUpperCase('tr-TR') + text.slice(1);
}

export function formatSelectedDate(value: string): string {
  return new Intl.DateTimeFormat('tr-TR', { weekday: 'short', day: '2-digit', month: 'long', timeZone: 'UTC' })
    .format(new Date(`${value}T00:00:00Z`));
}

export function formatDateTime(value: number | string): string {
  return new Intl.DateTimeFormat('tr-TR', {
    dateStyle: 'medium', timeStyle: 'short', timeZone: TIME_ZONE
  }).format(new Date(value));
}

export function formatTime(value: number | string, use24Hour = true): string {
  return new Intl.DateTimeFormat('tr-TR', {
    hour: '2-digit', minute: '2-digit', hour12: !use24Hour, timeZone: TIME_ZONE
  }).format(new Date(value));
}

export function timeOptions(): string[] {
  const options: string[] = [];
  for (let minute = 8 * 60; minute < 22 * 60; minute += 30) {
    options.push(`${String(Math.floor(minute / 60)).padStart(2, '0')}:${String(minute % 60).padStart(2, '0')}`);
  }
  return options;
}

const dayMonthYear = new Intl.DateTimeFormat('tr-TR', {
  day: 'numeric', month: 'long', year: 'numeric', timeZone: TIME_ZONE
});
const dayMonth = new Intl.DateTimeFormat('tr-TR', {
  day: 'numeric', month: 'long', timeZone: TIME_ZONE
});

/**
 * Kapalı zamanın bitişi dışlayıcıdır: 3 Eylül 00:00 → 4 Eylül 00:00 yalnızca 3 Eylül'ü kapatır.
 * Bu yüzden son kapalı gün, bitişten bir milisaniye öncesidir.
 */
function lastCoveredDay(endAt: number): number {
  return endAt - 1;
}

/** Aralık yerel gece yarısından gece yarısına mı uzanıyor? */
export function isFullDayRange(startAt: number, endAt: number): boolean {
  return endAt > startAt
    && startAt === toTimestamp(localDateKey(startAt), '00:00')
    && endAt === toTimestamp(localDateKey(endAt), '00:00');
}

/** Kapalı zaman aralığını okunur biçime çevirir. */
export function formatBlockRange(startAt: number, endAt: number, use24Hour = true): string {
  if (isFullDayRange(startAt, endAt)) {
    const last = lastCoveredDay(endAt);
    return localDateKey(startAt) === localDateKey(last)
      ? `${dayMonthYear.format(startAt)} · Tüm gün`
      : `${dayMonth.format(startAt)} – ${dayMonthYear.format(last)} · Tüm gün`;
  }
  if (localDateKey(startAt) === localDateKey(endAt)) {
    return `${dayMonthYear.format(startAt)} · ${formatTime(startAt, use24Hour)} – ${formatTime(endAt, use24Hour)}`;
  }
  return `${dayMonthYear.format(startAt)} ${formatTime(startAt, use24Hour)}`
    + ` → ${dayMonthYear.format(endAt)} ${formatTime(endAt, use24Hour)}`;
}
