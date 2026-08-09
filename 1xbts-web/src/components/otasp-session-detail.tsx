"use client";

import { Fragment } from "react";
import { Card, Stat } from "@/components/card";
import { formatTimeMs as formatTime } from "@/lib/format";
import { formatRoamingIndicator } from "@/lib/prl-options";
import {
  blockIdLabel,
  featureIdLabel,
  featureIdNumber,
  featurePRevLabel,
  formatEsnHex,
  formatMeidColon,
  outcomeBadgeColor,
  outcomeNameToLabel,
  resultCodeLabel,
  serviceOptionLabel,
} from "@/lib/otasp";

// Minimal interface mirroring the OtaspEvent + record proto objects
// returned by `/api/otasp-events/[sessionId]`. Replaced by the
// ts-proto generated types after `npm run proto`.
interface OtaspHardwareIdentity {
  esn?: number;
  meid?: string;
}

interface OtaspEventJson {
  sessionStart?: {
    device?: OtaspHardwareIdentity;
    featureCode?: string;
    serviceOption?: number;
  };
  protocolCapabilityReceived?: {
    mobFirmRev?: number;
    mobModel?: number;
    bandModeCap?: {
      raw?: number;
      bandClass0Analog?: boolean;
      bandClass0Cdma?: boolean;
      bandClass1Cdma?: boolean;
      bandClass3Cdma?: boolean;
      bandClass6Cdma?: boolean;
      reserved?: number;
    };
    otaspPRev?: number;
    features?: {
      featureId?: string | number;
      featurePRev?: number;
      featureIdRaw?: number;
    }[];
  };
  spcMismatch?: Record<string, never>;
  spcVerified?: Record<string, never>;
  hlrMiss?: { device?: OtaspHardwareIdentity };
  noNamCapacity?: { blockId?: number; feature?: string | number };
  blockSkipped?: { blockId?: number; reason?: string; feature?: string | number };
  blockDownloaded?: {
    blockId?: number;
    resultCode?: number;
    feature?: string | number;
    fields?: { name?: string; value?: string }[];
  };
  blockRejected?: { blockId?: number; resultCode?: number; feature?: string | number };
  commitResult?: { resultCode?: number };
  timeout?: { phase?: string };
  namReadback?: {
    blockId?: number;
    label?: string;
    fields?: { name?: string; value?: string }[];
    feature?: string | number;
  };
  stationClassMark?: {
    raw?: number;
    extended?: string | number;
    dualMode?: string | number;
    slottedClass?: string | number;
    meidSupport?: string | number;
    bandwidth25mhz?: boolean;
    transmission?: string | number;
    analogPowerClass?: number;
  };
  prlReadback?: PrlReadbackJson;
  sessionEnded?: { completedBlocks?: number; outcome?: string | number };
}

interface PrlReadbackJson {
  maxPrListSize?: number;
  curPrListSize?: number;
  prListId?: number;
  segmentCount?: number;
  decoded?: PrlDecodedJson;
  decodedExtended?: PrlDecodedExtendedJson;
  absent?: Record<string, never>;
  featureNotAdvertised?: Record<string, never>;
  rejected?: { blockId?: number; resultCode?: number };
  decodeFailed?: { reason?: string; rawBytes?: string };
}

interface PrlDecodedExtendedJson {
  prListSize?: number;
  prListId?: number;
  curSsprPRev?: number;
  prefOnly?: boolean;
  defRoamInd?: PrlRoamingJson;
  prListCrc?: number;
  computedCrc?: number;
  crcOk?: boolean;
  numAcqRecords?: number;
  numCommonSubnetRecords?: number;
  numExtSysRecords?: number;
  rawBytes?: string;
}

interface PrlRoamingJson {
  raw?: number;
  kind?: string | number;
}

interface PrlDecodedJson {
  prListSize?: number;
  prListId?: number;
  prefOnly?: boolean;
  defRoamInd?: PrlRoamingJson;
  prListCrc?: number;
  computedCrc?: number;
  crcOk?: boolean;
  acquisitionRecords?: PrlAcqRecordJson[];
  systemRecords?: PrlSysRecordJson[];
  rawBytes?: string;
}

