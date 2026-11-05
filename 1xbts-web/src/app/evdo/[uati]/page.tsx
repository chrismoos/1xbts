"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "next/navigation";
import Link from "next/link";
import { Card, Stat } from "@/components/card";
import { EvdoChip, EvdoLinkChip, EvdoVerdictChip } from "@/components/evdo";
import { HrpdAdmissionCard } from "@/components/hrpd-admission";
import { EvdoEventTable } from "@/components/evdo-events";
import { TimeSeriesChart } from "@/components/time-series-chart";
import { cellForColorCode, cellLabel, peerIdForCell } from "@/lib/cell";
import {
  a10ForEvdo,
  drcRateLabel,
  evdoIdentity,
  parseA10Sessions,
  evdoLinkState,
  evdoVerdict,
  formatAge,
  packetSessionForEvdo,
  type EvdoIdentity,
  type EvdoPacketSession,
} from "@/lib/evdo";
import {
  formatHrpdFullUati,
  hrpdRelatedUatis,
  hrpdTimestampNsToMs,
  hrpdTimestampNsToUs,
  isHrpdTelemetryTrafficEvent,
  uatiHex,
} from "@/lib/hrpd-correlation";
import { useLive } from "@/lib/live";
import { useMobileDirectory, type MobileDirectoryEntry } from "@/lib/mobile-directory";
import { AnEventRecord, GetSessionResponse, sessionStateToJSON, type Session } from "@/lib/proto/an/v1/service";
import { HrpdAccessEvent, HrpdSessionEvent, HrpdTrafficEvent } from "@/lib/proto/events/v1/an";
import { useBtsList } from "@/lib/use-bts-list";
import { useEventStream } from "@/lib/use-event-stream";
import { ErrorNotice } from "@/components/error-notice";

interface PacketSessionInfo extends EvdoPacketSession {
  phoneNumber?: string;
  mnId?: string;
  mnIdSource?: string;
}

interface TimelineRow {
  id: string;
  ts: number;
  stream: "session" | "access" | "traffic";
  event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent;
}

interface DrcSample {
  ts: number;
  drc: number;
  snrDb: number;
}

type Tab = "overview" | "identity" | "radio" | "packet" | "events";

const TABS: { key: Tab; label: string }[] = [
  { key: "overview", label: "Overview" },
  { key: "identity", label: "Identity" },
  { key: "radio", label: "Radio" },
  { key: "packet", label: "Packet data" },
  { key: "events", label: "Events" },
];

const MAX_TIMELINE = 200;
const MAX_SAMPLES = 120;
const OVERVIEW_EVENTS = 5;
const SNR_DB_PER_TENTH = 10;

function stateLabel(state: number): string {
  const name = sessionStateToJSON(state).replace(/^SESSION_STATE_/, "");
  return name.charAt(0) + name.slice(1).toLowerCase().replace(/_/g, " ");
}

function sessionFp(e: HrpdSessionEvent): string {
  return `s:${e.timestampNs}:${e.uati}:${e.reason}`;
}
function trafficFp(e: HrpdTrafficEvent): string {
  return `t:${e.timestampNs}:${e.uati}:${e.reason}:${e.drcValue}:${e.macIndex}`;
}
function accessFp(e: HrpdAccessEvent): string {
  return `a:${e.timestampNs}:${e.uati}:${e.accessSignature}:${e.reason}`;
}

function eventRelatedUatis(event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent): number[] {
  const values = [event.uati >>> 0];
  if ("receiveAti" in event && event.receiveAti) values.push(event.receiveAti >>> 0);
  if (event.fullUati?.compactUati32) values.push(event.fullUati.compactUati32 >>> 0);
  return values;
}

function isOneXMobile(mobile?: MobileDirectoryEntry): mobile is MobileDirectoryEntry {
  return Boolean(mobile && !mobile.address.startsWith("evdo-"));
}

