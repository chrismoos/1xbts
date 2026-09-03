"use client";

import { Suspense, useEffect, useState, useCallback } from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Card } from "@/components/card";
import { CellSelector } from "@/components/cell-selector";
import { cellTitle, cellToken, shortBaseStation } from "@/lib/cell";
import type { CellId } from "@/lib/proto/bsc/v1/service";
import { radioConfigPairName } from "@/lib/radio-config";
import { formatPowerFraction } from "@/lib/format";
import { serviceOptionName } from "@/lib/service-option";
import { useBtsList, usePinnedDefaultCell } from "@/lib/use-bts-list";

interface ChannelMobile {
  address: string;
  state: string;
  phoneNumber?: string;
  snrDb?: number;
  rxPowerDbm?: number;
  rxLevelDbfs?: number;
  signalPowerDb?: number;
  demodQualityPct?: number;
  voiceCallState?: string;
}

interface TrafficChannelPower {
  targetEbNtDb: number;
  effectiveTargetEbNtDb: number;
  manualTargetOverrideDb?: number;
  reversePilotEcIoDb?: number;
  forwardRadioConfig: number;
  reverseRadioConfig: number;
}

interface Channel {
  walshCode?: number;
  channelType: string;
  direction: string;
  powerFraction?: number;
  dataRateBps?: number;
  pagingChannelNumber?: number;
  accessChannelNumber?: number;
  mobile?: ChannelMobile;
  serviceOption?: number;
  trafficPower?: TrafficChannelPower;
  cell?: CellId;
}

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

function directionColor(dir: string): string {
  return dir === "forward"
    ? "text-accent-blue"
    : "text-accent-green";
}

function qualityColor(pct: number): string {
  if (pct >= 90) return "text-accent-green";
  if (pct >= 75) return "text-accent-amber";
  return "text-accent-red";
}

function channelName(ch: Channel): string {
  if (ch.walshCode != null) return `W${ch.walshCode}`;
  if (ch.accessChannelNumber != null) return `ACH ${ch.accessChannelNumber}`;
  return "-";
}

