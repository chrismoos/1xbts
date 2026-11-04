import Link from "next/link";
import { EvdoTxMode, type BtsConfig, type EvdoCarrierConfig, type OverheadConfig } from "@/lib/proto/bsc/v1/service";

export interface TxRadioHealth {
  underflows?: number;
  latePackets?: number;
  sequenceErrors?: number;
  burstAcks?: number;
  droppedPackets?: number;
  unknownEvents?: number;
}

export interface TxMetrics {
  rtRatio?: number;
  blocksTransmitted?: number;
  genAvgUs?: number;
  genMaxUs?: number;
  txAvgUs?: number;
  txMaxUs?: number;
  synthPilotUs?: number;
  synthSyncUs?: number;
  synthPagingUs?: number;
  synthSpreadUs?: number;
  syncFragmentsSent?: number;
  pagingFragmentsSent?: number;
  hwMarginMinUs?: number;
  hwMarginAvgUs?: number;
  hwMarginMaxUs?: number;
  lateBatches?: number;
  radioHealth?: TxRadioHealth;
  realtimeDegradedEvents?: number;
}

export interface RxStage {
  name: string;
  totalUs?: number;
  calls?: number;
  maxUs?: number;
  pctPipeline?: number;
}

export interface RxQueue {
  name: string;
  queuedSamples?: number;
  queuedAirtimeUs?: number;
  maxQueuedSamples?: number;
  maxResidencyUs?: number;
}

export interface RxMetrics {
  rtRatio?: number;
  reads?: number;
  samples?: number;
  captureUs?: number;
  pipelineUs?: number;
  totalUs?: number;
  totalMaxUs?: number;
  deficitMs?: number;
  stages?: RxStage[];
  queues?: RxQueue[];
}

export interface BearerMetrics {
  txFrames?: number;
  rxAccepted?: number;
  duplicateDrop?: number;
  lateDrop?: number;
  encodeErrors?: number;
  routeErrors?: number;
  deliveryErrors?: number;
}

export interface RadioMetrics {
  tx?: TxMetrics;
  rx?: RxMetrics;
  bearer?: BearerMetrics;
}

const RT_RATIO_WARN = 1.5;

// C.S0005 ZONE_TIMER codes, in minutes.
const ZONE_TIMER_MINUTES = [1, 2, 5, 10, 20, 30, 45, 60];
// The slot cycle is 1.28 s times 2^MAX_SLOT_CYCLE_INDEX (C.S0005 §2.6.2.1.1.3).
const SLOT_CYCLE_BASE_S = 1.28;

function mhz(hz?: number, digits = 3): string {
  return hz ? `${(hz / 1e6).toFixed(digits)} MHz` : "—";
}

function micros(us?: number): string {
  if (us == null) return "—";
  return us >= 1000 ? `${(us / 1000).toFixed(2)} ms` : `${us} µs`;
}

function count(value?: number): string {
  return value != null ? value.toLocaleString() : "—";
}

function powerDb(fraction?: number): string {
  return fraction && fraction > 0 ? `${(10 * Math.log10(fraction)).toFixed(1)} dB` : "off";
}

function ratioClass(ratio?: number): string {
  if (ratio == null) return "text-muted";
  return ratio >= RT_RATIO_WARN ? "text-accent-green" : "text-accent-amber";
}

function utcOffset(halfHours: number): string {
  const minutes = halfHours * 30;
  const abs = Math.abs(minutes);
  return `UTC${minutes < 0 ? "−" : "+"}${String(Math.floor(abs / 60)).padStart(2, "0")}:${String(abs % 60).padStart(2, "0")}`;
}

const QUEUE_ENDS: Record<string, string> = {
  "sdr-reader": "Radio reader",
  main: "Receiver",
  "hrpd-access": "EV-DO access",
  "hrpd-traffic": "EV-DO traffic",
};

