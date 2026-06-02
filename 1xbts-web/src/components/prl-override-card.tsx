"use client";

import { useEffect, useState, useCallback } from "react";
import { Card } from "@/components/card";

interface PrlSummary {
  prlId: string;
  name: string;
  isDefault: boolean;
  ssprPRev: number;
}

export function PrlOverrideCard({
  subscriberId,
  currentOverride,
  onChanged,
}: {
  subscriberId: string;
  currentOverride: string | undefined;
  onChanged?: () => void;
}) {
  const [prls, setPrls] = useState<PrlSummary[]>([]);
  const [selected, setSelected] = useState<string>(currentOverride ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSelected(currentOverride ?? "");
  }, [currentOverride]);

  const load = useCallback(async () => {
    try {
      const res = await fetch("/api/prls");
      const data = await res.json();
      if (!data.error) setPrls(data.prls ?? []);
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await fetch(
        `/api/subscribers/${encodeURIComponent(subscriberId)}/prl-override`,
        {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ prlId: selected || null }),
        }
      );
      const data = await res.json();
      if (data.error) setError(data.error);
      else onChanged?.();
    } finally {
      setBusy(false);
    }
  };

  const defaultPrl = prls.find((p) => p.isDefault);
  const dirty = (currentOverride ?? "") !== selected;

  return (
    <Card title="PRL override">
      <div className="space-y-2 text-xs">
        <p className="text-muted">
          OTASP <span className="font-mono">*228</span> pushes this PRL to this
          subscriber instead of the system default. Leave as
          <span className="font-mono"> Default</span> to use whatever's marked
          default site-wide.
        </p>
        <label className="block">
          <span className="text-muted">PRL</span>
          <select
            value={selected}
            onChange={(e) => setSelected(e.target.value)}
            className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1"
            disabled={busy}
          >
            <option value="">
              Default {defaultPrl ? `(${defaultPrl.name})` : "(none set)"}
            </option>
            {prls.map((p) => (
              <option key={p.prlId} value={p.prlId}>
                {p.name} ({p.ssprPRev === 1 ? "classic" : "extended"})
              </option>
            ))}
          </select>
        </label>
        {error && <p className="text-accent-red">{error}</p>}
        {dirty && (
          <button
            onClick={save}
            disabled={busy}
            className="bg-accent-blue/20 border border-accent-blue/40 text-accent-blue px-3 py-1 rounded disabled:opacity-50"
          >
            {busy ? "Saving…" : "Save override"}
          </button>
        )}
      </div>
    </Card>
  );
}
