"use client";

import { useState } from "react";
import { Card, Stat } from "@/components/card";
import { SignalQuality, type AccessSignal } from "@/lib/proto/bsc/v1/service";

const TIERS: Record<number, { bars: number; label: string; bar: string; text: string }> = {
  [SignalQuality.SIGNAL_QUALITY_POOR]: { bars: 1, label: "Poor", bar: "bg-accent-red", text: "text-accent-red" },
  [SignalQuality.SIGNAL_QUALITY_AVERAGE]: { bars: 2, label: "Average", bar: "bg-accent-amber", text: "text-accent-amber" },
  [SignalQuality.SIGNAL_QUALITY_GOOD]: { bars: 3, label: "Good", bar: "bg-accent-lime", text: "text-accent-lime" },
  [SignalQuality.SIGNAL_QUALITY_EXCELLENT]: { bars: 4, label: "Excellent", bar: "bg-accent-green", text: "text-accent-green" },
};

const BAR_HEIGHTS = { sm: [5, 8, 11, 14], lg: [9, 16, 23, 30] };

function tier(signal?: AccessSignal) {
  return signal ? TIERS[signal.quality] : undefined;
}

function formatDb(value: number | undefined, unit: string): string {
  return value != null ? `${value.toFixed(1)} ${unit}` : "-";
}

function formatMessage(name: string): string {
  return name.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/ Message$/, "");
}

function formatAge(measuredAtMs: number): string {
  const seconds = Math.max(0, Math.round((Date.now() - measuredAtMs) / 1000));
  if (seconds < 60) return `${seconds} s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  return `${Math.round(minutes / 60)} h ago`;
}

const TIER_BY_BARS = Object.fromEntries(Object.values(TIERS).map((t) => [t.bars, t]));

export function barsTier(bars?: number) {
  return bars != null ? TIER_BY_BARS[bars] : undefined;
}

export function Bars({ bars, size = "sm", label }: { bars?: number; size?: "sm" | "lg"; label?: string }) {
  const current = barsTier(bars);
  const heights = BAR_HEIGHTS[size];
  return (
    <span
      className={`inline-flex items-end ${size === "lg" ? "gap-1" : "gap-0.5"}`}
      style={{ height: heights[3] }}
      aria-label={label ?? (current ? `${current.label} signal` : "No signal measurement")}
    >
      {heights.map((height, i) => (
        <span
          key={i}
          className={`block rounded-[1px] ${size === "lg" ? "w-2" : "w-1"} ${
            current && i < current.bars ? current.bar : "bg-signal-bar-off"
          }`}
          style={{ height }}
        />
      ))}
    </span>
  );
}

export function accessSignalBars(signal?: AccessSignal): number | undefined {
  return tier(signal)?.bars;
}

export function SignalBars({ signal, size = "sm" }: { signal?: AccessSignal; size?: "sm" | "lg" }) {
  return <Bars bars={tier(signal)?.bars} size={size} />;
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between gap-4 py-0.5">
      <span className="text-muted">{label}</span>
      <span className="text-secondary font-mono">{value}</span>
    </div>
  );
}

const POPOVER_GAP_PX = 6;

// The popover is fixed-positioned so the table's scroll container cannot clip it.
export function SignalCell({ signal }: { signal?: AccessSignal }) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  if (!signal) {
    return (
      <span className="inline-flex items-center gap-2" title="No access probe measured yet">
        <SignalBars />
        <span className="text-dimmed text-xs">-</span>
      </span>
    );
  }
  const current = tier(signal);
  return (
    <span
      tabIndex={0}
      className="inline-flex items-center gap-2 outline-none cursor-default"
      onMouseEnter={(e) => setAnchor(e.currentTarget.getBoundingClientRect())}
      onMouseLeave={() => setAnchor(null)}
      onFocus={(e) => setAnchor(e.currentTarget.getBoundingClientRect())}
      onBlur={() => setAnchor(null)}
    >
      <SignalBars signal={signal} />
      <span className="font-mono text-xs text-secondary">
        {signal.ebNtDb.toFixed(1)} <span className="text-muted text-[11px]">dB</span>
      </span>
      {anchor && (
      <span
        className="fixed z-50 w-64 rounded-lg border border-border bg-popover p-3 text-xs shadow-lg"
        style={{ left: anchor.left, top: anchor.bottom + POPOVER_GAP_PX }}
      >
        <span className={`block font-semibold mb-1 ${current?.text ?? ""}`}>
          {current?.label ?? "Unknown"} · Eb/Nt {signal.ebNtDb.toFixed(1)} dB
        </span>
        <DetailRow label="Message" value={formatMessage(signal.message)} />
        <DetailRow label="Mobile RX power" value={formatPower(signal.signalPowerDbfs, powerUnits(signal))} />
        <DetailRow label="Band RX level" value={formatPower(signal.bandLevelDbfs, powerUnits(signal))} />
        <DetailRow
          label="Raw BER"
          value={signal.rawBerPct != null ? `${signal.rawBerPct.toFixed(1)}%` : "-"}
        />
        <DetailRow label="Measured" value={formatAge(signal.measuredAtMs)} />
      </span>
      )}
    </span>
  );
}

const SPARK_WIDTH = 300;
const SPARK_HEIGHT = 56;
const SPARK_MAX_DB = 30;

function sparkY(db: number): number {
  const clamped = Math.min(SPARK_MAX_DB, Math.max(0, db));
  return SPARK_HEIGHT - (clamped / SPARK_MAX_DB) * SPARK_HEIGHT;
}

function Sparkline({ values }: { values: number[] }) {
  if (values.length < 2) return null;
  const step = SPARK_WIDTH / (values.length - 1);
  const points = values.map((v, i) => `${(i * step).toFixed(1)},${sparkY(v).toFixed(1)}`).join(" ");
  const last = values[values.length - 1];
  return (
    <div className="mt-3">
      <svg
        className="block w-full"
        style={{ height: SPARK_HEIGHT }}
        viewBox={`0 0 ${SPARK_WIDTH} ${SPARK_HEIGHT}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={`Eb/Nt of the last ${values.length} access probes`}
      >
        <rect x={0} y={0} width={SPARK_WIDTH} height={SPARK_HEIGHT} fill="var(--bg-surface-raised)" />
        <polyline fill="none" stroke="var(--accent-cyan)" strokeWidth={1.6} points={points} />
        <circle cx={SPARK_WIDTH} cy={sparkY(last)} r={2.6} fill="var(--accent-cyan)" />
      </svg>
      <div className="text-[11px] text-muted mt-1">
        Eb/Nt of the last {values.length} access probes, 0 to {SPARK_MAX_DB} dB.
      </div>
    </div>
  );
}

