"use client";

import { Suspense, useMemo } from "react";
import { useSearchParams } from "next/navigation";
import { Card } from "@/components/card";
import { CellSelector } from "@/components/cell-selector";
import type { MobileInfo } from "@/components/mobiles-table";
import { type Channel, TrafficChannelsCard } from "@/components/traffic-channels";
import { cellTitle, cellToken, shortBaseStation } from "@/lib/cell";
import { SummaryTile } from "@/components/summary-tile";
import { ferColor, mergeCalls } from "@/lib/traffic-calls";
import type { CellId } from "@/lib/proto/bsc/v1/service";
import { formatPowerFraction } from "@/lib/format";
import { useLive } from "@/lib/live";
import { useBtsList, usePinnedDefaultCell } from "@/lib/use-bts-list";
import { ErrorNotice } from "@/components/error-notice";


interface CellWalshCapacity {
  cell?: CellId;
  totalWalshCodes: number;
}

interface ChannelListResponse {
  channels: Channel[];
  totalWalshCodes: number;
  cellWalshCapacity?: CellWalshCapacity[];
  error?: string;
}


function typeColor(type: string): string {
  switch (type) {
    case "pilot": return "bg-badge-blue-bg text-badge-blue-text";
    case "sync": return "bg-badge-purple-bg text-badge-purple-text";
    case "paging": return "bg-badge-yellow-bg text-badge-yellow-text";
    case "access": return "bg-badge-orange-bg text-badge-orange-text";
    case "traffic": return "bg-badge-green-bg text-badge-green-text";
    default: return "bg-surface-raised text-muted";
  }
}



function overheadName(ch: Channel): string {
  if (ch.walshCode != null) return `W${ch.walshCode}`;
  if (ch.accessChannelNumber != null) return `ACH ${ch.accessChannelNumber}`;
  return "-";
}

function overheadDetail(ch: Channel): string {
  return [
    ch.channelType === "paging" && ch.pagingChannelNumber != null ? `PCH ${ch.pagingChannelNumber}` : null,
    ch.dataRateBps ? `${ch.dataRateBps} bps` : null,
    ch.powerFraction != null ? formatPowerFraction(ch.powerFraction) : null,
  ]
    .filter(Boolean)
    .join(" · ");
}


export default function ChannelsPage() {
  return (
    <Suspense fallback={null}>
      <ChannelsView />
    </Suspense>
  );
}