export default function EvdoSessionPage() {
  const uatiParam = useParams<{ uati: string }>().uati;
  const uatiNum = Number.parseInt(uatiParam, 16);
  const [tab, setTab] = useState<Tab>("overview");
  const [timeline, setTimeline] = useState<TimelineRow[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [samples, setSamples] = useState<DrcSample[]>([]);
  const mobiles = useMobileDirectory();
  const { cells } = useBtsList();
  // Fingerprints of events seeded from history, consumed once if they also
  // arrive live (avoids a duplicate row at the load→stream boundary).
  const dedupRef = useRef<Set<string>>(new Set());

  const liveSession = useLive<unknown>(`/api/an-sessions/${uatiParam}`);
  const livePackets = useLive<{ sessions?: PacketSessionInfo[] }>("/api/packet-sessions");
  const liveA10 = useLive<unknown>("/api/pcf-a10-sessions");
  const a10s = useMemo(() => parseA10Sessions(liveA10.data), [liveA10.data]);
  const session: Session | null = useMemo(() => {
    const raw = liveSession.data as { session?: unknown; error?: string } | undefined;
    if (!raw || raw.error) return null;
    return GetSessionResponse.fromJSON(raw).session ?? null;
  }, [liveSession.data]);
  const sessionError =
    liveSession.errorBody ?? liveSession.error ?? (liveSession.data as { error?: string } | undefined)?.error ?? null;
  const packets = useMemo(() => livePackets.data?.sessions ?? [], [livePackets.data]);

  useEffect(() => {
    let cancelled = false;
    dedupRef.current = new Set();
    fetch(`/api/an-session-events/${uatiParam}`)
      .then((r) => r.json())
      .then((raw: { records?: unknown[]; error?: string }) => {
        if (cancelled || raw.error || !raw.records) return;
        const rows: TimelineRow[] = [];
        const seeded: DrcSample[] = [];
        for (const record of raw.records.map((value) => AnEventRecord.fromJSON(value))) {
          const tsMs = record.receivedMs;
          const id = `h-${tsMs}-${rows.length}`;
          if (record.session) {
            dedupRef.current.add(sessionFp(record.session));
            rows.push({ id, ts: tsMs * 1000, stream: "session", event: record.session });
          } else if (record.access) {
            dedupRef.current.add(accessFp(record.access));
            rows.push({ id, ts: tsMs * 1000, stream: "access", event: record.access });
          } else if (record.traffic) {
            dedupRef.current.add(trafficFp(record.traffic));
            if (isHrpdTelemetryTrafficEvent(record.traffic)) {
              seeded.push({
                ts: hrpdTimestampNsToMs(record.traffic.timestampNs) ?? tsMs,
                drc: record.traffic.drcValue,
                snrDb: record.traffic.reversePilotSnrDbTenths / SNR_DB_PER_TENTH,
              });
            } else {
              rows.push({ id, ts: tsMs * 1000, stream: "traffic", event: record.traffic });
            }
          }
        }
        // Records are oldest-first. The timeline renders newest-first.
        setTimeline(rows.reverse().slice(0, MAX_TIMELINE));
        setSamples(seeded.slice(-MAX_SAMPLES));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [uatiParam]);

  const relatedUatis = useMemo(
    () => new Set(session ? hrpdRelatedUatis(session) : [uatiNum >>> 0]),
    [session, uatiNum],
  );
  const packet = session ? packetSessionForEvdo(session, packets, a10s) : undefined;
  const a10 = session ? a10ForEvdo(session, a10s) : undefined;
  const identity = session ? evdoIdentity(session, mobiles, packet, a10) : undefined;
  const latest = samples.at(-1);

  const pushRow = useCallback((stream: TimelineRow["stream"], event: TimelineRow["event"]) => {
    const ts = hrpdTimestampNsToUs(event.timestampNs);
    setTimeline((prev) =>
      [{ id: `${ts}-${Math.random().toString(36).slice(2)}`, ts, stream, event }, ...prev].slice(0, MAX_TIMELINE),
    );
  }, []);

  const toggleExpand = useCallback((id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  useEventStream(
    "hrpd-session",
    useCallback(
      (data: string) => {
        const ev = HrpdSessionEvent.fromJSON(JSON.parse(data));
        if (!eventRelatedUatis(ev).some((value) => relatedUatis.has(value))) return;
        if (dedupRef.current.delete(sessionFp(ev))) return;
        pushRow("session", ev);
      },
      [relatedUatis, pushRow],
    ),
  );

  useEventStream(
    "hrpd-access",
    useCallback(
      (data: string) => {
        const ev = HrpdAccessEvent.fromJSON(JSON.parse(data));
        if (!eventRelatedUatis(ev).some((value) => relatedUatis.has(value))) return;
        if (dedupRef.current.delete(accessFp(ev))) return;
        pushRow("access", ev);
      },
      [relatedUatis, pushRow],
    ),
  );

  useEventStream(
    "hrpd-traffic",
    useCallback(
      (data: string) => {
        const ev = HrpdTrafficEvent.fromJSON(JSON.parse(data));
        if (!eventRelatedUatis(ev).some((value) => relatedUatis.has(value))) return;
        if (dedupRef.current.delete(trafficFp(ev))) return;
        if (isHrpdTelemetryTrafficEvent(ev)) {
          const sample = {
            ts: hrpdTimestampNsToMs(ev.timestampNs) ?? Date.now(),
            drc: ev.drcValue,
            snrDb: ev.reversePilotSnrDbTenths / SNR_DB_PER_TENTH,
          };
          setSamples((prev) => [...prev, sample].slice(-MAX_SAMPLES));
          return;
        }
        pushRow("traffic", ev);
      },
      [relatedUatis, pushRow],
    ),
  );

  const servingCell = cellForColorCode(cells, session?.fullUati?.colorCode ?? session?.colorCode);
  const servingPeerId = peerIdForCell(cells, servingCell);
  const canonicalUati = formatHrpdFullUati(session?.fullUati);
  const link = session ? evdoLinkState(session.connected, packet?.phase) : undefined;

  return (
    <div className="page-container space-y-4">
      <Link href="/evdo" className="text-sm text-accent-blue hover:underline">
        ← EV-DO Sessions
      </Link>
      <ErrorNotice error={sessionError} />

      <div className="flex flex-wrap items-start justify-between gap-4">
        <SessionHeading identity={identity} hasSession={session != null} />
        <div className="text-right">
          {session && link && (
            <div className="flex flex-wrap justify-end gap-1.5">
              <EvdoChip label={stateLabel(session.state)} className="text-accent-green bg-accent-green-bg" />
              <EvdoLinkChip state={link} />
              <EvdoVerdictChip verdict={evdoVerdict(session)} />
              {isOneXMobile(identity?.mobile) && (
                <EvdoChip label="Also on 1x" className="text-accent-blue bg-accent-blue-bg" />
              )}
            </div>
          )}
          <div className="mt-1.5 font-mono text-xs text-muted">
            UATI {uatiHex(uatiNum)}
            {session && ` · color ${session.colorCode}`}
            {servingPeerId && (
              <>
                {" · "}
                <Link href={`/bts/${encodeURIComponent(servingPeerId)}`} className="text-accent-cyan hover:underline">
                  {cellLabel(servingCell)}
                </Link>
              </>
            )}
          </div>
          {canonicalUati && <div className="font-mono text-[11px] text-dimmed">{canonicalUati}</div>}
        </div>
      </div>

      <div className="flex gap-1 border-b border-border">
        {TABS.map((t) => (
          <button
            key={t.key}
            type="button"
            onClick={() => setTab(t.key)}
            className={`-mb-px border-b-2 px-3 py-2 text-sm ${tab === t.key ? "border-accent-indigo text-primary" : "border-transparent text-muted hover:text-secondary"}`}
          >
            {t.label}
            {t.key === "events" && timeline.length > 0 && <span className="ml-1 text-dimmed">{timeline.length}</span>}
          </button>
        ))}
      </div>

      {!session && !sessionError && (
        <p className="text-sm text-muted">{liveSession.loading ? "Loading session…" : "Session not found."}</p>
      )}

      {session && identity && tab === "overview" && (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-4">
          <div className="glass-card p-4 lg:col-span-2 lg:row-span-2">
            <div className="mb-3 flex items-center justify-between">
              <h2 className="text-sm font-semibold text-primary">Radio link</h2>
              {link && <EvdoLinkChip state={link} />}
            </div>
            <RadioFigures session={session} snrDb={session.connected ? latest?.snrDb : undefined} />
            <div className="mt-3">
              <RadioChart samples={samples} height={180} />
            </div>
          </div>
          <div className="glass-card p-4 lg:col-span-2">
            <div className="mb-2 flex items-center justify-between">
              <h2 className="text-sm font-semibold text-primary">Identity</h2>
              <button type="button" onClick={() => setTab("identity")} className="text-xs text-accent-blue hover:underline">
                Details →
              </button>
            </div>
            <IdentityChips identity={identity} />
            <p className="mt-2 text-xs text-muted">{identitySummary(identity)}</p>
          </div>
          <div className="glass-card p-4">
            <h2 className="mb-2 text-sm font-semibold text-primary">Packet data</h2>
            {packet ? (
              <>
                <EvdoChip
                  label={`A10 ${packet.phase}`}
                  className={packet.phase === "active" ? "text-accent-green bg-accent-green-bg" : "text-accent-amber bg-accent-amber-bg"}
                />
                <div className="mt-1.5 font-mono text-sm">{packet.peerIp || "—"}</div>
                <Link href={`/packets/${encodeURIComponent(packet.sessionId)}`} className="text-xs text-accent-blue hover:underline">
                  Open packet session →
                </Link>
              </>
            ) : (
              <p className="text-xs text-dimmed">No packet session on this UATI.</p>
            )}
          </div>
          <div className="glass-card p-4">
            <h2 className="mb-2 text-sm font-semibold text-primary">1x</h2>
            {isOneXMobile(identity.mobile) ? (
              <>
                <EvdoChip
                  label={identity.mobile.trafficWalshCode != null ? "On a traffic channel" : (identity.mobile.state ?? "Registered")}
                  className="text-accent-cyan bg-accent-blue-bg"
                />
                {identity.mobile.trafficWalshCode != null && (
                  <div className="mt-1.5 font-mono text-sm">W{identity.mobile.trafficWalshCode}</div>
                )}
                <Link
                  href={`/mobiles/${encodeURIComponent(identity.mobile.address)}`}
                  className="text-xs text-accent-blue hover:underline"
                >
                  Open mobile →
                </Link>
              </>
            ) : (
              <p className="text-xs text-dimmed">Not registered on 1x.</p>
            )}
          </div>
          <div className="glass-card p-4 lg:col-span-4">
            <div className="mb-2 flex items-center justify-between">
              <h2 className="text-sm font-semibold text-primary">Latest events</h2>
              <button type="button" onClick={() => setTab("events")} className="text-xs text-accent-blue hover:underline">
                All events →
              </button>
            </div>
            <EvdoEventTable rows={timeline.slice(0, OVERVIEW_EVENTS)} expanded={expanded} onToggle={toggleExpand} />
          </div>
        </div>
      )}

      {session && identity && tab === "identity" && (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
          <div className="glass-card p-4 lg:col-span-2">
            <h2 className="mb-2 text-sm font-semibold text-primary">Identifiers</h2>
            <IdentityTable identity={identity} packet={packet} session={session} />
          </div>
          <HrpdAdmissionCard admission={session.admission} />
        </div>
      )}

      {session && tab === "radio" && (
        <div className="space-y-4">
          <div className="glass-card p-4">
            <RadioFigures session={session} snrDb={session.connected ? latest?.snrDb : undefined} />
          </div>
          <Card title="Reverse link telemetry">
            {samples.length >= 2 ? (
              <RadioChart samples={samples} height={260} />
            ) : (
              <p className="text-sm text-dimmed">Telemetry appears once the access terminal is on a traffic channel.</p>
            )}
          </Card>
        </div>
      )}

      {session && tab === "packet" && (
        <Card title="Packet data">
          {packet ? <PacketDetails packet={packet} /> : <p className="text-sm text-dimmed">No packet session on this UATI.</p>}
        </Card>
      )}

      {tab === "events" && (
        <div className="glass-card p-4">
          <EvdoEventTable rows={timeline} expanded={expanded} onToggle={toggleExpand} />
        </div>
      )}
    </div>
  );
}

function SessionHeading({ identity, hasSession }: { identity?: EvdoIdentity; hasSession: boolean }) {
  const mobile = identity?.mobile;
  const name = mobile?.subscriberDisplayName || mobile?.phoneNumber;
  const title = name ?? (identity?.hardwareId ? "Unknown device" : hasSession ? "Identifying…" : "EV-DO session");
  const ids = [
    identity?.hardwareId ? `${identity.hardwareId.label} ${identity.hardwareId.value}` : null,
    identity?.imsi ? `IMSI ${identity.imsi}` : null,
  ].filter((value): value is string => value != null);
  return (
    <div>
      <h1 className={`text-2xl font-semibold ${name ? "" : identity?.hardwareId ? "text-accent-amber" : "text-muted"}`}>
        {mobile && name ? (
          <Link href={`/mobiles/${encodeURIComponent(mobile.address)}`} className="hover:underline">
            {title}
          </Link>
        ) : (
          title
        )}
      </h1>
      {mobile?.phoneNumber && name !== mobile.phoneNumber && <div className="text-sm text-muted">{mobile.phoneNumber}</div>}
      {ids.map((id) => (
        <div key={id} className="font-mono text-xs text-dimmed">
          {id}
        </div>
      ))}
    </div>
  );
}

function RadioFigures({ session, snrDb }: { session: Session; snrDb?: number }) {
  const rate = session.connected ? drcRateLabel(session.drcIndex) : undefined;
  const figures: { label: string; value: string; unit?: string }[] = [
    { label: "Forward rate", value: rate ?? "—" },
    { label: "Reverse pilot SNR", value: snrDb != null ? snrDb.toFixed(1) : "—", unit: snrDb != null ? "dB" : undefined },
    { label: "MAC index", value: session.connected && session.macIndex != null ? String(session.macIndex) : "—" },
    { label: "Last activity", value: formatAge(session.lastActivityAtMs) },
  ];
  return (
    <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
      {figures.map((figure) => (
        <div key={figure.label}>
          <div className="text-xs text-muted">{figure.label}</div>
          <div className="font-mono text-lg whitespace-nowrap text-primary">
            {figure.value}
            {figure.unit && <span className="ml-1 text-sm text-muted">{figure.unit}</span>}
          </div>
        </div>
      ))}
    </div>
  );
}

function RadioChart({ samples, height }: { samples: DrcSample[]; height: number }) {
  if (samples.length < 2) {
    return <p className="text-xs text-dimmed">No telemetry yet. It appears once the access terminal is on a traffic channel.</p>;
  }
  return (
    <TimeSeriesChart
      title="DRC and reverse pilot SNR"
      width={900}
      height={height}
      yLabel="DRC / dB"
      timestamps={samples.map((s) => s.ts)}
      series={[
        { key: "drc", label: "DRC", color: "var(--accent-indigo)", values: samples.map((s) => s.drc) },
        { key: "snr", label: "Pilot SNR", color: "var(--accent-green)", values: samples.map((s) => s.snrDb) },
      ]}
    />
  );
}

function IdentityChips({ identity }: { identity: EvdoIdentity }) {
  const name = identity.mobile?.subscriberDisplayName || identity.mobile?.phoneNumber;
  return (
    <div className="flex flex-wrap gap-1.5">
      {identity.hardwareId && (
        <EvdoChip label={`${identity.hardwareId.label} ${identity.hardwareId.value}`} className="text-accent-purple bg-accent-purple-bg" />
      )}
      {name && <EvdoChip label={name} className="text-accent-green bg-accent-green-bg" />}
      {identity.imsi && <EvdoChip label={`IMSI ${identity.imsi}`} className="text-accent-green bg-accent-green-bg" />}
      {identity.nai && <EvdoChip label={`NAI ${identity.nai}`} className="text-muted bg-surface-raised" />}
      {!identity.hardwareId && !identity.imsi && <span className="text-xs text-dimmed">No identifier reported yet.</span>}
    </div>
  );
}

function identitySummary(identity: EvdoIdentity): string {
  if (!identity.mobile) {
    return identity.hardwareId
      ? "The Hardware ID matches no subscriber or mobile."
      : "Identity is known once the access terminal answers the Hardware ID request.";
  }
  const onOneX = isOneXMobile(identity.mobile) ? " The same device is registered on 1x." : "";
  return `Matched by ${identity.matchedBy.join(", ")}.${onOneX}`;
}

function IdentityTable({
  identity,
  packet,
  session,
}: {
  identity: EvdoIdentity;
  packet?: PacketSessionInfo;
  session: Session;
}) {
  const mobile = identity.mobile;
  const matched = (kind: string) => identity.matchedBy.includes(kind);
  const imsiSource = session.admission?.accessAuthImsi ? "A12 Access-Accept" : "Packet session";
  const imsiAgreement = !identity.imsi
    ? null
    : mobile?.imsi
      ? mobile.imsi === identity.imsi
        ? { label: "agrees with mobile", ok: true }
        : { label: "differs from mobile", ok: false }
      : { label: "reported", ok: null };
  const rows: { name: string; value: string; source: string; status?: { label: string; ok: boolean | null } }[] = [];
  if (identity.hardwareId) {
    rows.push({
      name: "Hardware ID",
      value: `${identity.hardwareId.label} ${identity.hardwareId.value}`,
      source: "Hardware ID Response",
      status:
        matched("MEID") || matched("ESN")
          ? { label: "matched", ok: true }
          : evdoVerdict(session) === "refused"
            ? { label: "not provisioned", ok: false }
            : { label: "reported", ok: null },
    });
  }
  if (identity.subscriberId) {
    rows.push({
      name: "Subscriber",
      value: mobile?.subscriberDisplayName || mobile?.phoneNumber || identity.subscriberId,
      source: "HLR, by Hardware ID",
      status: matched("subscriber") ? { label: "matched", ok: true } : { label: "reported", ok: null },
    });
  }
  if (identity.imsi) {
    rows.push({ name: "IMSI", value: identity.imsi, source: imsiSource, status: imsiAgreement ?? undefined });
  }
  if (identity.nai) {
    rows.push({ name: "NAI", value: identity.nai, source: "CHAP Response", status: { label: "reported", ok: null } });
  }
  if (isOneXMobile(mobile)) {
    rows.push({
      name: "1x mobile",
      value: mobile.address,
      source: "BSC registration",
      status: { label: `same ${identity.matchedBy.join(" and ")}`, ok: true },
    });
  }
  if (packet) {
    rows.push({ name: "Packet session", value: packet.sessionId, source: "PCF, by UATI", status: { label: "matched", ok: true } });
  }
  if (rows.length === 0) {
    return <p className="text-sm text-dimmed">The access terminal has not reported any identifier yet.</p>;
  }
  return (
    <table className="data-table">
      <thead>
        <tr>
          <th>Identifier</th>
          <th>Value</th>
          <th>Source</th>
          <th>Correlation</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.name} className="align-top">
            <td className="text-muted">{row.name}</td>
            <td className="font-mono text-primary break-all">{row.value}</td>
            <td className="text-muted">{row.source}</td>
            <td>
              {row.status && (
                <EvdoChip
                  label={row.status.label}
                  className={
                    row.status.ok === true
                      ? "text-accent-green bg-accent-green-bg"
                      : row.status.ok === false
                        ? "text-accent-red bg-accent-red-bg"
                        : "text-muted bg-surface-raised"
                  }
                />
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function PacketDetails({ packet }: { packet: PacketSessionInfo }) {
  const imsi = packet.subscriberImsi || packet.imsi;
  return (
    <div className="grid grid-cols-1 gap-x-8 sm:grid-cols-2">
      <div>
        <Stat label="Session" value={packet.sessionId} mono />
        <Stat label="State" value={packet.phase} />
        <Stat label="A10 key" value={packet.trafficWalshCode ? String(packet.trafficWalshCode) : "—"} mono />
        <Stat label="Mobile IP" value={packet.peerIp || "—"} mono />
      </div>
      <div>
        {packet.phoneNumber && <Stat label="Phone" value={packet.phoneNumber} mono />}
        {imsi && <Stat label="Subscriber IMSI" value={imsi} mono />}
        {packet.meid && <Stat label="MEID" value={packet.meid} mono />}
        {packet.esn ? <Stat label="ESN" value={`0x${packet.esn.toString(16).toUpperCase().padStart(8, "0")}`} mono /> : null}
        {packet.mnId && <Stat label="MN ID" value={`${packet.mnId}${packet.mnIdSource ? ` (${packet.mnIdSource})` : ""}`} mono />}
        <Link href={`/packets/${encodeURIComponent(packet.sessionId)}`} className="mt-1 inline-block text-xs text-accent-blue hover:underline">
          Open packet session →
        </Link>
      </div>
    </div>
  );
}
