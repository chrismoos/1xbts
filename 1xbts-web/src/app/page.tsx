"use client";

import { useEffect, useState, useCallback } from "react";
import Link from "next/link";
import { Card, Stat } from "@/components/card";
import { HrpdSummaryCard } from "@/components/hrpd-summary-card";
import { serviceOptionName } from "@/lib/service-option";
import { smsStateColor } from "@/lib/sms-state";
import { useEventStream } from "@/lib/use-event-stream";
import {
  formatAccessTypeName,
  formatAccessSummary,
  formatAccessChannel,
  formatPagingSummary,
  formatPagingChannel,
  formatTrafficSummary,
  formatTrafficChannel,
  shouldHideAccessEvent,
} from "@/components/message-detail";
import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";

// ─── Types ──────────────────────────────────────────────────────

interface SystemStatus {
  running: boolean;
  sid: number;
  nid: number;
  baseId: number;
  pilotPn: number;
  regZone: number;
}

interface RadioMetrics {
  tx?: { rtRatio: number; blocksTransmitted: number; syncFragmentsSent: number; pagingFragmentsSent: number };
  rx?: { rtRatio: number; deficitMs?: number };
}

interface MobileInfo {
  address: string;
  state: string;
  phoneNumber?: string;
  subscriberId?: string;
  trafficWalshCode?: number;
  trafficServiceOption?: number;
  snrDb?: number;
  rxPowerDbm?: number;
  rxLevelDbfs?: number;
}

interface ChannelEntry {
  walshCode?: number;
  channelType: string;
  direction: string;
  serviceOption?: number;
  mobile?: { address: string; state: string; phoneNumber?: string };
}

interface ChannelsResponse {
  channels: ChannelEntry[];
  totalWalshCodes: number;
}

interface SmsSubmission {
  smsId: string;
  destinationNumber: string;
  text: string;
  state: string;
  createdAt?: string;
}

interface PacketSession {
  sessionId: string;
  phase: string;
  serviceOption: number;
  peerIp: string;
  ourIp: string;
  mobileAddress: string;
  phoneNumber: string;
  accessTechnology: string;
  captureEnabled: boolean;
  uplinkFrames: number;
  downlinkFrames: number;
  uplinkBytes: number;
  downlinkBytes: number;
}

type RecentEvent =
  | { kind: "paging"; ts: number; summary: string; channel: string }
  | { kind: "traffic"; ts: number; summary: string; channel: string }
  | { kind: "access"; ts: number; summary: string; channel: string };

// ─── Helpers ────────────────────────────────────────────────────

function formatTime(ts: number): string {
  const d = new Date(ts);
  const h = d.getHours().toString().padStart(2, "0");
  const m = d.getMinutes().toString().padStart(2, "0");
  const s = d.getSeconds().toString().padStart(2, "0");
  return `${h}:${m}:${s}`;
}

function formatIsoTime(value?: string): string {
  if (!value) return "-";
  const ts = Date.parse(value);
  return Number.isFinite(ts) ? formatTime(ts) : "-";
}

function stateColor(state: string): string {
  switch (state) {
    case "Registered": return "bg-badge-green-bg text-badge-green-text";
    case "Paged": return "bg-badge-yellow-bg text-badge-yellow-text";
    case "TrafficAssigning": case "TrafficActive": return "bg-badge-purple-bg text-badge-purple-text";
    default: return "bg-badge-blue-bg text-badge-blue-text";
  }
}

function packetPhaseLabel(phase: string): string {
  switch (phase) {
    case "rlp_sync":
      return "RLP Sync";
    case "lcp":
      return "LCP";
    case "ipcp":
      return "IPCP";
    case "active":
      return "Active";
    default:
      return phase;
  }
}

const MAX_RECENT = 8;

// ─── Page ───────────────────────────────────────────────────────