interface PrlAcqRecordJson {
  acqTypeRaw?: number;
  cellularAnalog?: { ab?: string | number };
  cellularCdmaStandard?: { ab?: string | number; priSec?: string | number };
  cellularCdmaCustom?: { channels?: number[] };
  cellularCdmaPreferred?: { ab?: string | number };
  pcsCdmaUsingBlocks?: { blocks?: (string | number)[] };
  pcsCdmaUsingChannels?: { channels?: number[] };
  jtacsCdmaStandard?: { ab?: string | number; priSec?: string | number };
  jtacsCdmaCustom?: { channels?: number[] };
  bandClass6UsingChannels?: { channels?: number[] };
  unknown?: Record<string, never>;
}

interface PrlSysRecordJson {
  sid?: number;
  nidIncl?: string | number;
  nid?: number;
  sameGeoAsPrev?: boolean;
  prefNeg?: string | number;
  acqIndex?: number;
  roamingIndicator?: PrlRoamingJson;
  priority?: string | number;
}

interface SessionEventRecord {
  timestamp?: string;
  event?: OtaspEventJson;
}

interface SessionDetail {
  summary?: {
    sessionId?: string;
    esn?: number;
    meid?: string;
    startedAt?: string;
    endedAt?: string;
    outcome?: string | number;
    eventCount?: number;
  };
  events?: SessionEventRecord[];
}

function isoToMs(s?: string): number | null {
  if (!s) return null;
  const t = Date.parse(s);
  return Number.isFinite(t) ? t : null;
}

function relTime(ts: number): string {
  const delta = Date.now() - ts;
  const sec = Math.round(delta / 1000);
  if (sec < 60) return `${sec}s ago`;
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return `${Math.round(hr / 24)}d ago`;
}

function scmEnumStr(v: string | number | undefined): string {
  return typeof v === "string" ? v : "";
}

function scmExtendedLabel(v: string | number | undefined): string {
  const s = scmEnumStr(v);
  if (s.endsWith("PCS_FAMILY") || v === 2) return "PCS family (band classes 1, 4, 14)";
  if (s.endsWith("STANDARD_BANDS") || v === 1) return "Standard bands";
  return "unspecified";
}

function scmDualModeLabel(v: string | number | undefined): string {
  const s = scmEnumStr(v);
  if (s.endsWith("DUAL") || v === 2) return "Dual analog/CDMA";
  if (s.endsWith("CDMA_ONLY") || v === 1) return "CDMA only";
  return "unspecified";
}

function scmSlottedLabel(v: string | number | undefined): string {
  const s = scmEnumStr(v);
  if (s.endsWith("SLOTTED") && !s.endsWith("NON_SLOTTED")) return "Slotted (battery save)";
  if (v === 2) return "Slotted (battery save)";
  if (s.endsWith("NON_SLOTTED") || v === 1) return "Non-slotted";
  return "unspecified";
}

function scmMeidLabel(v: string | number | undefined): string {
  const s = scmEnumStr(v);
  if (s.endsWith("CONFIGURED") && !s.endsWith("NOT_CONFIGURED")) return "Configured";
  if (v === 2) return "Configured";
  if (s.endsWith("NOT_CONFIGURED") || v === 1) return "Not configured (ESN-only)";
  return "unspecified";
}

function scmTransmissionLabel(v: string | number | undefined): string {
  const s = scmEnumStr(v);
  if (s.endsWith("DISCONTINUOUS") || v === 2) return "Discontinuous (DTX)";
  if (s.endsWith("CONTINUOUS") || v === 1) return "Continuous";
  return "unspecified";
}

function decodeAscii(value: number): string {
  if (value >= 0x20 && value < 0x7f) return String.fromCharCode(value);
  return `0x${value.toString(16).toUpperCase()}`;
}

function hexByte(value: number | undefined): string {
  return `0x${(value ?? 0).toString(16).toUpperCase().padStart(2, "0")}`;
}

function endsWith(s: string | number | undefined, suffix: string): boolean {
  return typeof s === "string" && s.endsWith(suffix);
}

function abLabel(v: string | number | undefined): string {
  if (endsWith(v, "SYSTEM_A") || v === 1) return "System A";
  if (endsWith(v, "SYSTEM_B") || v === 2) return "System B";
  if (endsWith(v, "RESERVED") || v === 3) return "Reserved";
  if (endsWith(v, "EITHER") || v === 4) return "System A or B";
  return "—";
}

