"use client";

import { Suspense, useEffect, useState } from "react";
import { useSearchParams } from "next/navigation";
import { CellSelector } from "@/components/cell-selector";
import { EvdoChip } from "@/components/evdo";
import { EvdoSessionTable, useEvdoSessionRows } from "@/components/evdo-session-table";
import type { EvdoLinkState } from "@/lib/evdo";
import { useLive } from "@/lib/live";
import type { GetUatiAllocationResponse } from "@/lib/proto/an/v1/service";
import type { EvdoCarrierConfig } from "@/lib/proto/bsc/v1/service";
import { useBtsList, usePinnedDefaultCell } from "@/lib/use-bts-list";
import { ErrorNotice } from "@/components/error-notice";

type Filter = "all" | EvdoLinkState | "refused";

const FILTERS: { key: Filter; label: string }[] = [
  { key: "all", label: "All" },
  { key: "connected", label: "Connected" },
  { key: "dormant", label: "Dormant" },
  { key: "refused", label: "Refused" },
];

function anHref(path: string, cell: string | null, baseStation: string | null): string {
  const query = new URLSearchParams();
  if (cell) query.set("cell", cell);
  if (baseStation) query.set("baseStation", baseStation);
  const suffix = query.toString();
  return suffix ? `${path}?${suffix}` : path;
}

export default function EvdoSessionsPage() {
  return (
    <Suspense fallback={null}>
      <EvdoSessionsView />
    </Suspense>
  );
}

function EvdoSessionsView() {
  const searchParams = useSearchParams();
  const requested = searchParams.get("cell");
  const baseStation = searchParams.get("baseStation");
  const [evdo, setEvdo] = useState<EvdoCarrierConfig | null>(null);
  const [evdoReady, setEvdoReady] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");
  const { cells, loading: cellsLoading } = useBtsList();

  // The carrier belongs to one cell, so a URL without one waits for the cell
  // list and then names the default rather than asking for an unnamed cell.
  const pinnedDefault = usePinnedDefaultCell(cells);
  const carrierCell = requested ?? pinnedDefault;
  const carrierAddressed = requested !== null || !cellsLoading;

  useEffect(() => {
    if (!carrierAddressed) return;
    let cancelled = false;
    fetch(anHref("/api/bts-config", carrierCell, baseStation))
      .then((r) => r.json())
      .then((data: { evdo?: EvdoCarrierConfig; error?: string }) => {
        if (cancelled || data.error) return;
        setEvdo(data.evdo ?? null);
        setEvdoReady(true);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [carrierAddressed, carrierCell, baseStation]);

  // The AN only answers for an EV-DO-enabled cell, so hold the subscriptions
  // until the carrier config confirms it.
  const { rows, sessions, error } = useEvdoSessionRows(evdo ? anHref("/api/an-sessions", carrierCell, baseStation) : null);
  const liveAllocation = useLive<GetUatiAllocationResponse & { error?: string }>(
    evdo ? anHref("/api/an-uati-allocation", carrierCell, baseStation) : null,
  );
  const allocation = liveAllocation.data && !liveAllocation.data.error ? liveAllocation.data : null;

  const counts = {
    all: rows.length,
    connected: rows.filter((row) => row.link === "connected").length,
    dormant: rows.filter((row) => row.link === "dormant").length,
    idle: rows.filter((row) => row.link === "idle").length,
    refused: rows.filter((row) => row.refused).length,
  };
  const shown = rows.filter((row) =>
    filter === "all" ? true : filter === "refused" ? row.refused : row.link === filter,
  );
  const accessClosed = sessions.some((session) => session.admission?.accessClosed);
  const usedPct =
    allocation && allocation.capacity > 0 ? Math.min(100, (allocation.inUse / allocation.capacity) * 100) : 0;

  return (
    <div className="page-container space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold">EV-DO Sessions</h1>
          {evdo && (
            <div className="mt-1 flex flex-wrap items-center gap-x-5 gap-y-1 text-xs text-muted">
              <span>
                Carrier <span className="font-mono text-secondary">BC{evdo.bandClass} ch {evdo.channel}</span>
              </span>
              <span>
                Color <span className="font-mono text-secondary">{evdo.colorCode}</span>
              </span>
              <span>
                Sector <span className="font-mono text-secondary">{evdo.sectorId}</span>
              </span>
              {sessions.length > 0 && (
                <EvdoChip
                  label={accessClosed ? "Access closed · subscribers only" : "Access open"}
                  className="text-accent-indigo-text bg-accent-indigo-bg"
                />
              )}
            </div>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <CellSelector selected={carrierCell} />
          <div className="inline-flex overflow-hidden rounded-lg border border-border">
            {FILTERS.map((f) => (
              <button
                key={f.key}
                type="button"
                onClick={() => setFilter(f.key)}
                className={`px-3 py-1 text-xs ${filter === f.key ? "bg-surface-raised text-primary" : "text-muted hover:text-secondary"}`}
              >
                {f.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      <ErrorNotice error={error} />

      {evdoReady && !evdo && (
        <div className="glass-card p-4 text-sm text-muted">EV-DO not enabled on this cell.</div>
      )}

      {evdo && (
        <>
          <div className="flex flex-wrap gap-2.5">
            <Pill value={counts.all} label="sessions" />
            <Pill value={counts.connected} label="connected" className="text-accent-cyan" />
            <Pill value={counts.dormant} label="dormant" className="text-accent-amber" />
            <Pill value={counts.idle} label="idle" className="text-muted" />
            <Pill value={counts.refused} label="refused" className="text-accent-red" />
            {allocation && (
              <div className="flex min-w-[230px] items-center gap-3 rounded-xl border border-border bg-surface px-3 py-2">
                <span className="text-xs text-muted">UATI pool · color {allocation.colorCode}</span>
                <div className="h-1.5 flex-1 overflow-hidden rounded bg-surface-raised">
                  <div className="h-full bg-accent-indigo" style={{ width: `${usedPct}%` }} />
                </div>
                <span className="font-mono text-[11px]">
                  {allocation.inUse} / {allocation.capacity}
                </span>
              </div>
            )}
          </div>

          <div className="glass-card overflow-x-auto p-4">
            <EvdoSessionTable
              rows={shown}
              emptyLabel={rows.length === 0 ? "No sessions" : "No sessions match this filter"}
            />
          </div>
        </>
      )}
    </div>
  );
}

function Pill({ value, label, className }: { value: number; label: string; className?: string }) {
  return (
    <div className="flex items-baseline gap-2 rounded-xl border border-border bg-surface px-3 py-2">
      <span className={`text-lg font-semibold ${className ?? "text-primary"}`}>{value}</span>
      <span className="text-xs text-muted">{label}</span>
    </div>
  );
}
