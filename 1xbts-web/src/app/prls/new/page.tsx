"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { Card } from "@/components/card";
import {
  emptyClassicPrl,
  emptyExtendedPrl,
  runningSystemCarriers,
  runningSystemPrl,
  type RunningSystemCarrierSelection,
} from "@/lib/prl-empty";
import { type BtsConfig, EvdoTxMode } from "@/lib/proto/bsc/v1/service";

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

type Mode = "running" | "upload" | "classic" | "extended";

export default function NewPrlPage() {
  const router = useRouter();
  const [mode, setMode] = useState<Mode>("running");
  const [name, setName] = useState("");
  const [notes, setNotes] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [prListId, setPrListId] = useState("1");
  const [runningConfig, setRunningConfig] = useState<BtsConfig | null>(null);
  const [selectedCarriers, setSelectedCarriers] =
    useState<RunningSystemCarrierSelection>({ oneX: false, hrpd: false });
  const [configLoading, setConfigLoading] = useState(true);
  const [configError, setConfigError] = useState<string | null>(null);
  const [configRequest, setConfigRequest] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const runningReady =
    mode !== "running" ||
    (!configLoading &&
      runningConfig !== null &&
      (selectedCarriers.oneX || selectedCarriers.hrpd));

  useEffect(() => {
    let cancelled = false;
    setConfigLoading(true);
    setConfigError(null);
    fetch("/api/bts-config")
      .then(async (response) => {
        const data = (await response.json()) as BtsConfig & { error?: string };
        if (!response.ok || data.error) {
          throw new Error(data.error ?? "Could not read the running system.");
        }
        return data;
      })
      .then((config) => {
        if (cancelled) return;
        setRunningConfig(config);
        setSelectedCarriers(runningSystemCarriers(config));
      })
      .catch((err) => {
        if (cancelled) return;
        setRunningConfig(null);
        setConfigError(
          err instanceof Error ? err.message : "Could not read the running system.",
        );
      })
      .finally(() => {
        if (!cancelled) setConfigLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [configRequest]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) {
      setError("Name is required.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      let body: Record<string, unknown>;
      if (mode === "upload") {
        if (!file) {
          setError("Upload a .prl file.");
          return;
        }
        const buffer = new Uint8Array(await file.arrayBuffer());
        body = {
          name: name.trim(),
          notes,
          rawBytesBase64: bytesToBase64(buffer),
        };
      } else {
        const id = Number(prListId);
        if (!Number.isFinite(id) || id < 0 || id > 0xffff) {
          setError("PR_LIST_ID must be 0–65535.");
          return;
        }
        if (mode === "running" && !runningConfig) {
          setError("The running system configuration is not available.");
          return;
        }
        const built =
          mode === "classic"
            ? emptyClassicPrl(id)
            : mode === "extended"
              ? emptyExtendedPrl(id)
              : runningSystemPrl(id, runningConfig!, selectedCarriers);
        body = { name: name.trim(), notes, built };
      }
      const res = await fetch("/api/prls", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      const data = await res.json();
      if (!res.ok || data.error) {
        setError(data.error ?? "save failed");
        return;
      }
      const newId = data.prl?.summary?.prlId;
      router.push(newId ? `/prls/${newId}` : "/prls");
    } catch (err) {
      setError(err instanceof Error ? err.message : "save failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <h1 className="text-primary text-lg font-medium">New PRL</h1>

      <div className="flex gap-1 border border-border rounded text-xs w-fit">
        <ModeTab id="running" current={mode} onClick={setMode}>
          Running system
        </ModeTab>
        <ModeTab id="upload" current={mode} onClick={setMode}>
          Upload .prl
        </ModeTab>
        <ModeTab id="classic" current={mode} onClick={setMode}>
          Build classic
        </ModeTab>
        <ModeTab id="extended" current={mode} onClick={setMode}>
          Build extended
        </ModeTab>
      </div>

      <Card title={modeLabel(mode)}>
        <form onSubmit={submit} className="space-y-3 text-xs">
          <label className="block">
            <span className="text-muted">Name</span>
            <input
              className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="My PRL"
              maxLength={120}
            />
          </label>
          <label className="block">
            <span className="text-muted">Notes (optional)</span>
            <textarea
              className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1 font-mono"
              rows={2}
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
            />
          </label>

          {mode === "upload" && (
            <label className="block">
              <span className="text-muted">PRL file</span>
              <input
                type="file"
                accept=".prl,application/octet-stream"
                className="block w-full mt-1 text-primary"
                onChange={(e) => setFile(e.target.files?.[0] ?? null)}
              />
            </label>
          )}

          {mode === "running" && (
            <RunningSystemPicker
              config={runningConfig}
              selected={selectedCarriers}
              loading={configLoading}
              error={configError}
              onChange={setSelectedCarriers}
              onRetry={() => setConfigRequest((request) => request + 1)}
            />
          )}

          {mode !== "upload" && (
            <label className="block">
              <span className="text-muted">Initial PRL ID (0–65535)</span>
              <input
                type="number"
                className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1 font-mono"
                min={0}
                max={0xffff}
                value={prListId}
                onChange={(e) => setPrListId(e.target.value)}
                placeholder="e.g. 1"
              />
            </label>
          )}

          {error && <p className="text-accent-red">{error}</p>}

          <div className="flex gap-2">
            <button
              type="submit"
              disabled={busy || !runningReady}
              className="bg-accent-blue/20 border border-accent-blue/40 text-accent-blue px-3 py-1 rounded hover:bg-accent-blue/30 disabled:opacity-50"
            >
              {busy
                ? "Creating…"
                : mode === "upload"
                  ? "Upload"
                  : mode === "running"
                    ? "Generate PRL"
                    : "Create empty PRL"}
            </button>
            <button
              type="button"
              onClick={() => router.push("/prls")}
              className="text-muted px-3 py-1 hover:text-primary"
            >
              Cancel
            </button>
          </div>
        </form>
      </Card>
    </div>
  );
}

function ModeTab({
  id,
  current,
  onClick,
  children,
}: {
  id: Mode;
  current: Mode;
  onClick: (m: Mode) => void;
  children: React.ReactNode;
}) {
  const active = id === current;
  return (
    <button
      type="button"
      onClick={() => onClick(id)}
      className={`px-3 py-1 ${
        active ? "bg-accent-blue/20 text-accent-blue" : "text-muted hover:text-primary"
      }`}
    >
      {children}
    </button>
  );
}

function modeLabel(m: Mode): string {
  switch (m) {
    case "running": return "Generate for the running system";
    case "upload": return "Upload an existing PRL";
    case "classic": return "Build a new classic PRL (SSPR_P_REV = 1)";
    case "extended": return "Build a new extended PRL (SSPR_P_REV = 3)";
  }
}

function RunningSystemPicker({
  config,
  selected,
  loading,
  error,
  onChange,
  onRetry,
}: {
  config: BtsConfig | null;
  selected: RunningSystemCarrierSelection;
  loading: boolean;
  error: string | null;
  onChange: (selected: RunningSystemCarrierSelection) => void;
  onRetry: () => void;
}) {
  if (loading) {
    return (
      <div className="rounded border border-border bg-bg/40 px-3 py-3 text-muted">
        Reading the running system…
      </div>
    );
  }
  if (error || !config) {
    return (
      <div className="rounded border border-accent-red/40 bg-accent-red/5 px-3 py-3">
        <p className="text-accent-red">{error ?? "Running system unavailable."}</p>
        <button
          type="button"
          onClick={onRetry}
          className="mt-2 rounded border border-border px-2 py-1 text-secondary hover:text-primary"
        >
          Try again
        </button>
      </div>
    );
  }

  const available = runningSystemCarriers(config);
  return (
    <fieldset className="rounded border border-border bg-bg/40 px-3 py-3">
      <legend className="px-1 text-muted">Include detected carriers</legend>
      <p className="mb-2 text-muted">
        The generated extended PRL opens in the editor before you publish it.
      </p>
      <div className="space-y-2">
        {available.oneX && (
          <CarrierChoice
            checked={selected.oneX}
            onChange={(checked) => onChange({ ...selected, oneX: checked })}
            title="1x cdma2000"
            detail={`${config.bandClass.toUpperCase()} · Channel ${config.cdmaChannel} · SID ${config.overhead?.sid ?? "—"} · ${config.overhead?.nid === 0xffff ? "Any NID" : `NID ${config.overhead?.nid ?? "—"}`}`}
          />
        )}
        {available.hrpd && config.evdo && (
          <CarrierChoice
            checked={selected.hrpd}
            onChange={(checked) => onChange({ ...selected, hrpd: checked })}
            title="EV-DO (HRPD)"
            detail={`BC${config.evdo.bandClass} · Channel ${config.evdo.channel} · Sector ${config.evdo.sectorId} · Subnet /${config.evdo.subnetMask}`}
          />
        )}
      </div>
      {config.evdo?.mode === EvdoTxMode.EVDO_TX_MODE_HRPD_ONLY && (
        <p className="mt-2 text-muted">The BTS is running in HRPD-only mode.</p>
      )}
    </fieldset>
  );
}

function CarrierChoice({
  checked,
  onChange,
  title,
  detail,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  title: string;
  detail: string;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-2 rounded border border-border px-3 py-2 hover:border-accent-blue/50">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className="mt-0.5 accent-[var(--color-accent-blue)]"
      />
      <span>
        <span className="block text-primary">{title}</span>
        <span className="block break-all font-mono text-muted">{detail}</span>
      </span>
    </label>
  );
}
