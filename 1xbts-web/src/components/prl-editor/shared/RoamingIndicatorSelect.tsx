// TSB-58 roaming indicator picker. Values 0–4 are the standardized
// labels; 5–63 are spec-reserved; 64–255 are carrier-private (each
// carrier ships an ERI database mapping these to icons and banner
// text). Without an ERI we label them generically; operators
// authoring private-network PRLs should stick to 0–4 unless they're
// also pushing a matching ERI.

const STANDARD_LABELS: Record<number, string> = {
  0: "On Home",
  1: "Roaming",
  2: "International",
  3: "LTE",
  4: "Flashing",
};

export function roamIndLabel(raw: number): string {
  if (raw in STANDARD_LABELS) return `${STANDARD_LABELS[raw]} (${raw})`;
  if (raw < 64) return `Reserved (${raw})`;
  return `Carrier Specific (${raw})`;
}

interface Option {
  value: number;
  label: string;
  group: "standard" | "reserved" | "carrier";
}

const ALL_OPTIONS: Option[] = Array.from({ length: 256 }, (_, raw) => {
  if (raw in STANDARD_LABELS) {
    return { value: raw, label: roamIndLabel(raw), group: "standard" };
  }
  if (raw < 64) {
    return { value: raw, label: roamIndLabel(raw), group: "reserved" };
  }
  return { value: raw, label: roamIndLabel(raw), group: "carrier" };
});

export function RoamingIndicatorSelect({
  label,
  hint,
  value,
  onChange,
  error,
}: {
  label: string;
  hint?: string;
  value: number;
  onChange: (next: number) => void;
  error?: string;
}) {
  const standard = ALL_OPTIONS.filter((o) => o.group === "standard");
  const reserved = ALL_OPTIONS.filter((o) => o.group === "reserved");
  const carrier = ALL_OPTIONS.filter((o) => o.group === "carrier");
  return (
    <label className="block">
      <span className="text-muted text-[11px]">{label}</span>
      <select
        className={`block w-full mt-0.5 bg-bg border rounded px-2 py-1 text-xs font-mono ${
          error ? "border-accent-red" : "border-border"
        }`}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      >
        <optgroup label="TSB-58 standard">
          {standard.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </optgroup>
        <optgroup label="Reserved (5–63)">
          {reserved.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </optgroup>
        <optgroup label="Carrier Specific (64–255)">
          {carrier.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </optgroup>
      </select>
      {hint && !error && (
        <span className="text-dimmed text-[10px]">{hint}</span>
      )}
      {error && <span className="text-accent-red text-[10px]">{error}</span>}
    </label>
  );
}