// The receiver's queue names read "<from>-to-<to>". Each side becomes words.
function queueLabel(name: string): string {
  const words = (part: string) =>
    QUEUE_ENDS[part] ?? part.replace(/^1x-traffic-w(\d+)$/, "1x traffic W$1").replace(/-/g, " ");
  const [from, to] = name.split("-to-");
  return to ? `${words(from)} → ${words(to)}` : words(name);
}

// A stage name may be a full Rust type path. The last segment, split at
// capitals, is the readable part.
function stageLabel(name: string): string {
  const last = name.split("::").pop() ?? name;
  return last
    .replace(/Processor$/, "")
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/^Generic /, "")
    .replace(/^./, (c) => c.toUpperCase());
}

function Kv({ rows }: { rows: [label: React.ReactNode, value: React.ReactNode, field?: string][] }) {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
      {rows.map(([label, value, field], index) => (
        <div key={index} className="contents">
          <dt className="text-muted">
            {label}
            {field && <span className="block font-mono text-[10.5px] text-dimmed">{field}</span>}
          </dt>
          <dd className="text-right font-mono text-[13px] text-secondary">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div>
      <h4 className="mb-1.5 text-xs font-semibold text-primary">{title}</h4>
      {children}
    </div>
  );
}

function Chip({ children, tone = "muted" }: { children: React.ReactNode; tone?: "ok" | "warn" | "bad" | "info" | "evdo" | "muted" }) {
  const classes = {
    ok: "text-accent-green bg-accent-green-bg",
    warn: "text-accent-amber bg-accent-amber-bg",
    bad: "text-accent-red bg-accent-red-bg",
    info: "text-accent-blue bg-accent-blue-bg",
    evdo: "text-accent-purple bg-accent-purple-bg",
    muted: "text-muted bg-surface-raised",
  }[tone];
  return (
    <span className={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium normal-case tracking-normal whitespace-nowrap ${classes}`}>
      {children}
    </span>
  );
}

function StackBar({ parts }: { parts: { label: string; value: number; className: string }[] }) {
  const total = parts.reduce((sum, part) => sum + part.value, 0);
  return (
    <div>
      <div className="flex h-3 overflow-hidden rounded-full bg-surface-raised">
        {parts.map((part) => (
          <i key={part.label} className={`block h-full ${part.className}`} style={{ width: `${total > 0 ? (part.value / total) * 100 : 0}%` }} />
        ))}
      </div>
      <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-muted">
        {parts.map((part) => (
          <span key={part.label} className="inline-flex items-center gap-1.5">
            <i className={`inline-block h-2 w-2 rounded-sm ${part.className}`} />
            {part.label}
          </span>
        ))}
      </div>
    </div>
  );
}

// The open state belongs to the browser after the first render.
export function Fold({
  title,
  summary,
  defaultOpen = false,
  children,
}: {
  title: string;
  summary?: React.ReactNode;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  return (
    <details open={defaultOpen} className="group glass-card">
      <summary className="flex cursor-pointer list-none items-center justify-between gap-3 px-4 py-3 [&::-webkit-details-marker]:hidden">
        <span className="flex items-center gap-2 text-[11px] uppercase tracking-wider text-muted">
          <span className="inline-block transition-transform group-open:rotate-90">›</span>
          {title}
        </span>
        <span className="flex flex-wrap items-center justify-end gap-1.5 text-xs text-muted">{summary}</span>
      </summary>
      <div className="border-t border-border px-4 py-4">{children}</div>
    </details>
  );
}

export function Kpi({ label, value, detail, valueClass = "text-primary" }: { label: string; value: React.ReactNode; detail?: React.ReactNode; valueClass?: string }) {
  return (
    <div className="glass-card px-4 py-3">
      <div className="text-[11px] uppercase tracking-wider text-muted">{label}</div>
      <div className={`mt-0.5 text-2xl font-semibold ${valueClass}`}>{value}</div>
      {detail && <div className="text-xs text-muted">{detail}</div>}
    </div>
  );
}

export function HealthKpis({
  mobiles,
  rejected,
  calls,
  metrics,
}: {
  mobiles: number;
  rejected: number;
  calls: number;
  metrics: RadioMetrics | null;
}) {
  const tx = metrics?.tx;
  const rx = metrics?.rx;
  return (
    <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
      <Kpi label="Mobiles" value={mobiles} detail={`${mobiles - rejected} registered · ${rejected} rejected`} />
      <Kpi label="Calls" value={calls} detail={calls ? "on traffic channels" : "no traffic channels"} />
      <Kpi
        label="TX real time"
        value={tx?.rtRatio != null ? `${tx.rtRatio.toFixed(1)}×` : "—"}
        valueClass={ratioClass(tx?.rtRatio)}
        detail={tx ? `build ${micros(tx.genAvgUs)} avg · ${micros(tx.genMaxUs)} max` : "waiting for metrics"}
      />
      <Kpi
        label="RX real time"
        value={rx?.rtRatio != null ? `${rx.rtRatio.toFixed(1)}×` : "—"}
        valueClass={ratioClass(rx?.rtRatio)}
        detail={rx ? `${((rx.samples ?? 0) / 1e6).toFixed(2)} Msps · ${count(rx.reads)} reads/s` : "waiting for metrics"}
      />
    </div>
  );
}

function evdoCompositeOffsetKhz(config: BtsConfig): number | null {
  const evdo = config.evdo;
  if (!evdo || evdo.mode !== EvdoTxMode.EVDO_TX_MODE_ADJACENT_COMPOSITE || !evdo.compositeCenterFrequencyHz) return null;
  return Math.round((config.txCenterFrequencyHz - evdo.compositeCenterFrequencyHz) / 1000);
}

export function CarriersCard({ config }: { config: BtsConfig }) {
  const evdo = config.evdo;
  const hrpdOnly = evdo?.mode === EvdoTxMode.EVDO_TX_MODE_HRPD_ONLY;
  const offsetKhz = evdoCompositeOffsetKhz(config);
  return (
    <div className="glass-card p-4">
      <h2 className="glass-card-title -mx-4 -mt-4 mb-3 flex items-center justify-between gap-3">
        Carriers
        <span className="normal-case tracking-normal font-normal text-dimmed">{config.bandClass}</span>
      </h2>
      <table className="data-table">
        <thead>
          <tr>
            <th></th>
            <th>Channel</th>
            <th className="text-right">Forward</th>
            <th className="text-right">Reverse</th>
          </tr>
        </thead>
        <tbody>
          {!hrpdOnly && (
            <tr>
              <td><Chip tone="info">1x</Chip></td>
              <td className="font-mono">{config.cdmaChannel}</td>
              <td className="text-right font-mono">{mhz(config.txCenterFrequencyHz)}</td>
              <td className="text-right font-mono">{mhz(config.rxCenterFrequencyHz)}</td>
            </tr>
          )}
          {evdo && (
            <tr>
              <td><Chip tone="evdo">EV-DO</Chip></td>
              <td className="font-mono">{evdo.channel}</td>
              <td className="text-right font-mono">{mhz(evdo.frequencyHz)}</td>
              <td className="text-right font-mono">{mhz(evdo.reverseFrequencyHz)}</td>
            </tr>
          )}
        </tbody>
      </table>
      <p className="mt-2 text-xs text-dimmed">
        {offsetKhz != null && evdo
          ? `One composite carrier: LO at ${mhz(evdo.compositeCenterFrequencyHz)}, 1x at ${offsetKhz > 0 ? "+" : ""}${offsetKhz} kHz. `
          : ""}
        {(config.txSampleRateHz / 1e6).toFixed(4)} Msps · {mhz(config.txBandwidthHz, 2)} bandwidth
      </p>
    </div>
  );
}

export function CodeChannelsCard({ config }: { config: BtsConfig }) {
  const rows = [
    { name: "Pilot", channel: config.pilot, rate: undefined as number | undefined },
    { name: "Paging", channel: config.paging, rate: config.paging?.dataRateBps },
    { name: "Sync", channel: config.sync, rate: config.sync?.dataRateBps },
  ].filter((row) => row.channel);
  const used = rows.reduce((sum, row) => sum + (row.channel?.powerFraction ?? 0), 0);
  return (
    <div className="glass-card p-4">
      <h2 className="glass-card-title -mx-4 -mt-4 mb-3 flex items-center justify-between gap-3">
        Forward code channels
        <span className="normal-case tracking-normal font-normal text-dimmed">{(used * 100).toFixed(1)}% of TX power</span>
      </h2>
      <table className="data-table">
        <thead>
          <tr>
            <th>Channel</th>
            <th>Walsh</th>
            <th className="text-right">Rate</th>
            <th className="text-right">Power</th>
            <th className="w-1/4"></th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.name}>
              <td>{row.name}</td>
              <td className="font-mono">W{row.channel!.walshCode}</td>
              <td className="text-right font-mono">{row.rate ? `${row.rate} bps` : <span className="text-dimmed">—</span>}</td>
              <td className="text-right font-mono">{powerDb(row.channel!.powerFraction)}</td>
              <td>
                <div className="h-1.5 overflow-hidden rounded-full bg-surface-raised">
                  <i className="block h-full bg-accent-green" style={{ width: `${Math.min(100, row.channel!.powerFraction * 100)}%` }} />
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function authLabel(mode: number): string {
  if (mode === 0) return "Off (00)";
  if (mode === 1) return "On (01)";
  return String(mode);
}

export function ConfigurationSection({ config, softwareVersion }: { config: BtsConfig; softwareVersion?: string }) {
  const overhead: OverheadConfig | undefined = config.overhead;
  if (!overhead) return null;
  const zoneMinutes = ZONE_TIMER_MINUTES[overhead.zoneTimer];
  const slotSeconds = SLOT_CYCLE_BASE_S * 2 ** overhead.maxSlotCycleIndex;
  const tz = config.timezoneStatus;
  const ltmOff = tz?.ltmOff ?? overhead.ltmOff;
  const daylt = tz ? tz.daylt : overhead.daylt;
  const lpSec = tz?.lpSec ?? overhead.lpSec;
  const channelFields = [overhead.cdmaFreq != null && "CDMA_FREQ", overhead.extCdmaFreq != null && "EXT_CDMA_FREQ"]
    .filter(Boolean)
    .join(" · ");
  return (
    <Fold
      title="Configuration · what the cell broadcasts"
      summary={`P_REV ${overhead.pRev} · BASE_ID ${overhead.baseId}${tz?.tz ? ` · ${tz.tz}` : ""}`}
      defaultOpen
    >
      <div className="grid grid-cols-1 gap-6 sm:grid-cols-2 xl:grid-cols-4">
        <Group title="Identity">
          <Kv
            rows={[
              ["System ID", overhead.sid, "SID"],
              ["Network ID", overhead.nid, "NID"],
              ["Base station ID", overhead.baseId, "BASE_ID"],
              ["Band class", overhead.bandClassOverride != null ? `BC${overhead.bandClassOverride}` : config.bandClass, overhead.bandClassOverride != null ? "BAND_CLASS" : undefined],
              ["Channel", overhead.extCdmaFreq ?? overhead.cdmaFreq ?? config.cdmaChannel, channelFields || undefined],
              ["Protocol revision", `${overhead.pRev}, min ${overhead.minPRev}`, "P_REV · MIN_P_REV"],
              ["Software", softwareVersion || <span className="text-dimmed">not reported</span>],
            ]}
          />
        </Group>
        <Group title="Registration">
          <Kv
            rows={[
              ["On power-up", overhead.powerUpReg ? "Yes" : "No", "POWER_UP_REG"],
              ["On parameter change", overhead.parameterReg ? "Yes" : "No", "PARAMETER_REG"],
              ["Zone", overhead.regZone, "REG_ZONE"],
              ["Zones a mobile keeps", overhead.totalZones, "TOTAL_ZONES"],
              ["Zone timer", zoneMinutes != null ? `${overhead.zoneTimer} (${zoneMinutes} min)` : overhead.zoneTimer, "ZONE_TIMER"],
              ["Authentication", authLabel(overhead.authMode), "AUTH"],
            ]}
          />
        </Group>
        <Group title="Paging">
          <Kv
            rows={[
              ["Paging channels", overhead.pageChan, "PAGE_CHAN"],
              ["Max slot cycle", `${overhead.maxSlotCycleIndex} (${slotSeconds.toFixed(2)} s)`, "MAX_SLOT_CYCLE_INDEX"],
              ["Config sequence", overhead.configSeq, "CONFIG_SEQ"],
              ["Access config sequence", overhead.accConfigSeq, "ACC_CONFIG_SEQ"],
            ]}
          />
        </Group>
        <Group title="Time">
          <Kv
            rows={[
              ["Time zone", tz?.tz ? <>{tz.tz}{config.timezone?.source && <span className="block text-[11px] text-dimmed">from {config.timezone.source}</span>}</> : <span className="text-dimmed">fixed offset</span>],
              ["Local offset", `${utcOffset(ltmOff)} (${ltmOff})`, "LTM_OFF"],
              ["Daylight saving", daylt ? "Yes" : "No", "DAYLT"],
              ["Leap seconds", lpSec, "LP_SEC"],
            ]}
          />
        </Group>
      </div>
    </Fold>
  );
}

export function TransmitterSection({ config }: { config: BtsConfig }) {
  const pilot = config.pilot?.powerFraction ?? 0;
  const paging = config.paging?.powerFraction ?? 0;
  const sync = config.sync?.powerFraction ?? 0;
  const free = Math.max(0, 1 - pilot - paging - sync);
  const backoffDb = config.txDigitalBackoff > 0 ? 20 * Math.log10(config.txDigitalBackoff) : null;
  const backoff = `×${config.txDigitalBackoff.toFixed(2)}${backoffDb != null ? ` (${backoffDb.toFixed(1)} dB)` : ""}`;
  const oversample = config.chipRateHz ? Math.round(config.txSampleRateHz / config.chipRateHz) : null;
  return (
    <Fold title="Transmitter" summary={`${config.spreadingRate.toUpperCase()} · ${(config.txSampleRateHz / 1e6).toFixed(4)} Msps · backoff ${backoff}`}>
      <div className="grid grid-cols-1 gap-6 md:grid-cols-3">
        <Group title="Waveform">
          <Kv
            rows={[
              ["Spreading rate", config.spreadingRate.toUpperCase()],
              ["Chip rate", `${(config.chipRateHz / 1e6).toFixed(4)} Mcps`],
              ["Sample rate", `${(config.txSampleRateHz / 1e6).toFixed(4)} Msps${oversample ? ` (${oversample}×)` : ""}`],
              ["Block size", `${config.blockSizeChips} chips`],
              ["Pilot offset", `PN ${config.pilotOffset}`],
            ]}
          />
        </Group>
        <Group title="RF">
          <Kv
            rows={[
              ["TX center", mhz(config.txCenterFrequencyHz)],
              ["RX center", mhz(config.rxCenterFrequencyHz)],
              ["Bandwidth", mhz(config.txBandwidthHz, 2)],
              ["Band subclass", config.bandSubclass],
              ["Digital backoff", backoff],
            ]}
          />
        </Group>
        <Group title="Forward power split">
          <StackBar
            parts={[
              { label: `Pilot ${(pilot * 100).toFixed(1)}%`, value: pilot, className: "bg-accent-green" },
              { label: `Paging ${(paging * 100).toFixed(1)}%`, value: paging, className: "bg-accent-blue" },
              { label: `Sync ${(sync * 100).toFixed(1)}%`, value: sync, className: "bg-accent-amber" },
              { label: `Free ${(free * 100).toFixed(1)}%`, value: free, className: "bg-surface-raised" },
            ]}
          />
          <p className="mt-2 text-xs text-dimmed">Traffic channels take their power from the free share.</p>
        </Group>
      </div>
    </Fold>
  );
}

function evdoModeLabel(mode: number): string {
  switch (mode) {
    case EvdoTxMode.EVDO_TX_MODE_ADJACENT_COMPOSITE:
      return "Composite with 1x";
    case EvdoTxMode.EVDO_TX_MODE_DUAL_RF:
      return "Separate RF";
    case EvdoTxMode.EVDO_TX_MODE_HRPD_ONLY:
      return "EV-DO only";
    default:
      return "—";
  }
}

export function EvdoCarrierSection({
  evdo,
  sessions,
  connected,
  uati,
  sessionsHref,
}: {
  evdo?: EvdoCarrierConfig;
  sessions: number | null;
  connected: number | null;
  uati: { inUse: number; capacity: number } | null;
  sessionsHref: string;
}) {
  if (!evdo) return null;
  const levelDb = evdo.gain > 0 ? 20 * Math.log10(evdo.gain) : null;
  const sectorId = evdo.sectorId.replace(/(.{4})(?=.)/g, "$1 ");
  return (
    <Fold
      title="EV-DO carrier"
      summary={
        <>
          <span>channel {evdo.channel} · color code {evdo.colorCode} · /{evdo.subnetMask}</span>
          {sessions != null && <Chip tone="evdo">{sessions} sessions</Chip>}
        </>
      }
    >
      <div className="grid grid-cols-1 gap-6 md:grid-cols-3">
        <Group title="Carrier">
          <Kv
            rows={[
              ["Channel", `${evdo.channel} (BC${evdo.bandClass})`],
              ["Forward", mhz(evdo.frequencyHz)],
              ["Reverse", mhz(evdo.reverseFrequencyHz)],
              ["Mode", evdoModeLabel(evdo.mode)],
              ...(evdo.mode === EvdoTxMode.EVDO_TX_MODE_ADJACENT_COMPOSITE
                ? ([["LO", mhz(evdo.compositeCenterFrequencyHz)]] as [string, string][])
                : []),
              ["Level vs 1x", levelDb != null ? `${levelDb.toFixed(1)} dB (×${evdo.gain.toFixed(2)})` : "off"],
            ]}
          />
        </Group>
        <Group title="Access network">
          <Kv
            rows={[
              ["Color code", evdo.colorCode],
              ["Subnet", `/${evdo.subnetMask}`],
              ["Advertised on 1x", evdo.advertiseOn1x ? "Yes" : "No"],
            ]}
          />
          <div className="mt-2 text-xs text-dimmed">Sector ID</div>
          <div className="break-all font-mono text-xs text-secondary">{sectorId}</div>
        </Group>
        <Group title="Sessions">
          <Kv
            rows={[
              ["Open", sessions ?? "—"],
              ["Connected now", connected ?? "—"],
              ["UATIs in use", uati ? `${uati.inUse.toLocaleString()} of ${uati.capacity.toLocaleString()}` : "—"],
            ]}
          />
          <Link href={sessionsHref} className="mt-2 inline-block text-xs text-accent-green hover:underline">
            EV-DO sessions →
          </Link>
        </Group>
      </div>
    </Fold>
  );
}

export function DiagnosticsSection({ metrics }: { metrics: RadioMetrics | null }) {
  const tx = metrics?.tx;
  const rx = metrics?.rx;
  const bearer = metrics?.bearer;
  const health = tx?.radioHealth;
  const faults =
    (health?.underflows ?? 0) + (health?.latePackets ?? 0) + (health?.sequenceErrors ?? 0) + (health?.droppedPackets ?? 0);
  const degraded = tx?.realtimeDegradedEvents ?? 0;
  const synth = [
    { label: "Pilot", value: tx?.synthPilotUs ?? 0, className: "bg-accent-green" },
    { label: "Sync", value: tx?.synthSyncUs ?? 0, className: "bg-accent-amber" },
    { label: "Paging", value: tx?.synthPagingUs ?? 0, className: "bg-accent-blue" },
    { label: "Spreading", value: tx?.synthSpreadUs ?? 0, className: "bg-accent-purple" },
  ];
  const synthTotal = synth.reduce((sum, part) => sum + part.value, 0);
  const queues = rx?.queues ?? [];
  return (
    <Fold
      title="Diagnostics · TX and RX timing"
      summary={
        metrics ? (
          <>
            <Chip tone={(tx?.rtRatio ?? 0) >= RT_RATIO_WARN ? "ok" : "warn"}>TX {tx?.rtRatio?.toFixed(1) ?? "—"}×</Chip>
            <Chip tone={(rx?.rtRatio ?? 0) >= RT_RATIO_WARN ? "ok" : "warn"}>RX {rx?.rtRatio?.toFixed(1) ?? "—"}×</Chip>
            {faults > 0 && <Chip tone="bad">{faults} radio faults</Chip>}
          </>
        ) : (
          "waiting for metrics"
        )
      }
    >
      {!metrics ? (
        <p className="text-sm text-dimmed">Waiting for metrics from the BTS.</p>
      ) : (
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
            <div className="rounded-lg border border-border p-3">
              <div className="mb-2 flex items-center justify-between text-[11px] uppercase tracking-wider text-muted">
                Transmitter <Chip tone={(tx?.rtRatio ?? 0) >= RT_RATIO_WARN ? "ok" : "warn"}>{tx?.rtRatio?.toFixed(1) ?? "—"}× real time</Chip>
              </div>
              <Kv
                rows={[
                  ["Block build, avg / max", `${micros(tx?.genAvgUs)} / ${micros(tx?.genMaxUs)}`],
                  ["Block send, avg / max", `${micros(tx?.txAvgUs)} / ${micros(tx?.txMaxUs)}`],
                  ...(tx?.hwMarginMinUs != null
                    ? ([["Hardware margin", `${micros(tx.hwMarginMinUs)} – ${micros(tx.hwMarginMaxUs)}`]] as [string, string][])
                    : []),
                  ["Late batches", count(tx?.lateBatches)],
                  ["Blocks sent", count(tx?.blocksTransmitted)],
                  ["Sync / paging fragments", `${count(tx?.syncFragmentsSent)} / ${count(tx?.pagingFragmentsSent)}`],
                ]}
              />
              {synthTotal > 0 && (
                <div className="mt-3">
                  <div className="mb-1 text-xs text-dimmed">Build time by channel</div>
                  <StackBar
                    parts={synth.map((part) => ({ ...part, label: `${part.label} ${Math.round((part.value / synthTotal) * 100)}%` }))}
                  />
                </div>
              )}
              {degraded > 0 && (
                <p className="mt-3 text-xs text-dimmed">
                  Running without real-time priority ({degraded} {degraded === 1 ? "thread" : "threads"}). The host does not allow
                  SCHED_FIFO.
                </p>
              )}
            </div>
            <div className="rounded-lg border border-border p-3">
              <div className="mb-2 flex items-center justify-between text-[11px] uppercase tracking-wider text-muted">
                Receiver <Chip tone={(rx?.rtRatio ?? 0) >= RT_RATIO_WARN ? "ok" : "warn"}>{rx?.rtRatio?.toFixed(1) ?? "—"}× real time</Chip>
              </div>
              <Kv
                rows={[
                  ["Reads", `${count(rx?.reads)} /s`],
                  ["Samples", `${((rx?.samples ?? 0) / 1e6).toFixed(2)} Msps`],
                  ["Pipeline", `${micros(rx?.pipelineUs)} per s`],
                  ["Worst read", micros(rx?.totalMaxUs)],
                  ...(rx?.deficitMs != null ? ([["Deficit", `${rx.deficitMs.toFixed(1)} ms`]] as [string, string][]) : []),
                ]}
              />
              {(rx?.stages?.length ?? 0) > 0 && (
                <table className="mt-3 data-table">
                  <thead>
                    <tr>
                      <th>Stage</th>
                      <th className="text-right">Share</th>
                      <th className="text-right">Max</th>
                    </tr>
                  </thead>
                  <tbody>
                    {rx!.stages!.map((stage) => (
                      <tr key={stage.name}>
                        <td  title={stage.name}>{stageLabel(stage.name)}</td>
                        <td className="text-right font-mono">{(stage.pctPipeline ?? 0).toFixed(1)}%</td>
                        <td className="text-right font-mono">{micros(stage.maxUs)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
            <div className="rounded-lg border border-border p-3">
              <div className="mb-2 flex items-center justify-between text-[11px] uppercase tracking-wider text-muted">
                Radio link <Chip tone={faults ? "bad" : "ok"}>{faults ? `${faults} faults` : "No faults"}</Chip>
              </div>
              <Kv
                rows={[
                  ["Underflows", count(health?.underflows)],
                  ["Late packets", count(health?.latePackets)],
                  ["Sequence errors", count(health?.sequenceErrors)],
                  ["Dropped packets", count(health?.droppedPackets)],
                ]}
              />
              {bearer && (
                <>
                  <div className="mb-1 mt-3 text-xs text-dimmed">Voice and data bearer</div>
                  <Kv
                    rows={[
                      ["Frames sent / received", `${count(bearer.txFrames)} / ${count(bearer.rxAccepted)}`],
                      ["Duplicate / late drops", `${count(bearer.duplicateDrop)} / ${count(bearer.lateDrop)}`],
                      ["Errors", count((bearer.encodeErrors ?? 0) + (bearer.routeErrors ?? 0) + (bearer.deliveryErrors ?? 0))],
                    ]}
                  />
                </>
              )}
            </div>
          </div>
          {queues.length > 0 && (
            <div className="rounded-lg border border-border p-3">
              <div className="mb-1 flex items-center justify-between text-[11px] uppercase tracking-wider text-muted">
                RX queues
                <span className="normal-case tracking-normal text-dimmed">A queue that stays full means its worker is falling behind.</span>
              </div>
              <table className="data-table">
                <thead>
                  <tr>
                    <th>Queue</th>
                    <th className="text-right">Now</th>
                    <th className="text-right">Peak</th>
                    <th className="text-right">Longest wait</th>
                    <th className="w-1/5"></th>
                  </tr>
                </thead>
                <tbody>
                  {queues.map((queue) => {
                    const fill = queue.maxQueuedSamples ? (queue.queuedSamples ?? 0) / queue.maxQueuedSamples : 0;
                    return (
                      <tr key={queue.name}>
                        <td  title={queue.name}>{queueLabel(queue.name)}</td>
                        <td className="text-right font-mono">
                          {count(queue.queuedSamples)}
                          {queue.queuedAirtimeUs ? <span className="text-dimmed"> · {micros(queue.queuedAirtimeUs)}</span> : null}
                        </td>
                        <td className="text-right font-mono">{count(queue.maxQueuedSamples)}</td>
                        <td className="text-right font-mono">{micros(queue.maxResidencyUs)}</td>
                        <td>
                          <div className="h-1.5 overflow-hidden rounded-full bg-surface-raised">
                            <i className={`block h-full ${fill >= 0.9 ? "bg-accent-amber" : "bg-accent-green"}`} style={{ width: `${Math.min(100, fill * 100)}%` }} />
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </Fold>
  );
}