function stdChanLabel(v: string | number | undefined): string {
  if (endsWith(v, "RESERVED") || v === 1) return "Reserved";
  if (endsWith(v, "PRIMARY_OR_SECONDARY") || v === 4) return "Primary or Secondary";
  if (endsWith(v, "PRIMARY") || v === 2) return "Primary";
  if (endsWith(v, "SECONDARY") || v === 3) return "Secondary";
  return "—";
}

function pcsBlockLabel(v: string | number | undefined): string {
  const map: Record<string, string> = {
    A: "A", B: "B", C: "C", D: "D", E: "E", F: "F",
    RESERVED: "Reserved", ANY: "Any",
  };
  if (typeof v === "string") {
    for (const k of Object.keys(map)) if (v.endsWith(`_${k}`) || v === k) return map[k];
  }
  if (typeof v === "number") {
    return ["—", "A", "B", "C", "D", "E", "F", "Reserved", "Any"][v] ?? "—";
  }
  return "—";
}

function nidInclLabel(v: string | number | undefined): string {
  if (endsWith(v, "ANY") || v === 1) return "Any NID (wildcard)";
  if (endsWith(v, "SINGLE") || v === 2) return "Single NID";
  if (endsWith(v, "PUBLIC") || v === 3) return "Public NID (0x0000)";
  if (endsWith(v, "RESERVED") || v === 4) return "Reserved";
  return "—";
}

function prefNegLabel(v: string | number | undefined): string {
  if (endsWith(v, "PREFERRED") || v === 1) return "Preferred";
  if (endsWith(v, "NEGATIVE") || v === 2) return "Negative";
  return "—";
}

function priorityLabel(v: string | number | undefined): string {
  if (endsWith(v, "MORE_DESIRABLE") || v === 1) return "More desirable";
  if (endsWith(v, "EQUALLY_DESIRABLE") || v === 2) return "Equally desirable";
  return "—";
}

function roamingLabel(r: PrlRoamingJson | undefined): string {
  if (!r) return "—";
  const raw = r.raw ?? 0;
  return formatRoamingIndicator(raw);
}

function acqRowLabel(rec: PrlAcqRecordJson): { title: string; detail: string } {
  if (rec.cellularAnalog)
    return { title: "Cellular Analog", detail: abLabel(rec.cellularAnalog.ab) };
  if (rec.cellularCdmaStandard)
    return {
      title: "Cellular CDMA (Standard Channels)",
      detail: `${abLabel(rec.cellularCdmaStandard.ab)} · ${stdChanLabel(rec.cellularCdmaStandard.priSec)}`,
    };
  if (rec.cellularCdmaCustom)
    return {
      title: "Cellular CDMA (Custom Channels)",
      detail: `Channels: ${(rec.cellularCdmaCustom.channels ?? []).join(", ") || "—"}`,
    };
  if (rec.cellularCdmaPreferred)
    return { title: "Cellular CDMA Preferred", detail: abLabel(rec.cellularCdmaPreferred.ab) };
  if (rec.pcsCdmaUsingBlocks)
    return {
      title: "PCS CDMA (Using Blocks)",
      detail: `Blocks: ${(rec.pcsCdmaUsingBlocks.blocks ?? []).map(pcsBlockLabel).join(", ") || "—"}`,
    };
  if (rec.pcsCdmaUsingChannels)
    return {
      title: "PCS CDMA / 2 GHz (Using Channels)",
      detail: `Channels: ${(rec.pcsCdmaUsingChannels.channels ?? []).join(", ") || "—"}`,
    };
  if (rec.jtacsCdmaStandard)
    return {
      title: "JTACS CDMA (Standard Channels)",
      detail: `${abLabel(rec.jtacsCdmaStandard.ab)} · ${stdChanLabel(rec.jtacsCdmaStandard.priSec)}`,
    };
  if (rec.jtacsCdmaCustom)
    return {
      title: "JTACS CDMA (Custom Channels)",
      detail: `Channels: ${(rec.jtacsCdmaCustom.channels ?? []).join(", ") || "—"}`,
    };
  if (rec.bandClass6UsingChannels)
    return {
      title: "Band Class 6 (Using Channels)",
      detail: `Channels: ${(rec.bandClass6UsingChannels.channels ?? []).join(", ") || "—"}`,
    };
  return {
    title: `Unknown (raw ACQ_TYPE=0x${(rec.acqTypeRaw ?? 0).toString(16).padStart(2, "0").toUpperCase()})`,
    detail: "—",
  };
}

