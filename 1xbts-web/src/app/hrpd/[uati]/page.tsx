"use client";

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useParams } from "next/navigation";
import Link from "next/link";
import { Card } from "@/components/card";
import {
  AnEventRecord,
  GetSessionResponse,
  sessionStateToJSON,
  type HrpdHardwareIdResponse,
  type Session,
} from "@/lib/proto/an/v1/service";
import { useEventStream } from "@/lib/use-event-stream";
import {
  HrpdAccessDetail,
  HrpdSessionDetail,
  HrpdTrafficDetail,
  formatHrpdAccessSummary,
  formatHrpdSessionSummary,
  formatHrpdTrafficSummary,
  hrpdDirectionClass,
  hrpdDirectionLabel,
} from "@/components/message-detail";
import {
  HrpdAccessEvent,
  HrpdSessionEvent,
  HrpdTrafficEvent,
} from "@/lib/proto/events/v1/an";
import { formatTime } from "@/lib/message-log";
import { TimeSeriesChart } from "@/components/time-series-chart";
import {
  formatHrpdFullUati,
  hrpdTimestampNsToMs,
  hrpdTimestampNsToUs,
  hrpdReceiveUati,
  hrpdRelatedUatis,
  hrpdSessionMatchesPacket,
  isHrpdTelemetryTrafficEvent,
  uatiHex,
} from "@/lib/hrpd-correlation";
import {
  mobileForPacketSession,
  mobileLabel,
  useMobileDirectory,
} from "@/lib/mobile-directory";

interface SessionResponse {
  session?: Session;
  error?: string;
}

