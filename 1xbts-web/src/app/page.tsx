"use client";

import { useEffect, useState, useCallback } from "react";
import Link from "next/link";
import { Card, Stat } from "@/components/card";
import { HrpdSummaryCard } from "@/components/hrpd-summary-card";
import { AccessSummaryCard } from "@/components/access-summary-card";
import { useBaseStations } from "@/lib/use-base-stations";
import { attachStateLabel, cellLabel } from "@/lib/cell";
import { radioConfigPairName } from "@/lib/radio-config";
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
import { CellKind, BtsAttachState, type BtsSummary } from "@/lib/proto/bts_management/v1/service";

// ─── Types ──────────────────────────────────────────────────────

interface RadioMetrics {
  tx?: { rtRatio: number; blocksTransmitted: number; syncFragmentsSent: number; pagingFragmentsSent: number };
  rx?: { rtRatio: number; deficitMs?: number };
}

type NetworkCell = BtsSummary & { baseStation: string };

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
  cell?: { cell: number; sector: number };
  mobile?: { address: string; state: string; phoneNumber?: string };
  trafficPower?: {
    forwardRadioConfig: number;
    reverseRadioConfig: number;
  };
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
  | { kind: "paging"; ts: number; summary: string; channel: string; source?: string }
  | { kind: "traffic"; ts: number; summary: string; channel: string; source?: string }
  | { kind: "access"; ts: number; summary: string; channel: string; source?: string };

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
  const { nodes } = useBaseStations();
  const [networkCells, setNetworkCells] = useState<NetworkCell[]>([]);
  const [metricsByCell, setMetricsByCell] = useState<Record<string, RadioMetrics>>({});
  const [mobiles, setMobiles] = useState<MobileInfo[]>([]);
  const [channelData, setChannelData] = useState<ChannelsResponse | null>(null);
  const [smsRecent, setSmsRecent] = useState<SmsSubmission[]>([]);
  const [packetSessions, setPacketSessions] = useState<PacketSession[]>([]);
  const [recentEvents, setRecentEvents] = useState<RecentEvent[]>([]);

  const attachedBs = nodes.filter((node) => node.attached).length;
  const inServiceCells = networkCells.filter(
    (cell) => cell.state === BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE,
  ).length;

  useEffect(() => {
    const fetchNetworkCells = () => {
      void fetch("/api/bts")
        .then((response) => response.json())
        .then((result) => {
          setNetworkCells(result.bts || []);
        });
    };
    fetchNetworkCells();
    const interval = setInterval(fetchNetworkCells, 10000);
    return () => clearInterval(interval);
  }, []);

  // Stream live radio metrics, keyed by the cell that emitted them so the
  // health rollup covers every cell rather than whichever reported last.
  useEventStream("radio-metrics", (data) => {
    const parsed = JSON.parse(data);
    if (parsed.error) return;
    const node = parsed.management?.sourceNodeId ?? "";
    setMetricsByCell((prev) => ({ ...prev, [node]: parsed }));
  });

  const cellMetrics = Object.values(metricsByCell);
  const healthyCells = cellMetrics.filter(
    (m) => (m.tx?.rtRatio ?? 0) >= 0.95 && (m.rx?.rtRatio ?? 0) >= 0.95,
  ).length;
  const worstTx = cellMetrics.reduce((lo, m) => Math.min(lo, m.tx?.rtRatio ?? Infinity), Infinity);
  const worstRx = cellMetrics.reduce((lo, m) => Math.min(lo, m.rx?.rtRatio ?? Infinity), Infinity);

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
    const parsed = JSON.parse(data);
    const ev: PagingEvent = parsed;
    const name = ev.header?.msgTypeName ?? "Paging";
    const detail = formatPagingSummary(ev);
    addRecent({
      kind: "paging",
      ts: ev.timestampUs ? Math.floor(ev.timestampUs / 1000) : Date.now(),
      summary: detail ? `${name} - ${detail}` : name,
      channel: formatPagingChannel(ev),
      source: parsed.management?.sourceNodeId,
    });
  }, [addRecent]));

  useEventStream("traffic", useCallback((data: string) => {
    const parsed = JSON.parse(data);
    const ev: TrafficEvent = parsed;
    const name = ev.header?.msgTypeName ?? "Traffic";
    const detail = formatTrafficSummary(ev);
    addRecent({
      kind: "traffic",
      ts: ev.timestampUs ? Math.floor(ev.timestampUs / 1000) : Date.now(),
      summary: detail ? `${name} - ${detail}` : name,
      channel: formatTrafficChannel(ev),
      source: parsed.management?.sourceNodeId,
    });
  }, [addRecent]));

  useEventStream("access", useCallback((data: string) => {
    const parsed = JSON.parse(data);
    const ev: AccessEvent = parsed;
    if (shouldHideAccessEvent(ev)) return;
    addRecent({
      kind: "access",
      ts: ev.timestampUs ? Math.floor(ev.timestampUs / 1000) : Date.now(),
      summary: `${formatAccessTypeName(ev)} ${formatAccessSummary(ev)}`,
      channel: formatAccessChannel(ev),
      source: parsed.management?.sourceNodeId,
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
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <Card title="Network" className="lg:col-span-2">
          {nodes.length > 0 ? (
            <>
              <div className="grid grid-cols-2 gap-x-8 mb-3">
                <Stat label="Base Stations" value={`${attachedBs}/${nodes.length} attached`} />
                <Stat label="Cells" value={`${inServiceCells}/${networkCells.length} in service`} />
              </div>
              <div className="overflow-x-auto border-t border-border">
                <table className="w-full text-xs">
                  <thead>
                    <tr className="text-dimmed uppercase tracking-wide text-[10px]">
                      <th className="text-left py-2 font-medium">Cell</th>
                      <th className="text-left py-2 font-medium">Type</th>
                      <th className="text-left py-2 font-medium">State</th>
                      <th className="text-left py-2 font-medium">Radio</th>
                      <th className="text-right py-2 font-medium">Mobiles</th>
                    </tr>
                  </thead>
                  <tbody>
                    {networkCells.slice(0, 5).map((cell) => (
                      <tr key={`${cell.baseStation}:${cell.peerId}`} className="border-t border-border-subtle">
                        <td className="py-2 pr-3">
                          <Link
                            href={`/bts/${encodeURIComponent(cell.peerId || "")}?baseStation=${encodeURIComponent(cell.baseStation || "")}`}
                            className="font-medium text-secondary hover:text-accent-green"
                          >
                            {cell.name || cellLabel(cell.cell)}
                          </Link>
                          <div className="text-[10px] text-dimmed font-mono mt-0.5">
                            SID {cell.sid} · NID {cell.nid}
                          </div>
                        </td>
                        <td className="py-2 pr-3">
                          <span className={`rounded px-1.5 py-0.5 text-[10px] ${
                            cell.kind === CellKind.CELL_KIND_FEMTOCELL
                              ? "bg-badge-blue-bg text-badge-blue-text"
                              : "bg-badge-purple-bg text-badge-purple-text"
                          }`}>
                            {cell.kind === CellKind.CELL_KIND_FEMTOCELL ? "Femto" : "SDR"}
                          </span>
                        </td>
                        <td className="py-2 pr-3 text-muted">{attachStateLabel(cell.state)}</td>
                        <td className="py-2 pr-3 text-dimmed font-mono whitespace-nowrap">
                          {cell.bandClass || "-"} · CH {cell.cdmaChannel}
                        </td>
                        <td className="py-2 text-right text-secondary">{cell.servedMobiles ?? "-"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          ) : (
            <p className="text-dimmed text-sm">Unavailable</p>
          )}
          <div className="mt-2 pt-2 border-t border-border flex items-center justify-between">
            <Link href="/bts" className="text-xs text-accent-green hover:text-accent-green">
              View all cells &rarr;
            </Link>
            {networkCells.length > 5 && (
              <span className="text-[11px] text-dimmed">Showing 5 of {networkCells.length}</span>
            )}
          </div>
        </Card>

        <Card title="Radio Health">
          {cellMetrics.length > 0 ? (
            <>
              <div className="flex items-center gap-2 mb-2">
                <span className={`inline-block w-2 h-2 rounded-full ${
                  healthyCells === cellMetrics.length
                    ? "bg-accent-green"
                    : worstTx >= 0.8 && worstRx >= 0.8
                      ? "bg-accent-amber"
                      : "bg-accent-red"
                }`} />
                <span className="text-xs text-muted">
                  {healthyCells}/{cellMetrics.length} cells real-time
                </span>
              </div>
              <Stat label="Worst TX" value={Number.isFinite(worstTx) ? `${worstTx.toFixed(3)}x` : "-"} mono />
              <Stat label="Worst RX" value={Number.isFinite(worstRx) ? `${worstRx.toFixed(3)}x` : "-"} mono />
            </>
          ) : (
            <p className="text-dimmed text-sm">No radio metrics</p>
          )}
          <div className="mt-2 pt-2 border-t border-border">
            <Link href="/bts" className="text-xs text-accent-green hover:text-accent-green">
              Per-cell radio metrics &rarr;
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
                      {ch.cell && (
                        <span className="text-dimmed font-mono" title="cell">{cellLabel(ch.cell)}</span>
                      )}
                      {radioConfigPairName(
                        ch.trafficPower?.forwardRadioConfig,
                        ch.trafficPower?.reverseRadioConfig,
                      ) && (
                        <span className="font-mono text-secondary">
                          {radioConfigPairName(
                            ch.trafficPower?.forwardRadioConfig,
                            ch.trafficPower?.reverseRadioConfig,
                          )}
                        </span>
                      )}
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

        <AccessSummaryCard />

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
                {nodes.length > 1 && ev.source && (
                  <span className="text-dimmed shrink-0 truncate max-w-[7rem] font-mono" title={ev.source}>
                    {ev.source}
                  </span>
                )}
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