export function SignalQualityCard({
  signal,
  recentEbNtDb,
  children,
}: {
  signal?: AccessSignal;
  recentEbNtDb?: number[];
  children?: React.ReactNode;
}) {
  const current = tier(signal);
  return (
    <Card title="Signal Quality">
      {!signal ? (
        <div className="flex items-center gap-3">
          <SignalBars size="lg" />
          <span className="text-dimmed text-sm">No access probe measured yet.</span>
        </div>
      ) : null}
      {signal && (
        <>
          <div className="flex items-center gap-3.5 pb-3 mb-2 border-b border-border">
            <SignalBars signal={signal} size="lg" />
            <div>
              <div className="font-mono text-2xl font-semibold text-primary">
                {signal.ebNtDb.toFixed(1)} <span className="text-sm text-muted font-normal">dB Eb/Nt</span>
              </div>
              <div className={`text-xs ${current?.text ?? "text-muted"}`}>{current?.label ?? "Unknown"}</div>
            </div>
          </div>
          <Stat label="Message" value={formatMessage(signal.message)} />
          <Stat
            label="Mobile RX power"
            value={formatPower(signal.signalPowerDbfs, powerUnits(signal))}
            title={dbfsTitle(signal.signalPowerDbfs, powerUnits(signal))}
            mono
          />
          <Stat
            label="Band RX level"
            value={formatPower(signal.bandLevelDbfs, powerUnits(signal))}
            title={dbfsTitle(signal.bandLevelDbfs, powerUnits(signal))}
            mono
          />
          <Stat
            label="Raw BER"
            value={signal.rawBerPct != null ? `${signal.rawBerPct.toFixed(1)}%` : "-"}
            mono
          />
          <Stat label="Measured" value={new Date(signal.measuredAtMs).toLocaleTimeString()} mono />
          {children}
          <Sparkline values={recentEbNtDb ?? []} />
        </>
      )}
      {!signal && children}
    </Card>
  );
}