function DownloadPrlButton({
  base64,
  filename,
}: {
  base64: string;
  filename: string;
}) {
  const onClick = () => {
    // Proto bytes arrive as base64 over JSON. Decode and offer as
    // application/octet-stream so the browser saves a .prl file the
    // operator can re-import via /prls/new.
    const bin = atob(base64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const url = URL.createObjectURL(
      new Blob([bytes], { type: "application/octet-stream" }),
    );
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  };
  return (
    <button
      onClick={onClick}
      className="text-accent-blue hover:underline font-mono"
    >
      ⬇ Download {filename}
    </button>
  );
}

function PrlReadbackCard({ rb }: { rb: PrlReadbackJson }) {
  if (rb.featureNotAdvertised) {
    return (
      <p className="text-dimmed text-xs">
        MS did not advertise the SSPR feature, so the PRL was not read.
      </p>
    );
  }
  if (rb.absent) {
    return (
      <p className="text-xs">
        MS reports no PRL programmed (<span className="font-mono">CUR_PR_LIST_SIZE = 0</span>).
      </p>
    );
  }
  if (rb.decodedExtended) {
    const d = rb.decodedExtended;
    return (
      <div className="text-xs space-y-1">
        <p>
          ✓ Extended PRL (SSPR_P_REV = {d.curSsprPRev ?? 0}) retrieved.
          PR_LIST_ID =&nbsp;
          <span className="font-mono">{d.prListId ?? 0}</span>, size{" "}
          <span className="font-mono">{d.prListSize ?? 0}</span> octets,
          CRC {d.crcOk ? "OK" : "MISMATCH"}.
        </p>
        <p className="text-muted">
          {d.numAcqRecords ?? 0} acquisition records ·{" "}
          {d.numCommonSubnetRecords ?? 0} common subnet records ·{" "}
          {d.numExtSysRecords ?? 0} system records.
        </p>
        {d.rawBytes && (
          <p>
            <DownloadPrlButton
              base64={d.rawBytes}
              filename={`prl-${d.prListId ?? 0}.prl`}
            />
            <span className="text-dimmed ml-2">
              Import at /prls/new for full structured editing.
            </span>
          </p>
        )}
      </div>
    );
  }
  if (rb.rejected) {
    return (
      <p className="text-accent-red text-xs">
        ✗ MS rejected the SSPR Configuration Request — block_id=
        <span className="font-mono">
          0x{(rb.rejected.blockId ?? 0).toString(16).padStart(2, "0").toUpperCase()}
        </span>{" "}
        result=
        <span className="font-mono">
          0x{(rb.rejected.resultCode ?? 0).toString(16).padStart(2, "0").toUpperCase()}
        </span>
      </p>
    );
  }
  if (rb.decodeFailed) {
    const partialId = rb.prListId ?? 0;
    return (
      <div className="text-xs space-y-1">
        <p className="text-accent-red">✗ PRL decode failed.</p>
        <p className="text-muted">Reason: <span className="font-mono">{rb.decodeFailed.reason ?? ""}</span></p>
        {rb.decodeFailed.rawBytes && (
          <p>
            <DownloadPrlButton
              base64={rb.decodeFailed.rawBytes}
              filename={`prl-${partialId}-undecoded.prl`}
            />
            <span className="text-dimmed ml-2">Raw PRL.</span>
          </p>
        )}
      </div>
    );
  }
  if (!rb.decoded) return <p className="text-dimmed text-xs">(no PRL data)</p>;
  const d = rb.decoded;
  return (
    <div className="text-xs space-y-1">
      <p>
        ✓ Classic PRL (SSPR_P_REV = 1) retrieved. PR_LIST_ID =&nbsp;
        <span className="font-mono">{d.prListId ?? 0}</span>, size{" "}
        <span className="font-mono">{d.prListSize ?? 0}</span> octets,
        CRC {d.crcOk ? "OK" : "MISMATCH"}.
      </p>
      <p className="text-muted">
        {(d.acquisitionRecords ?? []).length} acquisition records ·{" "}
        {(d.systemRecords ?? []).length} system records ·{" "}
        DEF_ROAM_IND {roamingLabel(d.defRoamInd)} ·{" "}
        {d.prefOnly ? "PREF_ONLY" : "not pref-only"}.
      </p>
      {d.rawBytes && (
        <p>
          <DownloadPrlButton
            base64={d.rawBytes}
            filename={`prl-${d.prListId ?? 0}.prl`}
          />
          <span className="text-dimmed ml-2">
            Import at /prls/new for full structured editing.
          </span>
        </p>
      )}
    </div>
  );
}

function EventRow({ rec }: { rec: SessionEventRecord }) {
  const ms = isoToMs(rec.timestamp);
  const tsLabel = ms != null ? formatTime(ms) : "";
  const tsRel = ms != null ? relTime(ms) : "";
  const ev = rec.event ?? {};

  let title = "Event";
  let body: React.ReactNode = null;
  let tone = "border-border";

  if (ev.sessionStart) {
    const s = ev.sessionStart;
    title = "Session Start";
    tone = "border-accent-blue/20";
    body = (
      <div className="grid grid-cols-2 gap-x-6 text-xs">
        <Stat label="ESN" value={formatEsnHex(s.device?.esn)} mono />
        <Stat label="MEID" value={formatMeidColon(s.device?.meid)} mono />
        <Stat label="Feature Code" value={s.featureCode || "—"} mono />
        <Stat label="Service Option" value={serviceOptionLabel(s.serviceOption ?? 0)} />
      </div>
    );
  } else if (ev.protocolCapabilityReceived) {
    const pcr = ev.protocolCapabilityReceived;
    const bmc = pcr.bandModeCap ?? {};
    const bands: { label: string; supported: boolean }[] = [
      { label: "Band Class 0 Analog (cellular AMPS)", supported: !!bmc.bandClass0Analog },
      { label: "Band Class 0 CDMA (800 MHz cellular)", supported: !!bmc.bandClass0Cdma },
      { label: "Band Class 1 CDMA (1900 MHz PCS)", supported: !!bmc.bandClass1Cdma },
      { label: "Band Class 3 CDMA (JTACS / Japan 800)", supported: !!bmc.bandClass3Cdma },
      { label: "Band Class 6 CDMA (2 GHz)", supported: !!bmc.bandClass6Cdma },
    ];
    title = "Protocol Capability Received";
    tone = "border-accent-blue/20";
    body = (
      <div className="space-y-2 text-xs">
        <div className="grid grid-cols-2 gap-x-6">
          <Stat label="MOB_FIRM_REV" value={`0x${(pcr.mobFirmRev ?? 0).toString(16).toUpperCase()}`} mono />
          <Stat label="MOB_MODEL" value={`0x${(pcr.mobModel ?? 0).toString(16).toUpperCase()} (${decodeAscii(pcr.mobModel ?? 0)})`} mono />
          <Stat label="OTASP_P_REV" value={pcr.otaspPRev != null ? `0x${pcr.otaspPRev.toString(16).toUpperCase().padStart(2, "0")}` : "—"} mono />
          <Stat label="BAND_MODE_CAP" value={`0x${(bmc.raw ?? 0).toString(16).toUpperCase().padStart(2, "0")}${(bmc.reserved ?? 0) !== 0 ? ` (RESERVED=${bmc.reserved})` : ""}`} mono />
        </div>
        <div>
          <div className="text-muted text-[11px] uppercase tracking-wide mb-1">Band/Mode Capability</div>
          <ul className="space-y-0.5">
            {bands.map((b) => (
              <li
                key={b.label}
                className={`text-xs ${b.supported ? "text-accent-green" : "text-dimmed"}`}
              >
                <span className="mr-2">{b.supported ? "✓" : "○"}</span>
                {b.label}
              </li>
            ))}
          </ul>
        </div>
        {pcr.features && pcr.features.length > 0 && (
          <div>
            <div className="text-muted text-[11px] uppercase tracking-wide mb-1">Advertised Features</div>
            <ul className="space-y-0.5">
              {pcr.features.map((f, i) => {
                const featureId = featureIdNumber(f.featureId, f.featureIdRaw);
                const featurePRev = f.featurePRev ?? 0;
                return (
                  <li key={`${featureId}-${i}`} className="text-xs text-accent-green">
                    <span className="mr-2">✓</span>
                    <span className="font-mono mr-2">{hexByte(featureId)}</span>
                    {featureIdLabel(featureId)}
                    <span className="text-muted ml-2">
                      (P_REV {hexByte(featurePRev)} · {featurePRevLabel(featureId, featurePRev)})
                    </span>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </div>
    );
  } else if (ev.spcMismatch) {
    title = "SPC Mismatch";
    tone = "border-accent-red/40";
    body = (
      <p className="text-accent-red text-xs">
        ✗ Service Programming Code mismatch — operator must verify SPC before programming.
      </p>
    );
  } else if (ev.spcVerified) {
    title = "SPC Verified";
    tone = "border-accent-green/30";
    body = (
      <p className="text-accent-green text-xs">
        ✓ Service Programming Code verified — programming may proceed.
      </p>
    );
  } else if (ev.blockSkipped) {
    const s = ev.blockSkipped;
    title = "Block Skipped";
    tone = "border-accent-yellow/30";
    body = (
      <p className="text-accent-yellow text-xs">
        ⊘ {s.reason ?? "Skipped — feature not supported by MS."}
      </p>
    );
  } else if (ev.timeout) {
    title = "Inbound Silence Timeout";
    tone = "border-accent-red/40";
    body = (
      <p className="text-accent-red text-xs">
        ✗ MS did not respond within the threshold. Was waiting on: {ev.timeout.phase ?? "next response"}.
        Releasing the call.
      </p>
    );
  } else if (ev.hlrMiss) {
    const d = ev.hlrMiss.device;
    title = "HLR Miss";
    tone = "border-accent-red/40";
    body = (
      <p className="text-accent-red text-xs">
        ✗ No HLR record for device {formatEsnHex(d?.esn)}{d?.meid ? ` / MEID ${formatMeidColon(d?.meid)}` : ""}.
        Pre-provision the subscriber to allow OTASP.
      </p>
    );
  } else if (ev.noNamCapacity) {
    title = "No NAM Capacity";
    tone = "border-accent-red/40";
    body = (
      <p className="text-accent-red text-xs">
        ✗ Mobile reports MAX_SID_NID = 0 on {blockIdLabel(ev.noNamCapacity.blockId ?? 0, ev.noNamCapacity.feature)} — cannot store any home SID.
      </p>
    );
  } else if (ev.namReadback) {
    const r = ev.namReadback;
    title = `NAM Read-Back — ${r.label ?? blockIdLabel(r.blockId ?? 0, r.feature)}`;
    tone = "border-accent-blue/20";
    body = (
      <div className="text-xs">
        <div className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-0.5">
          {(r.fields ?? []).map((f, i) => (
            <Fragment key={`${f.name}-${i}`}>
              <span className="text-muted">{f.name ?? ""}</span>
              <span className="font-mono">{f.value ?? ""}</span>
            </Fragment>
          ))}
        </div>
      </div>
    );
  } else if (ev.stationClassMark) {
    const s = ev.stationClassMark;
    title = "Station Class Mark";
    tone = "border-accent-blue/20";
    const rows: [string, string][] = [
      ["Raw", `0x${(s.raw ?? 0).toString(16).padStart(2, "0").toUpperCase()}`],
      ["Extended (bit 7)", scmExtendedLabel(s.extended)],
      ["Dual mode (bit 6)", scmDualModeLabel(s.dualMode)],
      ["Slotted class (bit 5)", scmSlottedLabel(s.slottedClass)],
      ["MEID support (bit 4)", scmMeidLabel(s.meidSupport)],
      ["25 MHz bandwidth (bit 3)", s.bandwidth25mhz ? "yes" : "no"],
      ["Transmission (bit 2)", scmTransmissionLabel(s.transmission)],
      ["Analog power class (bits 1–0)", String(s.analogPowerClass ?? 0)],
    ];
    body = (
      <div className="text-xs">
        <div className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-0.5">
          {rows.map(([k, v]) => (
            <Fragment key={k}>
              <span className="text-muted">{k}</span>
              <span className="font-mono">{v}</span>
            </Fragment>
          ))}
        </div>
      </div>
    );
  } else if (ev.prlReadback) {
    const rb = ev.prlReadback;
    title = "PRL Read-Back";
    tone = "border-accent-blue/20";
    body = <PrlReadbackCard rb={rb} />;
  } else if (ev.blockDownloaded) {
    const d = ev.blockDownloaded;
    title = `Block Downloaded — ${blockIdLabel(d.blockId ?? 0, d.feature)}`;
    tone = "border-accent-green/30";
    const fields = d.fields ?? [];
    body = (
      <div className="text-xs space-y-2">
        <p className="text-accent-green">
          ✓ {resultCodeLabel(d.resultCode ?? 0)}
        </p>
        {fields.length > 0 && (
          <div className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-0.5">
            {fields.map((f, i) => (
              <Fragment key={`${f.name}-${i}`}>
                <span className="text-muted">{f.name ?? ""}</span>
                <span className="font-mono">{f.value ?? ""}</span>
              </Fragment>
            ))}
          </div>
        )}
      </div>
    );
  } else if (ev.blockRejected) {
    const d = ev.blockRejected;
    title = `Block Rejected — ${blockIdLabel(d.blockId ?? 0, d.feature)}`;
    tone = "border-accent-red/40";
    body = (
      <p className="text-accent-red text-xs">
        ✗ {resultCodeLabel(d.resultCode ?? 0)}
      </p>
    );
  } else if (ev.commitResult) {
    const rc = ev.commitResult.resultCode ?? 0;
    title = "Commit Result";
    tone = rc === 0 ? "border-accent-green/30" : "border-accent-red/40";
    body = rc === 0 ? (
      <p className="text-accent-green text-xs">✓ Committed</p>
    ) : (
      <p className="text-accent-red text-xs">✗ Commit rejected — {resultCodeLabel(rc)}</p>
    );
  } else if (ev.sessionEnded) {
    const se = ev.sessionEnded;
    title = "Session Ended";
    tone = "border-border";
    body = (
      <p className="text-xs">
        <span className={`px-2 py-0.5 rounded ${outcomeBadgeColor(se.outcome)}`}>
          {outcomeNameToLabel(se.outcome)}
        </span>
        <span className="ml-3 text-muted">
          Completed blocks: <span className="font-mono">{se.completedBlocks ?? 0}</span>
        </span>
      </p>
    );
  } else {
    body = <p className="text-dimmed text-xs">(unknown event variant)</p>;
  }

  return (
    <div className={`border-l-4 pl-3 py-2 ${tone}`}>
      <div className="flex items-baseline gap-3">
        <span className="text-primary text-sm font-medium">{title}</span>
        {ms != null && (
          <span className="text-muted font-mono text-[11px]" title={tsLabel}>
            {tsRel}
          </span>
        )}
      </div>
      <div className="mt-1.5">{body}</div>
    </div>
  );
}

export function OtaspSessionDetail({ session }: { session: SessionDetail }) {
  const sum = session.summary ?? {};
  const events = session.events ?? [];
  const startedMs = isoToMs(sum.startedAt);
  const endedMs = isoToMs(sum.endedAt);

  return (
    <div className="space-y-6">
      <Card title="Session">
        <div className="grid grid-cols-1 md:grid-cols-2 gap-x-8">
          <Stat label="Session ID" value={sum.sessionId || "—"} mono />
          <Stat
            label="Outcome"
            value={outcomeNameToLabel(sum.outcome)}
          />
          <Stat label="ESN" value={formatEsnHex(sum.esn)} mono />
          <Stat label="MEID" value={formatMeidColon(sum.meid)} mono />
          <Stat
            label="Started"
            value={startedMs != null ? formatTime(startedMs) : "—"}
            mono
          />
          <Stat
            label="Ended"
            value={endedMs != null ? formatTime(endedMs) : "in progress"}
            mono
          />
        </div>
      </Card>

      <Card title={`Events (${events.length})`}>
        {events.length === 0 ? (
          <p className="text-dimmed text-sm">No events recorded.</p>
        ) : (
          <div className="space-y-3">
            {events.map((rec, i) => (
              <EventRow key={i} rec={rec} />
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}
