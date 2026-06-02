"use client";

import { useEffect, useState } from "react";
import { Card } from "@/components/card";

export function SpcOverrideCard({
  subscriberId,
  current,
  onChanged,
}: {
  subscriberId: string;
  current: string | undefined;
  onChanged?: () => void;
}) {
  const [value, setValue] = useState<string>(current ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setValue(current ?? "");
  }, [current]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const trimmed = value.trim();
      if (trimmed && !/^\d{6}$/.test(trimmed)) {
        setError("SPC must be exactly 6 digits");
        return;
      }
      const res = await fetch(
        `/api/subscribers/${encodeURIComponent(subscriberId)}/spc`,
        {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ spc: trimmed || null }),
        }
      );
      const data = await res.json();
      if (data.error) setError(data.error);
      else onChanged?.();
    } finally {
      setBusy(false);
    }
  };

  const dirty = (current ?? "") !== value;

  return (
    <Card title="Service Programming Code">
      <div className="space-y-2 text-xs">
        <p className="text-muted">
          6-digit lock code used during OTASP{" "}
          <span className="font-mono">*228</span> Verify SPC. Leave blank to
          use the IS-95 default <span className="font-mono">000000</span>.
        </p>
        <label className="block">
          <span className="text-muted">SPC</span>
          <input
            type="text"
            inputMode="numeric"
            maxLength={6}
            value={value}
            placeholder="000000"
            onChange={(e) => setValue(e.target.value)}
            className="block w-32 mt-1 bg-bg border border-border rounded px-2 py-1 font-mono"
            disabled={busy}
          />
        </label>
        {error && <p className="text-accent-red">{error}</p>}
        {dirty && (
          <button
            onClick={save}
            disabled={busy}
            className="bg-accent-blue/20 border border-accent-blue/40 text-accent-blue px-3 py-1 rounded disabled:opacity-50"
          >
            {busy ? "Saving…" : "Save SPC"}
          </button>
        )}
      </div>
    </Card>
  );
}
