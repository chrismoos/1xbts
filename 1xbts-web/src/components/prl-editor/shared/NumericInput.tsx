// Bounded integer input with bit-width range validation.

import { useState, useEffect } from "react";

export function NumericInput({
  label,
  value,
  onChange,
  min = 0,
  max = Number.MAX_SAFE_INTEGER,
  bits,
  disabled,
  className,
  error,
  hint,
}: {
  label?: string;
  value: number;
  onChange: (next: number) => void;
  min?: number;
  max?: number;
  /** Convenience: when set, `max = 2^bits - 1`. */
  bits?: number;
  disabled?: boolean;
  className?: string;
  error?: string;
  hint?: string;
}) {
  const effectiveMax = bits != null ? (1 << bits) - 1 : max;
  const [text, setText] = useState(String(value));

  useEffect(() => {
    setText(String(value));
  }, [value]);

  const commit = (raw: string) => {
    const n = Number(raw);
    if (Number.isFinite(n) && n >= min && n <= effectiveMax) {
      onChange(Math.floor(n));
    } else {
      // Snap back to the last committed value.
      setText(String(value));
    }
  };

  return (
    <label className={`block ${className ?? ""}`}>
      {label && (
        <span className="text-muted text-[11px]">
          {label}
          {hint && <span className="text-dimmed ml-1">({hint})</span>}
        </span>
      )}
      <input
        type="number"
        className={`block w-full mt-0.5 bg-bg border rounded px-2 py-1 text-xs font-mono disabled:opacity-50 ${
          error ? "border-accent-red" : "border-border"
        }`}
        value={text}
        min={min}
        max={effectiveMax}
        disabled={disabled}
        onChange={(e) => setText(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
      />
      {error && <span className="text-accent-red text-[11px]">{error}</span>}
    </label>
  );
}

export function TextInput({
  label,
  value,
  onChange,
  placeholder,
  maxLength,
  disabled,
  className,
  error,
}: {
  label?: string;
  value: string;
  onChange: (next: string) => void;
  placeholder?: string;
  maxLength?: number;
  disabled?: boolean;
  className?: string;
  error?: string;
}) {
  return (
    <label className={`block ${className ?? ""}`}>
      {label && <span className="text-muted text-[11px]">{label}</span>}
      <input
        type="text"
        className={`block w-full mt-0.5 bg-bg border rounded px-2 py-1 text-xs font-mono disabled:opacity-50 ${
          error ? "border-accent-red" : "border-border"
        }`}
        value={value}
        placeholder={placeholder}
        maxLength={maxLength}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      />
      {error && <span className="text-accent-red text-[11px]">{error}</span>}
    </label>
  );
}
