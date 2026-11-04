import type { EvdoLinkState, EvdoVerdict } from "@/lib/evdo";

const LINK_CHIP: Record<EvdoLinkState, { label: string; className: string }> = {
  connected: { label: "Connected", className: "text-accent-cyan bg-accent-blue-bg" },
  dormant: { label: "Dormant", className: "text-accent-amber bg-accent-amber-bg" },
  idle: { label: "Idle", className: "text-muted bg-surface-raised" },
};

const VERDICT_CHIP: Record<EvdoVerdict, { label: string; className: string }> = {
  admitted: { label: "Admitted", className: "text-accent-green bg-accent-green-bg" },
  refused: { label: "Refused", className: "text-accent-red bg-accent-red-bg" },
  unknown: { label: "Waiting for Hardware ID", className: "text-dimmed bg-surface-raised" },
};

const CHIP = "inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium whitespace-nowrap";

const SNR_FLOOR_DB = -5;
const SNR_SPAN_DB = 20;
const SNR_GOOD_DB = 7;
const SNR_FAIR_DB = 2;

export function EvdoChip({ label, className }: { label: string; className: string }) {
  return <span className={`${CHIP} ${className}`}>{label}</span>;
}

export function EvdoLinkChip({ state }: { state: EvdoLinkState }) {
  return <EvdoChip {...LINK_CHIP[state]} />;
}

export function EvdoVerdictChip({ verdict }: { verdict: EvdoVerdict }) {
  return <EvdoChip {...VERDICT_CHIP[verdict]} />;
}

export function EvdoSnrBar({ snrDb, width = 70 }: { snrDb?: number; width?: number }) {
  if (snrDb == null) return <span className="text-dimmed text-xs">—</span>;
  const pct = Math.max(4, Math.min(100, ((snrDb - SNR_FLOOR_DB) / SNR_SPAN_DB) * 100));
  const color =
    snrDb >= SNR_GOOD_DB ? "bg-accent-green" : snrDb >= SNR_FAIR_DB ? "bg-accent-amber" : "bg-accent-red";
  return (
    <div className="flex items-center gap-1.5">
      <div className="h-1.5 rounded bg-surface-raised overflow-hidden" style={{ width }}>
        <div className={`h-full ${color}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="font-mono text-[11px]">{snrDb.toFixed(1)} dB</span>
    </div>
  );
}
