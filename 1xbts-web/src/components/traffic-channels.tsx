"use client";

import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import { RxPower } from "@/components/rx-power";
import { Bars } from "@/components/signal-quality";
import { TimeSeriesChart } from "@/components/time-series-chart";
import { VoicePrivacyIndicator } from "@/components/voice-privacy-indicator";
import { cellLabel, cellToken } from "@/lib/cell";
import { type TrafficCall, ferColor, lifetimeFerPct, mergeCalls } from "@/lib/traffic-calls";
import type { CellId } from "@/lib/proto/bsc/v1/service";
import { CALL_KIND_BADGE, CALL_KIND_LABEL, callKind, callStateLabel } from "@/lib/call-kind";
import { radioConfigPairName } from "@/lib/radio-config";
import { measuredReverseDb, recentMedianDb, reverseMetricLabel, reverseQualityBars } from "@/lib/reverse-quality";
import { serviceOptionName } from "@/lib/service-option";
import { liveSocket } from "@/lib/live";

export interface ChannelMobile {
  address: string;
  state: string;
  phoneNumber?: string;
  snrDb?: number;
  rxPowerDbm?: number;
  rxLevelDbfs?: number;
  signalPowerDb?: number;
  demodQualityPct?: number;
  voiceCallState?: string;
  voicePrivacy?: string;
}

export interface PowerSample {
  timestampMs: number;
  measuredEbNtDb: number;
  targetEbNtDb: number;
  forwardGainDb: number;
  ferPct: number;
}

export interface TrafficChannelPower {
  targetEbNtDb: number;
  effectiveTargetEbNtDb: number;
  manualTargetOverrideDb?: number;
  reversePilotEcIoDb?: number;
  ferPct: number;
  framesTotal: number;
  framesCrcError: number;
  forwardGainOffsetDb: number;
  forwardLastFerPct: number;
  forwardLastPmrmErrors: number;
  forwardLastPmrmFrames: number;
  forwardPmrmCount: number;
  forwardPilotEcIoDb?: number[];
  forwardRadioConfig: number;
  reverseRadioConfig: number;
  measuredInnerLoop1sDb?: number;
  reversePowerDbfs?: number;
  reversePowerDbm?: number;
  lastPcgPilotEcNtDb?: number[];
  lastPcgSnrDb?: number[];
  lastActivePcgMask?: boolean[];
  powerHistory?: PowerSample[];
}

