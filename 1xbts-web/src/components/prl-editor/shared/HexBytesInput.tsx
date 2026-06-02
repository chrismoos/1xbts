// Hex-bytes editor. Display + edit as uppercase pairs ("CAFE01");
// accept space/comma separators on input. The container is bounded
// by `lengthBits` — byte count must equal ceil(lengthBits / 8).
// Server-side packs MSB-first into the wire bit stream.

import { useEffect, useState } from "react";

function normaliseHex(input: string): string {
  return input
    .replace(/[\s,_-]/g, "")
    .toUpperCase();
}

function isValidHex(s: string): boolean {
  return /^[0-9A-F]*$/.test(s);
}

export function HexBytesInput({
  label,
  value,
  onChange,
  lengthBits,
  disabled,
  error,
}: {
  label?: string;
  /** Hex string, uppercase, no separators. */
  value: string;
  onChange: (next: string) => void;
  /** Required byte count derived as ceil(lengthBits / 8). */
  lengthBits: number;
  disabled?: boolean;
  error?: string;
}) {
  const requiredBytes = Math.ceil(lengthBits / 8);
  const requiredChars = requiredBytes * 2;
  const [draft, setDraft] = useState(value);

  useEffect(() => {
    setDraft(value);
  }, [value]);

  const commit = (raw: string) => {
    const cleaned = normaliseHex(raw);
    if (!isValidHex(cleaned)) {
      setDraft(value);
      return;
    }
    // Pad with trailing zeros if short; truncate if long.
    let next = cleaned;
    if (next.length < requiredChars) {
      next = next + "0".repeat(requiredChars - next.length);
    } else if (next.length > requiredChars) {
      next = next.slice(0, requiredChars);
    }
    setDraft(next);
    onChange(next);
  };

  return (
    <label className="block">
      {label && (
        <span className="text-muted text-[11px]">
          {label}{" "}
          <span className="text-dimmed">
            ({lengthBits} bits / {requiredBytes} octets, hex)
          </span>
        </span>
      )}
      <input
        type="text"
        className={`block w-full mt-0.5 bg-bg border rounded px-2 py-1 text-xs font-mono uppercase disabled:opacity-50 ${
          error ? "border-accent-red" : "border-border"
        }`}
        value={draft}
        disabled={disabled}
        placeholder={requiredBytes > 0 ? "00".repeat(requiredBytes) : "(empty)"}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
      />
      {error && <span className="text-accent-red text-[11px]">{error}</span>}
    </label>
  );
}