function ChannelsView() {
  const params = useSearchParams();
  const requested = params.get("cell");
  const requestedBs = params.get("baseStation") ?? "";
  const { cells } = useBtsList();
  // Channels belong to a cell, so the page shows one cell at a time. Resolve
  // the active cell (requested or the pinned default) and its base station.
  const pinnedDefault = usePinnedDefaultCell(cells);
  const activeToken = requested ?? pinnedDefault;
  const activeCell =
    cells.find(
      (c) =>
        cellToken(c.cell) === activeToken &&
        (!requestedBs || c.baseStation === requestedBs),
    ) ?? cells.find((c) => cellToken(c.cell) === activeToken);
  const baseStation = requestedBs || activeCell?.baseStation || "";
  const channelsKey = baseStation
    ? `/api/channels?baseStation=${encodeURIComponent(baseStation)}`
    : "/api/channels";
  const { data, error: errorText, errorBody, loading } = useLive<ChannelListResponse>(channelsKey);
  const error = errorBody ?? errorText;
  const liveMobiles = useLive<MobileInfo[]>("/api/mobiles").data;
  const subscriberNames = useMemo(() => {
    const names = new Map<string, string>();
    for (const m of Array.isArray(liveMobiles) ? liveMobiles : []) {
      if (m.subscriberDisplayName) names.set(m.address, m.subscriberDisplayName);
    }
    return names;
  }, [liveMobiles]);


  // A row the BSC could not attribute to a cell stays visible under whichever
  // cell is selected.
  const cell = activeToken;
  const channels = (data?.channels ?? []).filter(
    (c) => cell == null || !c.cell || cellToken(c.cell) === cell,
  );
  const overhead = channels.filter((c) => c.channelType !== "traffic");
  const forwardOverhead = overhead.filter((c) => c.direction === "forward");
  const calls = mergeCalls(channels);
  const walshPool =
    data?.cellWalshCapacity?.find((entry) => cellToken(entry.cell) === cell)
      ?.totalWalshCodes ?? data?.totalWalshCodes ?? 0;
  const freeWalsh = Math.max(0, walshPool - forwardOverhead.length - calls.length);
  const framesTotal = calls.reduce((sum, c) => sum + (c.power?.framesTotal ?? 0), 0);
  const framesBad = calls.reduce((sum, c) => sum + (c.power?.framesCrcError ?? 0), 0);
  const overheadPower = forwardOverhead.reduce((sum, c) => sum + (c.powerFraction ?? 0), 0);
  const trafficPower = calls.reduce((sum, c) => sum + (c.powerFraction ?? 0), 0);
  const powerPct = (fraction: number) => `${(fraction * 100).toFixed(1)}%`;

  return (
    <div className="page-container space-y-4">
      <div className="flex flex-wrap items-center gap-4">
        <h1 className="text-lg font-bold">Channels</h1>
        {activeCell?.cell && (
          <span className="text-sm text-muted font-mono">
            {cellTitle(activeCell.cell)}
            {new Set(cells.map((c) => c.baseStation)).size > 1 &&
              ` · ${shortBaseStation(activeCell.baseStation)}`}
          </span>
        )}
        <CellSelector selected={cell} />
      </div>

      {loading ? (
        <p className="text-dimmed text-sm">Loading...</p>
      ) : error ? (
        <ErrorNotice error={error} />
      ) : (
        <>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
            <SummaryTile
              label="Traffic channels"
              value={String(calls.length)}
              unit="active"
              detail={`${freeWalsh} of ${walshPool} Walsh codes free · ${forwardOverhead.length} overhead`}
            />
            <SummaryTile
              label="Reverse FER, all calls"
              value={framesTotal > 0 ? `${((100 * framesBad) / framesTotal).toFixed(2)}%` : "-"}
              unit="lifetime"
              valueClass={framesTotal > 0 ? ferColor((100 * framesBad) / framesTotal) : undefined}
              detail={`${framesBad.toLocaleString()} bad of ${framesTotal.toLocaleString()} frames`}
            />
            <SummaryTile
              label="Forward power"
              value={`${((overheadPower + trafficPower) * 100).toFixed(0)}%`}
              unit="in use"
              detail={[
                ...forwardOverhead.map((c) => `${c.channelType} ${powerPct(c.powerFraction ?? 0)}`),
                `traffic ${powerPct(trafficPower)}`,
              ].join(" · ")}
            />
          </div>


          <TrafficChannelsCard
            channels={channels}
            channelsKey={channelsKey}
            baseStation={baseStation}
            subscriberNames={subscriberNames}
          />

          <Card title="Overhead">
            {overhead.length === 0 ? (
              <p className="text-dimmed text-sm">No overhead channels.</p>
            ) : (
              <div className="flex flex-wrap gap-2.5">
                {overhead.map((ch, i) => (
                  <div key={i} className="min-w-44 rounded-lg border border-border px-3 py-2">
                    <div className="flex items-center gap-2">
                      <span className={`text-xs px-2 py-0.5 rounded ${typeColor(ch.channelType)}`}>{ch.channelType}</span>
                      <span className="font-mono text-xs text-primary">{overheadName(ch)}</span>
                      <span className={`font-mono text-[10px] ${ch.direction === "forward" ? "text-accent-blue" : "text-accent-green"}`}>
                        {ch.direction === "forward" ? "FWD" : "REV"}
                      </span>
                    </div>
                    <div className="mt-1 text-xs text-muted">{overheadDetail(ch) || "-"}</div>
                  </div>
                ))}
                {calls.length > 0 && (
                  <div className="min-w-44 rounded-lg border border-border px-3 py-2">
                    <div className="flex items-center gap-2">
                      <span className={`text-xs px-2 py-0.5 rounded ${typeColor("traffic")}`}>traffic</span>
                      <span className="font-mono text-xs text-primary">
                        {calls.length} {calls.length === 1 ? "channel" : "channels"}
                      </span>
                      <span className="font-mono text-[10px] text-accent-blue">FWD</span>
                    </div>
                    <div className="mt-1 text-xs text-muted">
                      {powerPct(trafficPower)}
                      {calls.some((c) => c.powerFraction != null) &&
                        ` · ${calls
                          .filter((c) => c.powerFraction != null)
                          .map((c) => `W${c.walshCode} ${powerPct(c.powerFraction ?? 0)}`)
                          .join(" · ")}`}
                    </div>
                  </div>
                )}
              </div>
            )}
          </Card>
        </>
      )}
    </div>
  );
}