const POWER_SCALE_LO_DBFS = -100;
const POWER_SCALE_HI_DBFS = 0;
/** Power this close to full scale risks compressing the receiver. */
const HOT_ZONE_DB = 12;
const POWER_TICK_STEP_DB = 20;

function powerPct(dbfs: number): number {
  const pct = ((dbfs - POWER_SCALE_LO_DBFS) / (POWER_SCALE_HI_DBFS - POWER_SCALE_LO_DBFS)) * 100;
  return Math.min(100, Math.max(0, pct));
}

const HOT_ZONE_PCT = 100 - powerPct(POWER_SCALE_HI_DBFS - HOT_ZONE_DB);

interface PowerUnits {
  unit: "dBm" | "dBFS";
  offset: number;
}

function powerUnits(signal: AccessSignal): PowerUnits {
  if (signal.signalPowerDbm != null && signal.signalPowerDbfs != null) {
    return { unit: "dBm", offset: signal.signalPowerDbm - signal.signalPowerDbfs };
  }
  return { unit: "dBFS", offset: 0 };
}

function formatPower(dbfs: number | undefined, units: PowerUnits): string {
  return dbfs != null ? formatDb(dbfs + units.offset, units.unit) : "-";
}

function dbfsTitle(dbfs: number | undefined, units: PowerUnits): string | undefined {
  return dbfs != null && units.unit === "dBm" ? formatDb(dbfs, "dBFS") : undefined;
}

function noiseFloorLabel(signal: AccessSignal, units: PowerUnits): string {
  if (signal.noiseFloorDbfs == null) return "-";
  const value = formatPower(signal.noiseFloorDbfs, units);
  return signal.noiseFloorStale ? `${value} (stale)` : value;
}

export function RxPowerCell({ signal }: { signal?: AccessSignal }) {
  const power = signal?.signalPowerDbfs;
  const units = signal ? powerUnits(signal) : { unit: "dBFS" as const, offset: 0 };
  const floor = signal?.noiseFloorDbfs;
  const hot = power != null && power >= POWER_SCALE_HI_DBFS - HOT_ZONE_DB;
  const title = signal
    ? `Mobile ${formatPower(power, units)}${units.unit === "dBm" && power != null ? ` (${power.toFixed(1)} dBFS)` : ""} · band ${formatPower(signal.bandLevelDbfs, units)} · noise floor ${noiseFloorLabel(signal, units)}`
    : "No access probe measured yet";
  return (
    <span className="inline-flex items-center gap-2" title={title}>
      <span className="relative block w-[72px] h-2 rounded-[2px] bg-signal-bar-off overflow-hidden">
        {power != null && (
          <span
            className={`absolute inset-y-0 left-0 ${hot ? "bg-accent-amber" : "bg-accent-cyan"}`}
            style={{ width: `${powerPct(power)}%` }}
          />
        )}
        <span className="absolute inset-y-0 right-0 bg-accent-red opacity-20" style={{ width: `${HOT_ZONE_PCT}%` }} />
        {floor != null && (
          <span
            className={`absolute -inset-y-px w-[1.5px] bg-secondary ${signal?.noiseFloorStale ? "opacity-25" : "opacity-50"}`}
            style={{ left: `${powerPct(floor)}%` }}
          />
        )}
      </span>
      <span className="font-mono text-xs text-secondary min-w-[78px] text-right">
        {power != null ? (
          <>
            {(power + units.offset).toFixed(1)} <span className="text-muted text-[11px]">{units.unit}</span>
          </>
        ) : (
          <span className="text-dimmed">-</span>
        )}
      </span>
    </span>
  );
}

