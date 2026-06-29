"use client";

import { useCallback, useEffect, useState } from "react";
import { Card } from "@/components/card";

interface PrlSummary {
  prlId: string;
  name: string;
  isDefault: boolean;
  ssprPRev: number;
}

const SAVE_BTN =
  "bg-accent-blue/20 border border-accent-blue/40 text-accent-blue px-3 py-1 rounded disabled:opacity-50";

export function OtaspOverridesCard({
  subscriberId,
  prlOverride,
  spc,
  analogControlChannel,
  onChanged,
}: {
  subscriberId: string;
  prlOverride: string | undefined;
  spc: string | undefined;
  analogControlChannel: number | undefined;
  onChanged?: () => void;
}) {
  return (
    <Card title="OTASP Provisioning">
      <div className="space-y-4 text-xs">
        <p className="text-muted">
          Values written to this subscriber&apos;s handset during OTASP{" "}
          <span className="font-mono">*228</span>.
        </p>
        <PrlSection
          subscriberId={subscriberId}
          current={prlOverride}
          onChanged={onChanged}
        />
        <div className="border-t border-border" />
        <SpcSection
          subscriberId={subscriberId}
          current={spc}
          onChanged={onChanged}
        />
        <div className="border-t border-border" />
        <AnalogControlChannelSection
          subscriberId={subscriberId}
          current={analogControlChannel}
          onChanged={onChanged}
        />
      </div>
    </Card>
  );
}

function PrlSection({
  subscriberId,
  current,
  onChanged,
}: {
  subscriberId: string;
  current: string | undefined;
  onChanged?: () => void;
}) {
  const [prls, setPrls] = useState<PrlSummary[]>([]);
  const [selected, setSelected] = useState<string>(current ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSelected(current ?? "");
  }, [current]);

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
  const dirty = (current ?? "") !== selected;

  return (
    <section className="space-y-2">
      <h4 className="font-medium">PRL override</h4>
      <p className="text-muted">
        Pushes this PRL instead of the system default. Leave as
        <span className="font-mono"> Default</span> to use whatever&apos;s
        marked default site-wide.
      </p>
      <select
        value={selected}
        onChange={(e) => setSelected(e.target.value)}
        className="block w-full bg-bg border border-border rounded px-2 py-1"
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
      {error && <p className="text-accent-red">{error}</p>}
      {dirty && (
        <button onClick={save} disabled={busy} className={SAVE_BTN}>
          {busy ? "Saving…" : "Save"}
        </button>
      )}
    </section>
  );
}

function SpcSection({
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
    <section className="space-y-2">
      <h4 className="font-medium">Service Programming Code</h4>
      <p className="text-muted">
        6-digit lock code used for Verify SPC. Leave blank to use the IS-95
        default <span className="font-mono">000000</span>.
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
        <button onClick={save} disabled={busy} className={SAVE_BTN}>
          {busy ? "Saving…" : "Save"}
        </button>
      )}
    </section>
  );
}

function AnalogControlChannelSection({
  subscriberId,
  current,
  onChanged,
}: {
  subscriberId: string;
  current: number | undefined;
  onChanged?: () => void;
}) {
  const [value, setValue] = useState<string>(
    current === undefined ? "" : String(current)
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setValue(current === undefined ? "" : String(current));
  }, [current]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const trimmed = value.trim();
      let firstchp: number | null = null;
      if (trimmed) {
        if (!/^\d+$/.test(trimmed)) {
          setError("Channel must be a whole number");
          return;
        }
        const n = Number(trimmed);
        if (n > 2047) {
          setError("Channel must be in 0–2047");
          return;
        }
        firstchp = n;
      }
      const res = await fetch(
        `/api/subscribers/${encodeURIComponent(subscriberId)}/firstchp-override`,
        {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ firstchp }),
        }
      );
      const data = await res.json();
      if (data.error) setError(data.error);
      else onChanged?.();
    } finally {
      setBusy(false);
    }
  };

  const normalized = current === undefined ? "" : String(current);
  const dirty = normalized !== value.trim();

  return (
    <section className="space-y-2">
      <h4 className="font-medium">Analog Control Channel</h4>
      <p className="text-muted">
        First analog control channel the handset scans (FIRSTCHP). Dedicated
        AMPS control channels are <span className="font-mono">313–333</span>{" "}
        (System A) and <span className="font-mono">334–354</span> (System B).
        Leave blank to keep the handset&apos;s existing value.
      </p>
      <label className="block">
        <span className="text-muted">Channel</span>
        <input
          type="text"
          inputMode="numeric"
          value={value}
          placeholder="keep current"
          onChange={(e) => setValue(e.target.value)}
          className="block w-32 mt-1 bg-bg border border-border rounded px-2 py-1 font-mono"
          disabled={busy}
        />
      </label>
      {error && <p className="text-accent-red">{error}</p>}
      {dirty && (
        <button onClick={save} disabled={busy} className={SAVE_BTN}>
          {busy ? "Saving…" : "Save"}
        </button>
      )}
    </section>
  );
}