function formatDb(value?: number): string {
  return value != null ? `${value.toFixed(1)} dB` : "-";
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
  const [data, setData] = useState<ChannelListResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const [powerDrafts, setPowerDrafts] = useState<Record<number, string>>({});
  const [mutatingWalsh, setMutatingWalsh] = useState<number | null>(null);

  const load = useCallback(async () => {
    try {
      // A base station in the URL scopes the list to its cells. Otherwise show
      // every base station's channels.
      const url = baseStation
        ? `/api/channels?baseStation=${encodeURIComponent(baseStation)}`
        : "/api/channels";
      const res = await fetch(url);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const json = await res.json();
      if (json.error) throw new Error(json.error);
      setData(json);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown");
    } finally {
      setLoading(false);
    }
  }, [baseStation]);

  const updatePowerDraft = useCallback((walshCode: number, value: string) => {
    setPowerDrafts((prev) => ({ ...prev, [walshCode]: value }));
  }, []);

  // A channel is addressed by (cell, walsh): every cell hands out the same
  // codes from its own pool. A row without a cell goes out unaddressed, which
  // the BSC resolves only while one cell is enrolled.
  const applyPowerOverride = useCallback(
    async (channel: Channel, payload: { targetDb?: number; clear?: boolean }) => {
      const walshCode = channel.walshCode;
      if (walshCode == null) return;
      setMutatingWalsh(walshCode);
      setMutationError(null);
      try {
        const res = await fetch("/api/channels/power-override", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            walshCode,
            ...(cellToken(channel.cell) ? { cell: cellToken(channel.cell) } : {}),
            ...(baseStation ? { baseStation } : {}),
            ...payload,
          }),
        });
        const json = await res.json();
        if (!res.ok || !json.accepted) {
          throw new Error(json.message || `HTTP ${res.status}`);
        }

        setPowerDrafts((prev) => {
          const next = { ...prev };
          if (json.trafficPower?.effectiveTargetEbNtDb != null) {
            next[walshCode] = json.trafficPower.effectiveTargetEbNtDb.toFixed(1);
          } else if (payload.clear) {
            delete next[walshCode];
          }
          return next;
        });
        await load();
      } catch (err) {
        setMutationError(err instanceof Error ? err.message : "unknown");
      } finally {
        setMutatingWalsh((current) => (current === walshCode ? null : current));
      }
    },
    [load, baseStation]
  );

  const handlePin = useCallback(
    async (channel: Channel) => {
      if (channel.walshCode == null || !channel.trafficPower) return;
      const raw =
        powerDrafts[channel.walshCode] ??
        channel.trafficPower.effectiveTargetEbNtDb.toFixed(1);
      const targetDb = Number(raw.trim());
      if (!Number.isFinite(targetDb)) {
        setMutationError(`invalid target for W${channel.walshCode}`);
        return;
      }
      await applyPowerOverride(channel, { targetDb });
    },
    [applyPowerOverride, powerDrafts]
  );

  const handleClear = useCallback(
    async (channel: Channel) => {
      await applyPowerOverride(channel, { clear: true });
    },
    [applyPowerOverride]
  );

  useEffect(() => {
    load();
    const interval = setInterval(load, 3000);
    return () => clearInterval(interval);
  }, [load]);

  // A row the BSC could not attribute to a cell stays visible under whichever
  // cell is selected.
  const cell = activeToken;
  const channels = (data?.channels ?? []).filter(
    (c) => cell == null || !c.cell || cellToken(c.cell) === cell,
  );
  const fwdChannels = channels.filter((c) => c.direction === "forward");
  const revChannels = channels.filter((c) => c.direction === "reverse");
  const trafficCount = channels.filter((c) => c.channelType === "traffic").length;
  const overheadCount = fwdChannels.filter((c) => c.channelType !== "traffic").length;
  const walshPool =
    data?.cellWalshCapacity?.find((entry) => cellToken(entry.cell) === cell)
      ?.totalWalshCodes ?? data?.totalWalshCodes;

  return (
    <div className="max-w-7xl mx-auto space-y-4">
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
        {data && (
          <div className="flex gap-3 text-xs text-muted">
            <span>{overheadCount} overhead</span>
            <span>{trafficCount} traffic</span>
            <span>{revChannels.length} reverse</span>
            <span>{(walshPool ?? 0) - overheadCount - trafficCount} idle walsh</span>
          </div>
        )}
      </div>

      {mutationError && <p className="text-accent-red text-sm">{mutationError}</p>}

      {loading ? (
        <p className="text-dimmed text-sm">Loading...</p>
      ) : error ? (
        <p className="text-accent-red text-sm">{error}</p>
      ) : (
        <>
          <Card title="Forward Link">
            {fwdChannels.length === 0 ? (
              <p className="text-dimmed text-sm">No forward-link channels.</p>
            ) : (
              <ChannelTable
                channels={fwdChannels}
                powerDrafts={powerDrafts}
                mutatingWalsh={mutatingWalsh}
                onPowerDraftChange={updatePowerDraft}
                onPin={handlePin}
                onClear={handleClear}
              />
            )}
          </Card>

          <Card title="Reverse Link">
            {revChannels.length === 0 ? (
              <p className="text-dimmed text-sm">No reverse-link channels.</p>
            ) : (
              <ChannelTable
                channels={revChannels}
                powerDrafts={powerDrafts}
                mutatingWalsh={mutatingWalsh}
                onPowerDraftChange={updatePowerDraft}
                onPin={handlePin}
                onClear={handleClear}
              />
            )}
          </Card>

        </>
      )}
    </div>
  );
}