interface PacketSessionInfo {
  sessionId: string;
  phase: string;
  accessTechnology: string;
  mobileAddress: string;
  subscriberId: string;
  phoneNumber: string;
  imsi: string;
  meid: string;
  hrpdMnId: string;
  hrpdMnIdSource: string;
  subscriberImsi: string;
  esn: number;
  trafficWalshCode: number;
  serviceOption: number;
  peerIp: string;
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

const MAX_TIMELINE = 200;
const MAX_SAMPLES = 120;

function stateLabel(state: number): string {
  return sessionStateToJSON(state).replace(/^SESSION_STATE_/, "");
}

function bytesToArray(value: unknown): number[] {
  if (value instanceof Uint8Array) return Array.from(value);
  if (Array.isArray(value)) return value.filter((byte) => typeof byte === "number");
  if (typeof value === "string") {
    try {
      return Array.from(atob(value), (char) => char.charCodeAt(0));
    } catch {
      return [];
    }
  }
  if (value && typeof value === "object") {
    const maybeBuffer = value as { data?: unknown; length?: unknown };
    if (Array.isArray(maybeBuffer.data)) {
      return maybeBuffer.data.filter((byte) => typeof byte === "number");
    }
    if (typeof maybeBuffer.length === "number") {
      const bytes: number[] = [];
      for (let i = 0; i < maybeBuffer.length; i += 1) {
        const byte = (value as Record<number, unknown>)[i];
        if (typeof byte === "number") bytes.push(byte);
      }
      return bytes;
    }
  }
  return [];
}

function bytesToHex(value: unknown): string {
  return bytesToArray(value)
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

function hardwareIdDisplay(
  response?: HrpdHardwareIdResponse,
): { label: string; value: string } | null {
  if (!response) return null;
  const bytes = bytesToArray(response.hardwareIdValue);
  const hex = bytesToHex(bytes).toUpperCase();
  if (response.hardwareIdType === 0x00ffff && bytes.length === 7) {
    return { label: "MEID", value: hex };
  }
  if (response.hardwareIdType === 0x010000 && bytes.length === 4) {
    return { label: "ESN", value: `0x${hex}` };
  }
  return {
    label: `Hardware ID 0x${response.hardwareIdType.toString(16).toUpperCase().padStart(6, "0")}`,
    value: hex || "-",
  };
}

// Boundary-dedup fingerprints: a live event whose fingerprint was just loaded
// from history is the same event re-arriving on the stream, so skip it once.
function sessionFp(e: HrpdSessionEvent): string {
  return `s:${e.timestampNs}:${e.uati}:${e.reason}`;
}
function trafficFp(e: HrpdTrafficEvent): string {
  return `t:${e.timestampNs}:${e.uati}:${e.reason}:${e.drcValue}:${e.macIndex}`;
}
function accessFp(e: HrpdAccessEvent): string {
  return `a:${e.timestampNs}:${e.uati}:${e.accessSignature}:${e.reason}`;
}

function hrpdEventTimestampUs(event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent): number {
  return hrpdTimestampNsToUs(event.timestampNs);
}

function hrpdEventTimestampMs(
  event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent,
  fallbackMs?: number,
): number {
  return hrpdTimestampNsToMs(event.timestampNs) ?? fallbackMs ?? Date.now();
}

function eventRelatedUatis(event: HrpdSessionEvent | HrpdAccessEvent | HrpdTrafficEvent): number[] {
  const values = [event.uati >>> 0];
  if ("receiveAti" in event && event.receiveAti) values.push(event.receiveAti >>> 0);
  if (event.fullUati?.compactUati32) values.push(event.fullUati.compactUati32 >>> 0);
  return values;
}

export default function HrpdSessionPage() {
  const uatiParam = useParams<{ uati: string }>().uati;
  const uatiNum = Number.parseInt(uatiParam, 16);

  const [session, setSession] = useState<Session | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [timeline, setTimeline] = useState<TimelineRow[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [samples, setSamples] = useState<DrcSample[]>([]);
  const [packetSessions, setPacketSessions] = useState<PacketSessionInfo[]>([]);
  const [latest, setLatest] = useState<{ drc: number; snrDb: number } | null>(
    null,
  );
  const mobiles = useMobileDirectory();
  // Fingerprints of events seeded from history, consumed once if they also
  // arrive live (avoids a duplicate row at the load→stream boundary).
  const dedupRef = useRef<Set<string>>(new Set());

  // Load recent event history once on mount, then stream live on top.
  useEffect(() => {
    let cancelled = false;
    dedupRef.current = new Set();
    fetch(`/api/an-session-events/${uatiParam}`)
      .then((r) => r.json())
      .then((raw: { records?: unknown[]; error?: string }) => {
        if (cancelled || raw.error || !raw.records) return;
        const records = raw.records.map((record) => AnEventRecord.fromJSON(record));
        const rows: TimelineRow[] = [];
        const seededSamples: DrcSample[] = [];
        let seededLatest: { drc: number; snrDb: number } | null = null;
        for (const rec of records) {
          const tsMs = rec.receivedMs;
          if (rec.session) {
            dedupRef.current.add(sessionFp(rec.session));
            rows.push({
              id: `h-${tsMs}-${rows.length}`,
              ts: tsMs * 1000,
              stream: "session",
              event: rec.session,
            });
          } else if (rec.access) {
            dedupRef.current.add(accessFp(rec.access));
            rows.push({
              id: `h-${tsMs}-${rows.length}`,
              ts: tsMs * 1000,
              stream: "access",
              event: rec.access,
            });
          } else if (rec.traffic) {
            dedupRef.current.add(trafficFp(rec.traffic));
            if (isHrpdTelemetryTrafficEvent(rec.traffic)) {
              const snrDb = rec.traffic.reversePilotSnrDbTenths / 10;
              const sampleTs = hrpdEventTimestampMs(rec.traffic, tsMs);
              seededSamples.push({
                ts: sampleTs,
                drc: rec.traffic.drcValue,
                snrDb,
              });
              seededLatest = { drc: rec.traffic.drcValue, snrDb };
            } else {
              rows.push({
                id: `h-${tsMs}-${rows.length}`,
                ts: tsMs * 1000,
                stream: "traffic",
                event: rec.traffic,
              });
            }
          }
        }
        // Records are oldest-first; the timeline renders newest-first.
        setTimeline(rows.reverse().slice(0, MAX_TIMELINE));
        setSamples(seededSamples.slice(-MAX_SAMPLES));
        if (seededLatest) setLatest(seededLatest);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [uatiParam]);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      fetch(`/api/an-sessions/${uatiParam}`)
        .then((r) => r.json())
        .then((raw: SessionResponse) => {
          if (cancelled) return;
          if (raw.error) {
            setError(raw.error);
          } else {
            const data = GetSessionResponse.fromJSON(raw);
            setError(null);
            setSession(data.session ?? null);
          }
        })
        .catch((e) => {
          if (!cancelled) setError(String(e));
        });
      fetch("/api/packet-sessions")
        .then((r) => r.json())
        .then((data: { sessions?: PacketSessionInfo[]; error?: string }) => {
          if (cancelled || data.error) return;
          setPacketSessions((data.sessions ?? []).filter((s) => s.accessTechnology === "HRPD"));
        })
        .catch(() => {});
    };
    tick();
    const id = setInterval(tick, 2000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [uatiParam]);

  const relatedUatis = useMemo(
    () => new Set(session ? hrpdRelatedUatis(session) : [uatiNum >>> 0]),
    [session, uatiNum],
  );

  const packetSession = useMemo(
    () =>
      session
        ? packetSessions.find((candidate) => hrpdSessionMatchesPacket(session, candidate))
        : packetSessions.find((candidate) => {
            const key = candidate.trafficWalshCode >>> 0;
            return key === (uatiNum >>> 0) || (key & 0x00ff_ffff) === (uatiNum & 0x00ff_ffff);
          }),
    [packetSessions, session, uatiNum],
  );
  const mobile = packetSession ? mobileForPacketSession(packetSession, mobiles) : undefined;
  const mobileLinkLabel = mobile ? mobileLabel(mobile) : undefined;
  const packetSubscriberImsi =
    packetSession?.subscriberImsi || packetSession?.imsi || "";
  const sessionHardwareId = hardwareIdDisplay(session?.hardwareIdResponse);
  const trafficUati = session
    ? hrpdReceiveUati(
        session.fullUati?.compactUati32 || session.uati,
        session.fullUati?.colorCode ?? session.colorCode,
      )
    : undefined;
  const canonicalUati = formatHrpdFullUati(session?.fullUati);

  const pushRow = useCallback((
    stream: TimelineRow["stream"],
    event: TimelineRow["event"],
  ) => {
    const ts = hrpdEventTimestampUs(event);
    setTimeline((prev) =>
      [
        { id: `${ts}-${Math.random().toString(36).slice(2)}`, ts, stream, event },
        ...prev,
      ].slice(0, MAX_TIMELINE),
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
          const snrDb = ev.reversePilotSnrDbTenths / 10;
          const sampleTs = hrpdEventTimestampMs(ev);
          setLatest({ drc: ev.drcValue, snrDb });
          setSamples((prev) =>
            [...prev, { ts: sampleTs, drc: ev.drcValue, snrDb }].slice(
              -MAX_SAMPLES,
            ),
          );
          return;
        }
        pushRow("traffic", ev);
      },
      [relatedUatis, pushRow],
    ),
  );

  const p = session?.protocols;

  return (
    <div className="p-6 space-y-4">
      <div className="flex items-center gap-3">
        <Link href="/hrpd" className="text-accent-blue hover:underline text-sm">
          ← Sessions
        </Link>
        <h1 className="text-2xl font-semibold font-mono">
          {canonicalUati ?? uatiHex(uatiNum)}
        </h1>
      </div>
      {error && <div className="text-accent-red text-sm">AN service: {error}</div>}

      <Card title="Session">
        {session ? (
          <div className="grid grid-cols-2 gap-x-8 gap-y-1 text-sm font-mono">
            {canonicalUati && (
              <div className="text-muted col-span-2">
                UATI <span className="text-primary">{canonicalUati}</span>
              </div>
            )}
            <div className="text-muted">
              Session Key{" "}
              <span className="text-primary">
                {uatiHex(session.uati)}
              </span>
            </div>
            {trafficUati !== undefined && trafficUati !== (session.uati >>> 0) && (
              <div className="text-muted">
                Receive ATI{" "}
                <span className="text-primary">
                  {uatiHex(trafficUati)}
                </span>
              </div>
            )}
            <div className="text-muted">
              State <span className="text-primary">{stateLabel(session.state)}</span>
            </div>
            <div className="text-muted">
              Color <span className="text-primary">{session.colorCode}</span>
            </div>
            {sessionHardwareId && (
              <div className="text-muted">
                {sessionHardwareId.label}{" "}
                <span className="text-primary">{sessionHardwareId.value}</span>
              </div>
            )}
            {p && (
              <>
                <div className="text-muted">
                  Air-Link Mgmt{" "}
                  <span className="text-primary">{p.airLinkManagement}</span>
                </div>
                <div className="text-muted">
                  Session Mgmt{" "}
                  <span className="text-primary">{p.sessionManagement}</span>
                </div>
                <div className="text-muted">
                  Address Mgmt{" "}
                  <span className="text-primary">{p.addressManagement}</span>
                </div>
                <div className="text-muted">
                  Connection Layer{" "}
                  <span className="text-primary">{p.connectionLayer}</span>
                </div>
                <div className="text-muted">
                  Security <span className="text-primary">{p.security}</span>
                </div>
                <div className="text-muted">
                  MAC <span className="text-primary">{p.mac}</span>
                </div>
                <div className="text-muted">
                  Physical Layer{" "}
                  <span className="text-primary">{p.physicalLayer}</span>
                </div>
              </>
            )}
          </div>
        ) : (
          <p className="text-muted text-sm">Session not found.</p>
        )}
      </Card>

      {packetSession && (
        <Card title="Packet Data">
          <div className="grid grid-cols-2 gap-x-8 gap-y-1 text-sm">
            <div className="text-muted">
              Session{" "}
              <Link
                href={`/packets/${encodeURIComponent(packetSession.sessionId)}`}
                className="font-mono text-accent-green hover:underline"
              >
                {packetSession.sessionId}
              </Link>
            </div>
            <div className="text-muted">
              State <span className="text-primary">{packetSession.phase}</span>
            </div>
            <div className="text-muted">
              A10 <span className="font-mono text-primary">{packetSession.trafficWalshCode || "-"}</span>
            </div>
            <div className="text-muted">
              Mobile IP <span className="font-mono text-primary">{packetSession.peerIp || "-"}</span>
            </div>
            {packetSession.phoneNumber && (
              <div className="text-muted">
                Phone{" "}
                <span className="font-mono text-primary">{packetSession.phoneNumber}</span>
              </div>
            )}
            {packetSession.subscriberId && (
              <div className="text-muted">
                Subscriber{" "}
                <span className="font-mono text-primary">{packetSession.subscriberId}</span>
              </div>
            )}
            {packetSubscriberImsi && (
              <div className="text-muted">
                Subscriber IMSI{" "}
                <span className="font-mono text-primary">{packetSubscriberImsi}</span>
              </div>
            )}
            {packetSession.meid && (
              <div className="text-muted">
                MEID <span className="font-mono text-primary">{packetSession.meid}</span>
              </div>
            )}
            {packetSession.esn !== 0 && (
              <div className="text-muted">
                ESN{" "}
                <span className="font-mono text-primary">
                  0x{packetSession.esn.toString(16).toUpperCase().padStart(8, "0")}
                </span>
              </div>
            )}
            {packetSession.hrpdMnId && (
              <div className="text-muted">
                HRPD MN ID{" "}
                <span className="font-mono text-primary">
                  {packetSession.hrpdMnId}
                  {packetSession.hrpdMnIdSource ? ` (${packetSession.hrpdMnIdSource})` : ""}
                </span>
              </div>
            )}
            {mobile && mobileLinkLabel && (
              <div className="text-muted">
                Mobile{" "}
                <Link
                  href={`/mobiles/${encodeURIComponent(mobile.address)}`}
                  className="font-mono text-accent-cyan hover:underline"
                >
                  {mobileLinkLabel.value}
                </Link>
              </div>
            )}
          </div>
        </Card>
      )}

      <Card title="Reverse Link">
        {latest ? (
          <div className="space-y-3">
            <div className="flex flex-wrap gap-x-8 gap-y-1 text-sm font-mono">
              <span className="text-muted">
                DRC index <span className="text-primary">{latest.drc}</span>
              </span>
              <span className="text-muted">
                reverse pilot SNR{" "}
                <span className="text-primary">
                  {latest.snrDb.toFixed(1)} dB
                </span>
              </span>
              <span className="text-muted">
                samples <span className="text-primary">{samples.length}</span>
              </span>
            </div>
            {samples.length >= 2 && (
              <div className="max-w-[900px]">
                <TimeSeriesChart
                  title="Reverse link telemetry"
                  width={900}
                  height={260}
                  yLabel="DRC / dB"
                  timestamps={samples.map((s) => s.ts)}
                  series={[
                    {
                      key: "drc",
                      label: "DRC",
                      color: "var(--accent-indigo)",
                      values: samples.map((s) => s.drc),
                    },
                    {
                      key: "snr",
                      label: "Pilot SNR",
                      color: "var(--accent-green)",
                      values: samples.map((s) => s.snrDb),
                    },
                  ]}
                />
              </div>
            )}
          </div>
        ) : (
          <p className="text-dimmed text-sm">
            No DRC reported yet. Reverse-link telemetry appears once the AT is on
            a traffic channel.
          </p>
        )}
      </Card>

      <Card title={`Live Events (${timeline.length})`}>
        {timeline.length === 0 ? (
          <p className="text-dimmed text-sm">Waiting for events...</p>
        ) : (
          <div className="space-y-0">
            {timeline.map((row) => {
              const isExpanded = expanded.has(row.id);
              let typeName: string;
              let summary: string;
              let detail: ReactNode;
              if (row.stream === "session") {
                const event = row.event as HrpdSessionEvent;
                typeName = "HRPD Session";
                summary = formatHrpdSessionSummary(event);
                detail = <HrpdSessionDetail event={event} />;
              } else if (row.stream === "access") {
                const event = row.event as HrpdAccessEvent;
                typeName = "HRPD Access";
                summary = formatHrpdAccessSummary(event);
                detail = <HrpdAccessDetail event={event} />;
              } else {
                const event = row.event as HrpdTrafficEvent;
                typeName = "HRPD Traffic";
                summary = formatHrpdTrafficSummary(event);
                detail = <HrpdTrafficDetail event={event} />;
              }
              const direction =
                "direction" in row.event ? row.event.direction : undefined;
              return (
                <div
                  key={row.id}
                  className="border-b border-border-subtle py-1.5 px-2 -mx-2 rounded"
                >
                  <div
                    className="flex items-center gap-2 text-sm cursor-pointer hover:bg-hover rounded"
                    onClick={() => toggleExpand(row.id)}
                  >
                    <div className="flex-1 min-w-0 flex items-center gap-2">
                      <span className="text-muted font-mono text-xs w-[15rem] shrink-0">
                        {formatTime(row.ts)}
                      </span>
                      <span className={`${hrpdDirectionClass(direction)} font-mono text-xs w-16 shrink-0`}>
                        {direction != null ? hrpdDirectionLabel(direction) : "EVDO"}
                      </span>
                      <span className="text-primary font-medium shrink-0">{typeName}</span>
                      <span className="text-muted text-xs truncate">{summary}</span>
                    </div>
                    <div className="shrink-0 flex items-center gap-1">
                      <span className="text-dimmed text-xs">{isExpanded ? "▾" : "▸"}</span>
                    </div>
                  </div>
                  {isExpanded && (
                    <div className="mt-1.5 ml-8 pb-1">{detail}</div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </Card>
    </div>
  );
}
