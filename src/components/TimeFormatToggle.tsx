type Props = { use24Hour: boolean; onChange: (value: boolean) => void };

export function TimeFormatToggle({ use24Hour, onChange }: Props) {
  return (
    <div className="time-format" aria-label="Saat biçimi">
      <button type="button" className={!use24Hour ? 'active' : ''} onClick={() => onChange(false)}>12s</button>
      <button type="button" className={use24Hour ? 'active' : ''} onClick={() => onChange(true)}>24s</button>
    </div>
  );
}