function ChannelTable({
  channels,
  powerDrafts,
  mutatingWalsh,
  onPowerDraftChange,
  onPin,
  onClear,
}: {
  channels: Channel[];
  powerDrafts: Record<number, string>;
  mutatingWalsh: number | null;
  onPowerDraftChange: (walshCode: number, value: string) => void;
  onPin: (channel: Channel) => void;
  onClear: (channel: Channel) => void;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-muted text-xs">
            <th className="text-left py-1">Channel</th>
            <th className="text-left py-1">Type</th>
            <th className="text-left py-1">Direction</th>
            <th className="text-left py-1">Details</th>
            <th className="text-left py-1">Mobile</th>
            <th className="text-left py-1">Power</th>
            <th className="text-left py-1">Override</th>
            <th className="text-right py-1">SNR</th>
            <th className="text-right py-1">Rx Level</th>
            <th className="text-right py-1">Quality</th>
          </tr>
        </thead>
        <tbody>
          {channels.map((ch, i) => {
            const canControlPower =
              ch.direction === "reverse" &&
              ch.channelType === "traffic" &&
              ch.walshCode != null &&
              ch.trafficPower != null;
            const isRc3 = ch.trafficPower?.reverseRadioConfig === 3;
            const reversePowerMetric = isRc3 ? "Pilot SINR" : "Eb/Nt";
            const radioConfigs = radioConfigPairName(
              ch.trafficPower?.forwardRadioConfig,
              ch.trafficPower?.reverseRadioConfig,
            );
            const draftValue =
              canControlPower && ch.walshCode != null
                ? powerDrafts[ch.walshCode] ??
                  ch.trafficPower?.effectiveTargetEbNtDb.toFixed(1) ??
                  ""
                : "";

            return (
              <tr key={i} className="border-t border-border hover:bg-hover">
                <td className="py-2 font-mono text-xs text-primary">
                  {channelName(ch)}
                </td>
                <td className="py-2">
                  <span className={`text-xs px-2 py-0.5 rounded ${typeColor(ch.channelType)}`}>
                    {ch.channelType}
                  </span>
                </td>
                <td className="py-2">
                  <span className={`text-xs font-mono ${directionColor(ch.direction)}`}>
                    {ch.direction === "forward" ? "FWD" : "REV"}
                  </span>
                </td>
                <td className="py-2 text-xs text-muted">
                  {ch.channelType === "pilot" && ch.powerFraction != null && (
                    <span>{formatPowerFraction(ch.powerFraction)}</span>
                  )}
                  {ch.channelType === "sync" && (
                    <span>
                      {ch.dataRateBps ? `${ch.dataRateBps} bps` : ""}
                      {ch.powerFraction != null && ` / ${formatPowerFraction(ch.powerFraction)}`}
                    </span>
                  )}
                  {ch.channelType === "paging" && (
                    <span>
                      PCH {ch.pagingChannelNumber}
                      {ch.dataRateBps ? ` / ${ch.dataRateBps} bps` : ""}
                      {ch.powerFraction != null && ` / ${formatPowerFraction(ch.powerFraction)}`}
                    </span>
                  )}
                  {ch.channelType === "access" && (
                    <span>
                      {ch.dataRateBps ? `${ch.dataRateBps} bps` : ""}
                    </span>
                  )}
                  {ch.channelType === "traffic" && (
                    <span>
                      {[radioConfigs, ch.serviceOption != null ? serviceOptionName(ch.serviceOption) : null]
                        .filter(Boolean)
                        .join(" · ")}
                    </span>
                  )}
                </td>
                <td className="py-2 text-xs">
                  {ch.mobile ? (
                    <span className="inline-flex items-center gap-2">
                      <Link
                        href={`/mobiles/${encodeURIComponent(ch.mobile.address)}`}
                        className="text-accent-green hover:text-accent-green transition-colors"
                      >
                        {ch.mobile.phoneNumber || ch.mobile.address}
                      </Link>
                      {ch.mobile.voiceCallState && (
                        <span className={`text-[10px] px-1.5 py-0.5 rounded ${
                          ch.mobile.voiceCallState === "Connected" ? "bg-badge-green-bg text-badge-green-text" :
                          ch.mobile.voiceCallState === "Alerting" ? "bg-badge-yellow-bg text-badge-yellow-text" :
                          ch.mobile.voiceCallState === "Releasing" ? "bg-badge-orange-bg text-badge-orange-text" :
                          "bg-badge-blue-bg text-badge-blue-text"
                        }`}>
                          {ch.mobile.voiceCallState}
                        </span>
                      )}
                    </span>
                  ) : (
                    <span className="text-dimmed">-</span>
                  )}
                </td>
                <td className="py-2 text-xs text-secondary">
                  {ch.trafficPower ? (
                    <div className="leading-5">
                      <div className="text-[10px] uppercase tracking-wide text-muted">
                        {reversePowerMetric}
                      </div>
                      <div className="font-mono text-primary">
                        eff {formatDb(ch.trafficPower.effectiveTargetEbNtDb)}
                      </div>
                      <div className="font-mono text-muted">
                        auto {formatDb(ch.trafficPower.targetEbNtDb)}
                      </div>
                      {ch.trafficPower.reversePilotEcIoDb != null ? (
                        <div className="font-mono text-muted">
                          pilot Ec/Io {formatDb(ch.trafficPower.reversePilotEcIoDb)}
                        </div>
                      ) : null}
                      <span className={`inline-flex px-1.5 py-0.5 rounded text-[10px] ${
                        ch.trafficPower.manualTargetOverrideDb != null
                          ? "bg-badge-orange-bg text-badge-orange-text"
                          : "bg-surface-raised text-muted"
                      }`}>
                        {ch.trafficPower.manualTargetOverrideDb != null ? "Pinned" : "Auto"}
                      </span>
                    </div>
                  ) : (
                    <span className="text-dimmed">-</span>
                  )}
                </td>
                <td className="py-2 text-xs text-secondary">
                  {canControlPower && ch.walshCode != null ? (
                    <div className="flex items-center gap-2">
                      <input
                        type="number"
                        step="0.1"
                        min={isRc3 ? "-20" : "0"}
                        max={isRc3 ? "40" : "20"}
                        inputMode="decimal"
                        value={draftValue}
                        onChange={(event) => onPowerDraftChange(ch.walshCode!, event.target.value)}
                        disabled={mutatingWalsh === ch.walshCode}
                        className="w-20 rounded border border-border-input bg-surface-solid px-2 py-1 text-right font-mono text-xs text-primary disabled:opacity-50"
                      />
                      <button
                        type="button"
                        onClick={() => onPin(ch)}
                        disabled={mutatingWalsh === ch.walshCode}
                        className="rounded border border-accent-green/30 px-2 py-1 text-[11px] text-accent-green hover:bg-accent-green-bg disabled:opacity-50"
                      >
                        Pin
                      </button>
                      <button
                        type="button"
                        onClick={() => onClear(ch)}
                        disabled={
                          mutatingWalsh === ch.walshCode ||
                          ch.trafficPower?.manualTargetOverrideDb == null
                        }
                        className="rounded border border-border-input px-2 py-1 text-[11px] text-secondary hover:bg-surface-raised disabled:opacity-50"
                      >
                        Clear
                      </button>
                    </div>
                  ) : (
                    <span className="text-dimmed">-</span>
                  )}
                </td>
                <td className="py-2 text-right font-mono text-xs text-secondary">
                  {ch.mobile?.snrDb != null ? ch.mobile.snrDb.toFixed(1) : "-"}
                </td>
                <td className="py-2 text-right font-mono text-xs text-secondary">
                  {ch.mobile?.rxPowerDbm != null
                    ? `${ch.mobile.rxPowerDbm.toFixed(1)} dBm`
                    : ch.mobile?.rxLevelDbfs != null
                    ? `${ch.mobile.rxLevelDbfs.toFixed(1)} dBFS`
                    : ch.mobile?.signalPowerDb != null
                    ? `${ch.mobile.signalPowerDb.toFixed(1)} dB`
                    : "-"}
                </td>
                <td className="py-2 text-right font-mono text-xs">
                  {ch.mobile?.demodQualityPct != null ? (
                    <span className={qualityColor(ch.mobile.demodQualityPct)}>
                      {ch.mobile.demodQualityPct.toFixed(0)}%
                    </span>
                  ) : "-"}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