export default function DashboardPage() {
  const [status, setStatus] = useState<SystemStatus | null>(null);
  const [metrics, setMetrics] = useState<RadioMetrics | null>(null);
  const [mobiles, setMobiles] = useState<MobileInfo[]>([]);
  const [channelData, setChannelData] = useState<ChannelsResponse | null>(null);
  const [smsRecent, setSmsRecent] = useState<SmsSubmission[]>([]);
  const [packetSessions, setPacketSessions] = useState<PacketSession[]>([]);
  const [recentEvents, setRecentEvents] = useState<RecentEvent[]>([]);


  // Fetch static system status once
  useEffect(() => {
    fetch("/api/system-status")
      .then((r) => r.json())
      .then((data) => { if (!data.error) setStatus(data); })
      .catch(() => {});
  }, []);

  // Stream live radio metrics
  useEventStream("radio-metrics", (data) => {
    const parsed = JSON.parse(data);
    if (!parsed.error) setMetrics(parsed);
  });

  // Poll mobiles, channels, sms
  const fetchAll = useCallback(async () => {
    const [mobilesRes, channelsRes, smsRes, packetRes] = await Promise.allSettled([
      fetch("/api/mobiles").then((r) => r.json()),
      fetch("/api/channels").then((r) => r.json()),
      fetch("/api/sms-history?limit=5").then((r) => r.json()),
      fetch("/api/packet-sessions").then((r) => r.json()),
    ]);
    if (mobilesRes.status === "fulfilled" && !mobilesRes.value.error) {
      setMobiles(mobilesRes.value);
    }
    if (channelsRes.status === "fulfilled" && !channelsRes.value.error) {
      setChannelData(channelsRes.value);
    }
    if (smsRes.status === "fulfilled" && !smsRes.value.error) {
      setSmsRecent(smsRes.value.submissions || []);
    }
    if (packetRes.status === "fulfilled" && !packetRes.value.error) {
      setPacketSessions(packetRes.value.sessions || []);
    }
  }, []);

  useEffect(() => {
    const initialFetch = setTimeout(() => {
      void fetchAll();
    }, 0);
    const interval = setInterval(() => {
      void fetchAll();
    }, 5000);
    return () => {
      clearTimeout(initialFetch);
      clearInterval(interval);
    };
  }, [fetchAll]);

  // Collect recent air-interface events
  const addRecent = useCallback((event: RecentEvent) => {
    setRecentEvents((prev) => [event, ...prev].slice(0, MAX_RECENT));
  }, []);

  useEventStream("paging", useCallback((data: string) => {
    const ev: PagingEvent = JSON.parse(data);
    const name = ev.header?.msgTypeName ?? "Paging";
    const detail = formatPagingSummary(ev);
    addRecent({
      kind: "paging",
      ts: ev.timestampUs ? Math.floor(ev.timestampUs / 1000) : Date.now(),
      summary: detail ? `${name} - ${detail}` : name,
      channel: formatPagingChannel(ev),
    });
  }, [addRecent]));

  useEventStream("traffic", useCallback((data: string) => {
    const ev: TrafficEvent = JSON.parse(data);
    const name = ev.header?.msgTypeName ?? "Traffic";
    const detail = formatTrafficSummary(ev);
    addRecent({
      kind: "traffic",
      ts: ev.timestampUs ? Math.floor(ev.timestampUs / 1000) : Date.now(),
      summary: detail ? `${name} - ${detail}` : name,
      channel: formatTrafficChannel(ev),
    });
  }, [addRecent]));

  useEventStream("access", useCallback((data: string) => {
    const ev: AccessEvent = JSON.parse(data);
    if (shouldHideAccessEvent(ev)) return;
    addRecent({
      kind: "access",
      ts: ev.timestampUs ? Math.floor(ev.timestampUs / 1000) : Date.now(),
      summary: `${formatAccessTypeName(ev)} ${formatAccessSummary(ev)}`,
      channel: formatAccessChannel(ev),
    });
  }, [addRecent]));

  const trafficChannels = channelData?.channels.filter((c) => c.channelType === "traffic") ?? [];
  const overheadCount = channelData?.channels.filter((c) => c.direction === "forward" && c.channelType !== "traffic").length ?? 0;
  const accessCount = channelData?.channels.filter((c) => c.channelType === "access").length ?? 0;
  const idleWalsh = (channelData?.totalWalshCodes ?? 0) - overheadCount - trafficChannels.length;

  const stateGroups = mobiles.reduce<Record<string, number>>((acc, m) => {
    acc[m.state] = (acc[m.state] || 0) + 1;
    return acc;
  }, {});

  return (
    <div className="max-w-7xl mx-auto space-y-4">
      <h1 className="text-lg font-bold">Dashboard</h1>

      {/* Top row: System + Radio */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <Card title="System Identity">
          {status ? (
            <>
              <Stat label="Status" value={status.running ? "Running" : "Stopped"} />
              <Stat label="SID / NID" value={`${status.sid} / ${status.nid}`} />
              <Stat label="BASE_ID" value={String(status.baseId)} />
              <Stat label="PILOT_PN" value={String(status.pilotPn)} />
              <Stat label="REG_ZONE" value={String(status.regZone)} />
            </>
          ) : (
            <p className="text-dimmed text-sm">Unavailable</p>
          )}
        </Card>

        <Card title="Radio Health">
          {metrics ? (
            <>
              <div className="flex items-center gap-2 mb-2">
                <span className={`inline-block w-2 h-2 rounded-full ${
                  (metrics.tx?.rtRatio ?? 0) >= 0.95 && (metrics.rx?.rtRatio ?? 0) >= 0.95
                    ? "bg-accent-green"
                    : (metrics.tx?.rtRatio ?? 0) >= 0.8 && (metrics.rx?.rtRatio ?? 0) >= 0.8
                      ? "bg-accent-amber"
                      : "bg-accent-red"
                }`} />
                <span className="text-xs text-muted">
                  TX {(metrics.tx?.rtRatio ?? 0).toFixed(1)}x / RX {(metrics.rx?.rtRatio ?? 0).toFixed(1)}x real-time
                </span>
              </div>
              {metrics.tx?.rtRatio != null && (
                <Stat label="TX RT Ratio" value={`${metrics.tx.rtRatio.toFixed(3)}x`} mono />
              )}
              {metrics.rx?.rtRatio != null && (
                <Stat label="RX RT Ratio" value={`${metrics.rx.rtRatio.toFixed(3)}x`} mono />
              )}
              {metrics.rx?.deficitMs != null && (
                <Stat label="RX Deficit" value={`${metrics.rx.deficitMs.toFixed(1)} ms`} mono />
              )}
            </>
          ) : (
            <p className="text-dimmed text-sm">No radio metrics</p>
          )}
          <div className="mt-2 pt-2 border-t border-border">
            <Link href="/radio" className="text-xs text-accent-green hover:text-accent-green">
              Detailed radio metrics &rarr;
            </Link>
          </div>
        </Card>
      </div>

      {/* Middle row: Channels + Mobiles + SMSC + Packet Data */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <HrpdSummaryCard />
        <Card title="Channels">
          {channelData ? (
            <>
              <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted mb-3">
                <span>{overheadCount} overhead</span>
                <span>{trafficChannels.length} traffic</span>
                <span>{accessCount} access</span>
                <span>{idleWalsh} idle</span>
              </div>
              {trafficChannels.length > 0 && (
                <div className="space-y-1.5">
                  {trafficChannels.map((ch, i) => (
                    <div key={i} className="flex items-center gap-2 text-xs">
                      <span className="font-mono text-primary">W{ch.walshCode}</span>
                      {ch.serviceOption != null && (
                        <span className="text-muted">{serviceOptionName(ch.serviceOption)}</span>
                      )}
                      {ch.mobile && (
                        <Link
                          href={`/mobiles/${encodeURIComponent(ch.mobile.address)}`}
                          className="text-accent-green hover:text-accent-green truncate"
                        >
                          {ch.mobile.phoneNumber || ch.mobile.address}
                        </Link>
                      )}
                    </div>
                  ))}
                </div>
              )}
              {trafficChannels.length === 0 && (
                <p className="text-dimmed text-xs">No active traffic channels</p>
              )}
            </>
          ) : (
            <p className="text-dimmed text-sm">Unavailable</p>
          )}
          <div className="mt-2 pt-2 border-t border-border">
            <Link href="/channels" className="text-xs text-accent-green hover:text-accent-green">
              All channels &rarr;
            </Link>
          </div>
        </Card>

        <Card title={`Mobiles (${mobiles.length})`}>
          {mobiles.length > 0 ? (
            <>
              <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted mb-3">
                {Object.entries(stateGroups).map(([state, count]) => (
                  <span key={state}>{count} {state}</span>
                ))}
              </div>
              <div className="space-y-1.5">
                {mobiles.slice(0, 8).map((ms, i) => (
                  <div key={i} className="flex items-center gap-2 text-xs">
                    <Link
                      href={`/mobiles/${encodeURIComponent(ms.address)}`}
                      className="text-accent-green hover:text-accent-green font-mono truncate"
                    >
                      {ms.phoneNumber || ms.address}
                    </Link>
                    <span className={`px-1.5 py-0.5 rounded ${stateColor(ms.state)}`}>
                      {ms.state}
                    </span>
                    {ms.trafficWalshCode != null && (
                      <span className="text-muted font-mono">W{ms.trafficWalshCode}</span>
                    )}
                  </div>
                ))}
                {mobiles.length > 8 && (
                  <p className="text-muted text-xs">+{mobiles.length - 8} more</p>
                )}
              </div>
            </>
          ) : (
            <p className="text-dimmed text-xs">No mobiles registered</p>
          )}
          <div className="mt-2 pt-2 border-t border-border">
            <Link href="/mobiles" className="text-xs text-accent-green hover:text-accent-green">
              All mobiles &rarr;
            </Link>
          </div>
        </Card>

        <Card title="SMSC">
          {smsRecent.length > 0 ? (
            <div className="space-y-1.5">
              {smsRecent.map((sms) => (
                <div key={sms.smsId} className="flex items-center gap-2 text-xs">
                  <span className="text-muted font-mono">
                    {formatIsoTime(sms.createdAt)}
                  </span>
                  <span className="text-secondary font-mono truncate max-w-[6rem]">
                    {sms.destinationNumber}
                  </span>
                  <span className="text-muted truncate max-w-[8rem]">{sms.text}</span>
                  <span className={`px-1.5 py-0.5 rounded ml-auto shrink-0 ${smsStateColor(sms.state)}`}>
                    {sms.state}
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <p className="text-dimmed text-xs">No SMS submissions</p>
          )}
          <div className="mt-2 pt-2 border-t border-border">
            <Link href="/smsc" className="text-xs text-accent-green hover:text-accent-green">
              SMSC &rarr;
            </Link>
          </div>
        </Card>

        <Card title="Packet Data">
          {packetSessions.length > 0 ? (
            <div className="space-y-2">
              <div className="flex items-center justify-between text-xs text-muted">
                <span>{packetSessions.filter((session) => session.phase === "active").length} active</span>
                <span>{packetSessions.filter((session) => session.captureEnabled).length} captured</span>
              </div>
              {packetSessions.slice(0, 4).map((s) => (
                <div key={s.sessionId} className="flex items-center gap-2 text-xs">
                  <span className={`px-1.5 py-0.5 rounded ${
                    s.phase === "active" ? "bg-badge-green-bg text-badge-green-text" :
                    s.phase === "closed" ? "bg-surface-raised text-muted" :
                    "bg-badge-yellow-bg text-badge-yellow-text"
                  }`}>
                    {packetPhaseLabel(s.phase)}
                  </span>
                  <Link
                    href={`/packets/${encodeURIComponent(s.sessionId)}`}
                    className="font-mono text-accent-green hover:text-accent-green"
                  >
                    {s.sessionId.slice(0, 8)}...
                  </Link>
                  <span className="text-muted truncate">
                    {s.phoneNumber || s.mobileAddress || s.peerIp || "-"}
                  </span>
                  <span className="text-muted ml-auto">
                    {s.accessTechnology || "1x"} {s.serviceOption === 33 ? "SO33" : `SO${s.serviceOption}`}
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <p className="text-dimmed text-xs">No packet sessions</p>
          )}
          <div className="mt-2 pt-2 border-t border-border">
            <Link href="/packets" className="text-xs text-accent-green hover:text-accent-green">
              Packet sessions &rarr;
            </Link>
          </div>
        </Card>
      </div>

      {/* Bottom: Recent Messages */}
      <Card title="Recent Messages">
        {recentEvents.length > 0 ? (
          <div className="space-y-0">
            {recentEvents.map((ev, i) => (
              <div key={i} className="flex items-center gap-2 text-xs py-1 border-b border-border-subtle last:border-0">
                <span className="text-muted font-mono w-[4.5rem] shrink-0">{formatTime(ev.ts)}</span>
                <span className={`font-mono w-5 shrink-0 ${
                  ev.kind === "access" ? "text-accent-green" : ev.kind === "traffic" ? "text-accent-cyan" : "text-accent-blue"
                }`}>
                  {ev.kind === "access" ? "RX" : "TX"}
                </span>
                <span className="text-dimmed shrink-0">{ev.channel}</span>
                <span className="text-secondary truncate">{ev.summary}</span>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-dimmed text-xs">Waiting for messages...</p>
        )}
        <div className="mt-2 pt-2 border-t border-border">
          <Link href="/messages" className="text-xs text-accent-green hover:text-accent-green">
            Full message log &rarr;
          </Link>
        </div>
      </Card>
    </div>
  );
}
