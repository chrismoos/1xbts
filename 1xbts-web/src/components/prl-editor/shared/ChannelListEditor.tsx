// Reusable channel/value list: integer rows with add/remove and a
// configurable bit-width clamp. Used for cellular custom channels,
// PCS channels, JTACS channels, etc. — all the *Custom and
// *UsingChannels variants.

import { useState } from "react";

export function ChannelListEditor({
  label,
  values,
  onChange,
  bits = 11,
  min = 0,
  disabled,
  maxRows,
}: {
  label?: string;
  values: number[];
  onChange: (next: number[]) => void;
  bits?: number;
  min?: number;
  disabled?: boolean;
  /** Caps the row count (defaults to 2^count_bits - 1). */
  maxRows?: number;
}) {
  const [draft, setDraft] = useState("");
  const max = (1 << bits) - 1;

  const add = () => {
    const n = Number(draft);
    if (!Number.isFinite(n)) return;
    if (n < min || n > max) return;
    if (maxRows && values.length >= maxRows) return;
    onChange([...values, Math.floor(n)]);
    setDraft("");
  };

  const removeAt = (index: number) =>
    onChange(values.filter((_, i) => i !== index));

  const updateAt = (index: number, next: number) => {
    const clamped = Math.max(min, Math.min(max, Math.floor(next)));
    onChange(values.map((v, i) => (i === index ? clamped : v)));
  };

  return (
    <div className="space-y-1">
      {label && (
        <div className="text-muted text-[11px]">
          {label}{" "}
          <span className="text-dimmed">
            ({values.length} entries, {bits}-bit, 0–{max})
          </span>
        </div>
      )}
      {values.length > 0 && (
        <div className="grid grid-cols-[auto_1fr_auto] gap-1 items-center">
          {values.map((v, i) => (
            <div
              key={i}
              className="contents"
            >
              <span className="font-mono text-dimmed text-[11px]">{i}</span>
              <input
                type="number"
                className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
                value={v}
                min={min}
                max={max}
                disabled={disabled}
                onChange={(e) => updateAt(i, Number(e.target.value))}
              />
              <button
                type="button"
                disabled={disabled}
                onClick={() => removeAt(i)}
                className="text-accent-red text-[11px] px-1 hover:underline disabled:opacity-50"
              >
                ✕
              </button>
            </div>
          ))}
        </div>
      )}
      <div className="flex gap-1">
        <input
          type="number"
          className="flex-1 bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
          value={draft}
          min={min}
          max={max}
          placeholder={`Add (0–${max})`}
          disabled={disabled || (maxRows != null && values.length >= maxRows)}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
        />
        <button
          type="button"
          onClick={add}
          disabled={disabled || draft === ""}
          className="text-accent-blue text-[11px] px-2 hover:underline disabled:opacity-50"
        >
          + Add
        </button>
      </div>
    </div>
  );
}