export interface Channel {
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

export type Call = TrafficCall<ChannelMobile, TrafficChannelPower>;

const SPARK_SAMPLES = 60;
const HISTORY_SAMPLES = 120;
const UNDER_TARGET_DB = 1;

function callStateBadge(state?: string): string {
  switch (callStateLabel(state)) {
    case "Connected": return "bg-badge-green-bg text-badge-green-text";
    case "Alerting": return "bg-badge-yellow-bg text-badge-yellow-text";
    case "Releasing": return "bg-badge-orange-bg text-badge-orange-text";
    default: return "bg-badge-blue-bg text-badge-blue-text";
  }
}

function signedDb(value: number): string {
  return `${value >= 0 ? "+" : ""}${value.toFixed(1)} dB`;
}

function Sparkline({ samples }: { samples: PowerSample[] }) {
  const recent = samples.slice(-SPARK_SAMPLES).filter((s) => Number.isFinite(s.measuredEbNtDb));
  if (recent.length < 2) return <span className="inline-block w-[70px]" />;
  const values = recent.flatMap((s) => [s.measuredEbNtDb, s.targetEbNtDb]);
  const lo = Math.min(...values) - 0.5;
  const hi = Math.max(...values) + 0.5;
  const path = (pick: (s: PowerSample) => number) =>
    recent
      .map((s, i) => `${i ? "L" : "M"}${((i / (recent.length - 1)) * 70).toFixed(1)},${(20 - ((pick(s) - lo) / (hi - lo)) * 20).toFixed(1)}`)
      .join(" ");
  return (
    <svg viewBox="0 0 70 20" preserveAspectRatio="none" className="inline-block h-5 w-[70px]" aria-hidden="true">
      <path d={path((s) => s.targetEbNtDb)} fill="none" stroke="var(--text-muted)" strokeOpacity={0.6} strokeWidth={1} />
      <path d={path((s) => s.measuredEbNtDb)} fill="none" stroke="var(--accent-green)" strokeWidth={1.4} />
    </svg>
  );
}

export function TrafficChannelsCard({
  channels,
  channelsKey,
  baseStation,
  subscriberNames,
  showCell,
  unavailable,
  className,
  bodyClassName,
  action,
  footer,
}: {
  channels: Channel[];
  channelsKey: string;
  baseStation: string;
  subscriberNames?: Map<string, string>;
  showCell?: boolean;
  unavailable?: boolean;
  className?: string;
  bodyClassName?: string;
  action?: ReactNode;
  footer?: ReactNode;
}) {
  const calls = useMemo(() => mergeCalls(channels), [channels]);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const [powerDrafts, setPowerDrafts] = useState<Record<string, string>>({});
  const [mutatingKey, setMutatingKey] = useState<string | null>(null);
  const [openCalls, setOpenCalls] = useState<Set<string>>(new Set());
  // The channel list reports no reverse history for RC1 and RC2.
  const historyRef = useRef<Map<string, PowerSample[]>>(new Map());

  useEffect(() => {
    const now = Date.now();
    const live = new Set<string>();
    for (const call of calls) {
      live.add(call.key);
      const measured = measuredReverseDb(call.power);
      if (!call.power || measured == null) continue;
      const samples = historyRef.current.get(call.key) ?? [];
      samples.push({
        timestampMs: now,
        measuredEbNtDb: measured,
        targetEbNtDb: call.power.effectiveTargetEbNtDb,
        forwardGainDb: call.power.forwardGainOffsetDb,
        ferPct: call.power.ferPct,
      });
      historyRef.current.set(call.key, samples.slice(-HISTORY_SAMPLES));
    }
    for (const key of historyRef.current.keys()) {
      if (!live.has(key)) historyRef.current.delete(key);
    }
  }, [calls]);

  // A channel is addressed by (cell, walsh): every cell hands out the same
  // codes from its own pool. A row without a cell goes out unaddressed, which
  // the BSC resolves only while one cell is enrolled.
  const applyPowerOverride = useCallback(
    async (call: Call, payload: { targetDb?: number; clear?: boolean }) => {
      setMutatingKey(call.key);
      setMutationError(null);
      try {
        const res = await fetch("/api/channels/power-override", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            walshCode: call.walshCode,
            ...(cellToken(call.cell) ? { cell: cellToken(call.cell) } : {}),
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
            next[call.key] = json.trafficPower.effectiveTargetEbNtDb.toFixed(1);
          } else if (payload.clear) {
            delete next[call.key];
          }
          return next;
        });
        liveSocket().refresh(channelsKey);
      } catch (err) {
        setMutationError(err instanceof Error ? err.message : "unknown");
      } finally {
        setMutatingKey((current) => (current === call.key ? null : current));
      }
    },
    [channelsKey, baseStation]
  );

  const handlePin = useCallback(
    async (call: Call) => {
      if (!call.power) return;
      const raw = powerDrafts[call.key] ?? call.power.effectiveTargetEbNtDb.toFixed(1);
      const targetDb = Number(raw.trim());
      if (!Number.isFinite(targetDb)) {
        setMutationError(`invalid target for W${call.walshCode}`);
        return;
      }
      await applyPowerOverride(call, { targetDb });
    },
    [applyPowerOverride, powerDrafts]
  );

  const toggleCall = useCallback((key: string) => {
    setOpenCalls((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  return (
    <Card title="Traffic Channels" className={className} action={action ?? (calls.length > 0 ? <span className="text-[11px] text-accent-green">● {calls.length} active</span> : undefined)}>
      {mutationError && <p className="mb-2 text-accent-red text-sm">{mutationError}</p>}
      <div className={bodyClassName}>
        {unavailable ? (
          <p className="text-dimmed text-sm">Unavailable</p>
        ) : calls.length === 0 ? (
          <p className="text-dimmed text-sm">No active traffic channels.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-muted text-xs">
                  <th className="text-left py-1 pr-3">Walsh</th>
                  {showCell && <th className="text-left py-1 pr-3">Cell</th>}
                  <th className="text-left py-1 pr-3">Mobile</th>
                  <th className="text-left py-1 pr-3">Call</th>
                  <th className="text-left py-1 pr-3">Reverse quality</th>
                  <th className="text-right py-1 pr-3">Rev FER</th>
                  <th className="text-right py-1 pr-3">Lifetime</th>
                  <th className="text-right py-1 pr-3">Fwd FER</th>
                  <th className="text-right py-1 pr-3">Fwd gain</th>
                  <th className="text-right py-1 pr-3">Rx</th>
                  <th className="text-left py-1">Signal</th>
                </tr>
              </thead>
              <tbody>
                {calls.map((call) => (
                  <CallRows
                    key={call.key}
                    call={call}
                    showCell={showCell}
                    subscriberName={call.mobile ? subscriberNames?.get(call.mobile.address) : undefined}
                    history={
                      call.power?.powerHistory?.length
                        ? call.power.powerHistory
                        : (historyRef.current.get(call.key) ?? [])
                    }
                    open={openCalls.has(call.key)}
                    onToggle={() => toggleCall(call.key)}
                    draft={powerDrafts[call.key]}
                    mutating={mutatingKey === call.key}
                    onDraftChange={(value) => setPowerDrafts((prev) => ({ ...prev, [call.key]: value }))}
                    onPin={() => handlePin(call)}
                    onClear={() => applyPowerOverride(call, { clear: true })}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
      {footer}
    </Card>
  );
}

function CallRows({
  call,
  showCell,
  subscriberName,
  history,
  open,
  onToggle,
  draft,
  mutating,
  onDraftChange,
  onPin,
  onClear,
}: {
  call: Call;
  showCell?: boolean;
  subscriberName?: string;
  history: PowerSample[];
  open: boolean;
  onToggle: () => void;
  draft?: string;
  mutating: boolean;
  onDraftChange: (value: string) => void;
  onPin: () => void;
  onClear: () => void;
}) {
  const power = call.power;
  const kind = callKind(call.serviceOption);
  const rc = power?.reverseRadioConfig;
  const metric = reverseMetricLabel(rc);
  const measured = recentMedianDb(history.map((s) => s.measuredEbNtDb)) ?? measuredReverseDb(power);
  const target = power?.effectiveTargetEbNtDb;
  const lifetime = lifetimeFerPct(power);
  const bars = reverseQualityBars(rc, measured);
  const radioConfigs = radioConfigPairName(power?.forwardRadioConfig, rc);
  const isRc3 = rc === 3;
  const draftValue = draft ?? target?.toFixed(1) ?? "";

  return (
    <Fragment>
      <tr className="border-t border-border hover:bg-hover cursor-pointer" onClick={onToggle}>
        <td className="py-2 pr-3 font-mono text-xs text-primary whitespace-nowrap">
          <span className="inline-block w-3 text-dimmed">{open ? "▾" : "▸"}</span> W{call.walshCode}
        </td>
        {showCell && (
          <td className="py-2 pr-3 font-mono text-xs text-muted whitespace-nowrap">
            {call.cell ? cellLabel(call.cell) : "-"}
          </td>
        )}
        <td className="py-2 pr-3 text-xs">
          {call.mobile ? (
            <Link
              href={`/mobiles/${encodeURIComponent(call.mobile.address)}`}
              onClick={(e) => e.stopPropagation()}
              className="text-accent-green hover:text-accent-green transition-colors"
            >
              {subscriberName || call.mobile.phoneNumber || call.mobile.address}
            </Link>
          ) : (
            <span className="text-dimmed">-</span>
          )}
          {subscriberName && call.mobile?.phoneNumber && (
            <span className="ml-1.5 font-mono text-[11px] text-muted">{call.mobile.phoneNumber}</span>
          )}
          <div className="text-[11px] text-muted whitespace-nowrap">
            {[radioConfigs, call.serviceOption != null ? serviceOptionName(call.serviceOption) : null]
              .filter(Boolean)
              .join(" · ")}
          </div>
        </td>
        <td className="py-2 pr-3 text-xs">
          <span className="inline-flex items-center gap-1.5 whitespace-nowrap">
            <span className={`px-1.5 py-0.5 rounded ${CALL_KIND_BADGE[kind]}`}>{CALL_KIND_LABEL[kind]}</span>
            {call.mobile?.voiceCallState && (
              <span className={`px-1.5 py-0.5 rounded ${callStateBadge(call.mobile.voiceCallState)}`}>
                {callStateLabel(call.mobile.voiceCallState)}
              </span>
            )}
            <VoicePrivacyIndicator status={call.mobile?.voicePrivacy} />
          </span>
        </td>
        <td className="py-2 pr-3 text-xs">
          {power ? (
            <span className="inline-flex items-center gap-2">
              <Sparkline samples={history} />
              <span className="leading-tight">
                <span
                  className={`block font-mono ${
                    measured != null && target != null && measured < target - UNDER_TARGET_DB
                      ? "text-accent-red"
                      : "text-primary"
                  }`}
                >
                  {measured != null ? `${measured.toFixed(1)} dB` : "-"}
                </span>
                <span className="block text-[11px] text-muted whitespace-nowrap">
                  {metric} · target {target?.toFixed(1) ?? "-"}
                  {power.manualTargetOverrideDb != null && " pinned"}
                </span>
              </span>
            </span>
          ) : (
            <span className="text-dimmed">-</span>
          )}
        </td>
        <td className={`py-2 pr-3 text-right font-mono text-xs ${power ? ferColor(power.ferPct) : "text-dimmed"}`}>
          {power ? `${power.ferPct.toFixed(1)}%` : "-"}
        </td>
        <td className="py-2 pr-3 text-right text-xs leading-tight">
          {lifetime != null && power ? (
            <>
              <span className={`block font-mono ${ferColor(lifetime)}`}>{lifetime.toFixed(2)}%</span>
              <span className="block text-[11px] text-muted font-mono">
                {power.framesCrcError}/{power.framesTotal.toLocaleString()}
              </span>
            </>
          ) : (
            <span className="text-dimmed">-</span>
          )}
        </td>
        <td className="py-2 pr-3 text-right font-mono text-xs">
          {power && power.forwardPmrmCount > 0 ? (
            <span className={ferColor(power.forwardLastFerPct)}>{power.forwardLastFerPct.toFixed(1)}%</span>
          ) : (
            <span className="text-dimmed" title="No Power Measurement Report yet">-</span>
          )}
        </td>
        <td className="py-2 pr-3 text-right font-mono text-xs text-secondary whitespace-nowrap">
          {power ? signedDb(power.forwardGainOffsetDb) : "-"}
          {call.powerFraction != null && (
            <div className="text-[11px] text-muted">{(call.powerFraction * 100).toFixed(1)}% fwd</div>
          )}
        </td>
        <td className="py-2 pr-3 text-right font-mono text-xs text-secondary whitespace-nowrap">
          {power?.reversePowerDbfs != null || power?.reversePowerDbm != null ? (
            <RxPower dbm={power.reversePowerDbm} dbfs={power.reversePowerDbfs} />
          ) : call.mobile?.rxPowerDbm != null || call.mobile?.rxLevelDbfs != null ? (
            <RxPower dbm={call.mobile.rxPowerDbm} dbfs={call.mobile.rxLevelDbfs} />
          ) : call.mobile?.signalPowerDb != null ? (
            `${call.mobile.signalPowerDb.toFixed(1)} dB`
          ) : (
            "-"
          )}
        </td>
        <td className="py-2" title={measured != null ? `${metric}: ${measured.toFixed(1)} dB` : "No measurement yet"}>
          <Bars bars={bars} />
        </td>
      </tr>
      {open && (
        <tr>
          <td colSpan={showCell ? 11 : 10} className="pb-4 pl-5 pt-1">
            {power ? (
              <div className="grid gap-3 lg:grid-cols-3">
                <div className="rounded-lg bg-surface-raised p-3">
                  <div className="mb-1 flex justify-between text-[11px] uppercase tracking-wide text-muted">
                    <span>Reverse {metric}</span>
                    <span>{radioConfigs ?? ""}</span>
                  </div>
                  {history.length >= 2 ? (
                    <TimeSeriesChart
                      width={420}
                      height={170}
                      yLabel="dB"
                      timestamps={history.map((s) => Number(s.timestampMs))}
                      series={[
                        { key: "target", label: "Target", color: "#f59e0b", values: history.map((s) => s.targetEbNtDb) },
                        { key: "measured", label: `Measured ${metric}`, color: "#22c55e", values: history.map((s) => s.measuredEbNtDb) },
                      ]}
                    />
                  ) : (
                    <p className="text-xs text-muted py-6 text-center">Collecting history.</p>
                  )}
                  {power.reversePilotEcIoDb != null && (
                    <div className="mt-1 flex justify-between text-xs">
                      <span className="text-muted">Reverse pilot Ec/Io</span>
                      <span className="font-mono">{power.reversePilotEcIoDb.toFixed(1)} dB</span>
                    </div>
                  )}
                </div>
                <div className="rounded-lg bg-surface-raised p-3">
                  <div className="mb-1 flex justify-between text-[11px] uppercase tracking-wide text-muted">
                    <span>Reverse FER</span>
                    <span>window {power.ferPct.toFixed(1)}%</span>
                  </div>
                  {history.length >= 2 ? (
                    <TimeSeriesChart
                      width={420}
                      height={170}
                      yLabel="%"
                      yMin={0}
                      timestamps={history.map((s) => Number(s.timestampMs))}
                      series={[{ key: "fer", label: "FER", color: "#f59e0b", values: history.map((s) => s.ferPct) }]}
                    />
                  ) : (
                    <p className="text-xs text-muted py-6 text-center">Collecting history.</p>
                  )}
                  <div className="mt-1 flex justify-between text-xs">
                    <span className="text-muted">Frames / CRC errors</span>
                    <span className="font-mono">
                      {power.framesTotal.toLocaleString()} / {power.framesCrcError}
                    </span>
                  </div>
                </div>
                <div className="rounded-lg bg-surface-raised p-3 text-xs">
                  <div className="mb-1 flex justify-between text-[11px] uppercase tracking-wide text-muted">
                    <span>Forward link</span>
                    <span>{power.forwardPmrmCount} PMRMs</span>
                  </div>
                  <div className="flex justify-between py-0.5">
                    <span className="text-muted">Last PMRM</span>
                    <span className="font-mono">
                      {power.forwardPmrmCount > 0
                        ? `${power.forwardLastPmrmErrors} errors / ${power.forwardLastPmrmFrames} frames`
                        : "none yet"}
                    </span>
                  </div>
                  <div className="flex justify-between py-0.5">
                    <span className="text-muted">Gain offset</span>
                    <span className="font-mono">{signedDb(power.forwardGainOffsetDb)}</span>
                  </div>
                  <div className="flex justify-between py-0.5">
                    <span className="text-muted">Pilot Ec/Io (mobile)</span>
                    <span className="font-mono">
                      {(power.forwardPilotEcIoDb ?? []).length > 0
                        ? (power.forwardPilotEcIoDb ?? []).map((db) => `${db.toFixed(1)} dB`).join(", ")
                        : "-"}
                    </span>
                  </div>
                  <div className="mt-3 mb-1 flex items-center justify-between text-[11px] uppercase tracking-wide text-muted">
                    <span>Reverse target override</span>
                    <span
                      className={`normal-case tracking-normal px-1.5 py-0.5 rounded ${
                        power.manualTargetOverrideDb != null
                          ? "bg-badge-orange-bg text-badge-orange-text"
                          : "bg-surface text-muted"
                      }`}
                    >
                      {power.manualTargetOverrideDb != null ? "Pinned" : "Auto"}
                    </span>
                  </div>
                  <div className="flex items-center gap-2">
                    <input
                      type="number"
                      step="0.1"
                      min={isRc3 ? "-20" : "0"}
                      max={isRc3 ? "40" : "20"}
                      inputMode="decimal"
                      value={draftValue}
                      onChange={(event) => onDraftChange(event.target.value)}
                      disabled={mutating}
                      className="w-20 rounded border border-border-input bg-surface-solid px-2 py-1 text-right font-mono text-xs text-primary disabled:opacity-50"
                    />
                    <span className="text-muted">dB</span>
                    <button
                      type="button"
                      onClick={onPin}
                      disabled={mutating}
                      className="rounded border border-accent-green/30 px-2 py-1 text-[11px] text-accent-green hover:bg-accent-green-bg disabled:opacity-50"
                    >
                      Pin
                    </button>
                    <button
                      type="button"
                      onClick={onClear}
                      disabled={mutating || power.manualTargetOverrideDb == null}
                      className="rounded border border-border-input px-2 py-1 text-[11px] text-secondary hover:bg-surface disabled:opacity-50"
                    >
                      Clear
                    </button>
                  </div>
                </div>
              </div>
            ) : (
              <p className="text-xs text-muted">No power-control snapshot yet.</p>
            )}
          </td>
        </tr>
      )}
    </Fragment>
  );
}