function PowerScale({ signal, units }: { signal: AccessSignal; units: PowerUnits }) {
  const ticks: number[] = [];
  for (let v = POWER_SCALE_LO_DBFS; v <= POWER_SCALE_HI_DBFS; v += POWER_TICK_STEP_DB) ticks.push(v);
  const floor = signal.noiseFloorDbfs;
  return (
    <div className="relative mt-4 mb-7">
      <div className="relative h-3.5 rounded bg-signal-bar-off">
        <span
          className="absolute inset-y-0 right-0 rounded-r bg-accent-red opacity-15"
          style={{ width: `${HOT_ZONE_PCT}%` }}
        />
        {floor != null && (
          <span
            className={`absolute -inset-y-1.5 w-0.5 bg-secondary ${signal.noiseFloorStale ? "opacity-25" : "opacity-50"}`}
            style={{ left: `${powerPct(floor)}%` }}
          />
        )}
        {signal.bandLevelDbfs != null && (
          <span
            className="absolute -inset-y-1 w-0.5 bg-accent-purple"
            style={{ left: `${powerPct(signal.bandLevelDbfs)}%` }}
          />
        )}
        {signal.signalPowerDbfs != null && (
          <span
            className="absolute top-1/2 w-4 h-4 rounded-full bg-accent-cyan border-2 border-[var(--bg-body)] -translate-x-1/2 -translate-y-1/2"
            style={{ left: `${powerPct(signal.signalPowerDbfs)}%` }}
          />
        )}
      </div>
      {ticks.map((v) => (
        <span
          key={v}
          className="absolute top-5 -translate-x-1/2 font-mono text-[10px] text-muted"
          style={{ left: `${powerPct(v)}%` }}
        >
          {Math.round(v + units.offset)}
        </span>
      ))}
    </div>
  );
}

function LegendItem({ swatch, label }: { swatch: string; label: string }) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <i className={`inline-block ${swatch}`} />
      {label}
    </span>
  );
}

export function RxPowerCard({ signal }: { signal?: AccessSignal }) {
  if (!signal || signal.signalPowerDbfs == null) {
    return (
      <Card title="RX Power">
        <span className="text-dimmed text-sm">No access probe measured yet.</span>
      </Card>
    );
  }
  const units = powerUnits(signal);
  const aboveFloor =
    signal.bandLevelDbfs != null && signal.noiseFloorDbfs != null
      ? `${(signal.bandLevelDbfs - signal.noiseFloorDbfs).toFixed(1)} dB`
      : "-";
  return (
    <Card title="RX Power">
      <div className="flex items-baseline gap-2.5">
        <span
          className="font-mono text-2xl font-semibold text-primary"
          title={dbfsTitle(signal.signalPowerDbfs, units)}
        >
          {(signal.signalPowerDbfs + units.offset).toFixed(1)}{" "}
          <span className="text-sm text-muted font-normal">{units.unit}</span>
        </span>
        <span className="text-xs text-muted">
          {formatMessage(signal.message)} · {formatAge(signal.measuredAtMs)}
        </span>
      </div>
      <PowerScale signal={signal} units={units} />
      <div className="flex flex-wrap gap-3.5 text-[11px] text-muted mb-3">
        <LegendItem swatch="w-2.5 h-2.5 rounded-full bg-accent-cyan" label="This mobile" />
        <LegendItem swatch="w-0.5 h-3 bg-accent-purple" label="Band level" />
        <LegendItem swatch="w-0.5 h-3 bg-secondary opacity-50" label="Noise floor" />
        <LegendItem swatch="w-2.5 h-2.5 rounded-sm bg-accent-red opacity-40" label="Hot zone" />
      </div>
      <Stat
        label="Mobile RX power"
        value={formatPower(signal.signalPowerDbfs, units)}
        title={dbfsTitle(signal.signalPowerDbfs, units)}
        mono
      />
      <Stat
        label="Band RX level"
        value={formatPower(signal.bandLevelDbfs, units)}
        title={dbfsTitle(signal.bandLevelDbfs, units)}
        mono
      />
      <Stat
        label="Noise floor"
        value={noiseFloorLabel(signal, units)}
        title={dbfsTitle(signal.noiseFloorDbfs, units)}
        mono
      />
      <Stat label="Band above noise floor" value={aboveFloor} mono />
    </Card>
  );
}
