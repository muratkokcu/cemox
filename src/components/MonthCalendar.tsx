import { ChevronLeft, ChevronRight } from 'lucide-react';
import { formatMonth, localDateKey, monthDays } from '../web/date';

type Props = {
  month: string;
  selectedDate: string;
  enabledDates?: Set<string>;
  dateTones?: Map<string, 'open' | 'pending' | 'approved' | 'mixed'>;
  minMonth?: string;
  maxMonth?: string;
  onMonthChange: (direction: -1 | 1) => void;
  onSelectDate: (date: string) => void;
};

const weekdays = ['PZT', 'SAL', 'ÇAR', 'PER', 'CUM', 'CMT', 'PAZ'];

export function MonthCalendar({
  month, selectedDate, enabledDates, dateTones, minMonth, maxMonth, onMonthChange, onSelectDate
}: Props) {
  const today = localDateKey();
  const cells = monthDays(month);
  const canGoBack = !minMonth || month.slice(0, 7) > minMonth.slice(0, 7);
  const canGoForward = !maxMonth || month.slice(0, 7) < maxMonth.slice(0, 7);

  return (
    <section className="calendar-panel" aria-label="Randevu takvimi">
      <header className="calendar-heading">
        <h2>{formatMonth(month)}</h2>
        <div className="calendar-nav">
          <button type="button" aria-label="Önceki ay" disabled={!canGoBack} onClick={() => onMonthChange(-1)}>
            <ChevronLeft size={19} />
          </button>
          <button type="button" aria-label="Sonraki ay" disabled={!canGoForward} onClick={() => onMonthChange(1)}>
            <ChevronRight size={19} />
          </button>
        </div>
      </header>
      <div className="weekday-row">
        {weekdays.map(day => <span key={day}>{day}</span>)}
      </div>
      <div className="month-grid">
        {cells.map(({ date, inMonth }) => {
          const enabled = inMonth && (!enabledDates || enabledDates.has(date));
          const tone = dateTones?.get(date);
          return (
            <button
              type="button"
              key={date}
              className={[
                'day-cell', inMonth ? '' : 'outside', enabled ? 'enabled' : 'disabled',
                selectedDate === date ? 'selected' : '', today === date ? 'today' : '', tone ? `tone-${tone}` : ''
              ].filter(Boolean).join(' ')}
              disabled={!enabled}
              aria-label={date}
              aria-pressed={selectedDate === date}
              onClick={() => onSelectDate(date)}
            >
              <span>{Number(date.slice(-2))}</span>
              {tone && <i aria-hidden="true" />}
            </button>
          );
        })}
      </div>
    </section>
  );
}
