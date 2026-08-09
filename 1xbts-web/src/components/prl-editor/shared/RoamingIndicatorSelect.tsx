import { SearchableSelect } from "./SearchableSelect";

const STANDARD_LABELS: Record<number, string> = {
  0: "Roaming",
  1: "Home",
  2: "Roaming (Flashing)",
  3: "Out of Neighborhood",
  4: "Out of Building",
  5: "Roaming - Preferred System",
  6: "Roaming - Available System",
  7: "Roaming - Alliance Partner",
  8: "Roaming - Premium Partner",
  9: "Roaming - Full Service Functionality",
  10: "Roaming - Partial Service Functionality",
  11: "Roaming Banner On",
  12: "Roaming Banner Off",
};

export function roamIndLabel(raw: number): string {
  if (raw in STANDARD_LABELS) return `${STANDARD_LABELS[raw]} (${raw})`;
  return `Reserved (${raw})`;
}

interface Option {
  value: number;
  label: string;
}

const ALL_OPTIONS: Option[] = Array.from({ length: 256 }, (_, raw) => ({
  value: raw,
  label: roamIndLabel(raw),
}));

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
  return (
    <div className="block">
      <span className="text-muted text-[11px]">{label}</span>
      <SearchableSelect
        className="mt-0.5"
        value={String(value)}
        options={ALL_OPTIONS.map((option) => ({
          value: String(option.value),
          label: option.label,
        }))}
        onChange={(next) => onChange(Number(next))}
        placeholder="Search roaming indications…"
        ariaLabel={label}
        invalid={Boolean(error)}
      />
      {hint && !error && (
        <span className="text-dimmed text-[10px]">{hint}</span>
      )}
      {error && <span className="text-accent-red text-[10px]">{error}</span>}
    </div>
  );
}
