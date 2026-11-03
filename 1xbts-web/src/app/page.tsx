"use client";

import { useState, useCallback } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import { SummaryTile } from "@/components/summary-tile";
import { HrpdSummaryCard } from "@/components/hrpd-summary-card";
import { AccessSummaryCard } from "@/components/access-summary-card";
import { type Channel, TrafficChannelsCard } from "@/components/traffic-channels";
import { RxPower } from "@/components/rx-power";
import { MobilesTable, type MobileInfo } from "@/components/mobiles-table";
import { useAdmissionVerdicts } from "@/components/access-activity";
import { useBaseStations } from "@/lib/use-base-stations";
import { attachStateLabel, cellLabel, sameCell, shortBaseStation } from "@/lib/cell";
import { ferColor, mergeCalls } from "@/lib/traffic-calls";
import { smsInFlight, smsStateColor } from "@/lib/sms-state";
import { useLive } from "@/lib/live";
import { useEventStream, type EventStreamOptions } from "@/lib/use-event-stream";
import { shouldHideAccessEvent } from "@/components/message-detail";
import { MessageListHeader, MessageRow, messageView } from "@/components/message-row";
import { type LogEntry, makeLogEntryId } from "@/lib/message-log";
import type { AccessEvent, PagingEvent, TrafficEvent } from "@/lib/proto/bsc/v1/service";
import { CellKind, BtsAttachState, type BtsSummary } from "@/lib/proto/bts_management/v1/service";

const REPLAY_EVENTS: EventStreamOptions = { replay: true };

interface RadioMetrics {
  tx?: { rtRatio: number };
  rx?: { rtRatio: number };
}

type NetworkCell = BtsSummary & { baseStation: string };

interface CoreStatus {
  components?: { msc: boolean; hlr: boolean; smsc: boolean };
}

interface ChannelsResponse {
  channels: Channel[];
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
  mobileAddress: string;
  phoneNumber: string;
  accessTechnology: string;
  captureEnabled: boolean;
  uplinkBytes: number;
  downlinkBytes: number;
}

interface RecentEvent {
  entry: LogEntry;
  source?: string;
}

type BottomTab = "messages" | "sms" | "packets";

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

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
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

function isToday(value?: string): boolean {
  if (!value) return false;
  const ts = new Date(value);
  const now = new Date();
  return ts.toDateString() === now.toDateString();
}

const REALTIME_FLOOR = 0.95;
const CALL_HISTORY_SAMPLES = 300;
const MOBILE_ROWS = 10;
const MAX_RECENT = 8;
const SMS_HISTORY_LIMIT = 100;
const SMS_ROWS = 8;
const NO_CELLS: NetworkCell[] = [];
const NO_MOBILES: MobileInfo[] = [];
const NO_SMS: SmsSubmission[] = [];
const NO_PACKET_SESSIONS: PacketSession[] = [];

export default function DashboardPage() {
  const { nodes } = useBaseStations();
  const networkCells = useLive<{ bts?: NetworkCell[] }>("/api/bts").data?.bts ?? NO_CELLS;
  const core = useLive<CoreStatus>("/api/core-status").data?.components;
  const [metricsByCell, setMetricsByCell] = useState<Record<string, RadioMetrics>>({});
  const liveMobiles = useLive<MobileInfo[]>("/api/mobiles").data;
  const mobiles = Array.isArray(liveMobiles) ? liveMobiles : NO_MOBILES;
  const channelData = useLive<ChannelsResponse>("/api/channels").data ?? null;
  const smsHistory =
    useLive<{ submissions?: SmsSubmission[] }>(`/api/sms-history?limit=${SMS_HISTORY_LIMIT}`).data?.submissions ?? NO_SMS;
  const packetSessions =
    useLive<{ sessions?: PacketSession[] }>("/api/packet-sessions").data?.sessions ?? NO_PACKET_SESSIONS;
  const { policy } = useAdmissionVerdicts({}, 1);
  const closed = policy?.accessMode === "closed";
  const [recentEvents, setRecentEvents] = useState<RecentEvent[]>([]);
  const [openRecent, setOpenRecent] = useState<Set<string>>(new Set());
  const [tab, setTab] = useState<BottomTab>("messages");
  const [callHistory, setCallHistory] = useState<{ source: ChannelsResponse | null; counts: number[] }>({
    source: null,
    counts: [],
  });

  // Stream live radio metrics, keyed by the cell that emitted them so the
  // health rollup covers every cell rather than whichever reported last.
  useEventStream("radio-metrics", (data) => {
    const parsed = JSON.parse(data);
    if (parsed.error) return;
    const node = parsed.management?.sourceNodeId ?? "";
    setMetricsByCell((prev) => ({ ...prev, [node]: parsed }));
  });

  const addRecent = useCallback((event: RecentEvent) => {
    setRecentEvents((prev) => [event, ...prev].slice(0, MAX_RECENT));
  }, []);

  useEventStream("paging", useCallback((data: string) => {
    const parsed = JSON.parse(data);
    const event: PagingEvent = parsed;
    const ts = event.timestampUs ?? Date.now() * 1000;
    const id = makeLogEntryId("tx", ts);
    addRecent({
      entry: { kind: "tx", stream: "paging", id, ts, identity: id, sortKey: id, event, seenCount: 1 },
      source: parsed.management?.sourceNodeId,
    });
  }, [addRecent]), REPLAY_EVENTS);

  useEventStream("traffic", useCallback((data: string) => {
    const parsed = JSON.parse(data);
    const event: TrafficEvent = parsed;
    const ts = event.timestampUs ?? Date.now() * 1000;
    const id = makeLogEntryId("tx", ts);
    addRecent({
      entry: { kind: "tx", stream: "traffic", id, ts, identity: id, sortKey: id, event, seenCount: 1 },
      source: parsed.management?.sourceNodeId,
    });
  }, [addRecent]), REPLAY_EVENTS);

  useEventStream("access", useCallback((data: string) => {
    const parsed = JSON.parse(data);
    const event: AccessEvent = parsed;
    if (shouldHideAccessEvent(event)) return;
    const ts = event.timestampUs ?? Date.now() * 1000;
    const id = makeLogEntryId("rx", ts);
    addRecent({
      entry: { kind: "rx", id, ts, identity: id, sortKey: id, event, seenCount: 1 },
      source: parsed.management?.sourceNodeId,
    });
  }, [addRecent]), REPLAY_EVENTS);

  const calls = channelData ? mergeCalls(channelData.channels) : [];

  if (channelData && callHistory.source !== channelData) {
    setCallHistory({ source: channelData, counts: [...callHistory.counts, calls.length].slice(-CALL_HISTORY_SAMPLES) });
  }

  const inServiceCells = networkCells.filter((cell) => cell.state === BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE);
  const cellMetrics = Object.values(metricsByCell);
  const worstTx = cellMetrics.reduce((lo, m) => Math.min(lo, m.tx?.rtRatio ?? Infinity), Infinity);
  const worstRx = cellMetrics.reduce((lo, m) => Math.min(lo, m.rx?.rtRatio ?? Infinity), Infinity);

  const problems = [
    ...(core
      ? (["msc", "hlr", "smsc"] as const).filter((c) => !core[c]).map((c) => `${c.toUpperCase()} down`)
      : []),
    ...nodes.filter((n) => !n.attached).map((n) => `${n.nodeId || shortBaseStation(n.managementEndpoint)} detached`),
    ...networkCells
      .filter((c) => c.state !== BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE)
      .map((c) => `${c.name || (c.cell ? cellLabel(c.cell) : shortBaseStation(c.managementEndpoint))} out of service`),
    ...(worstTx < REALTIME_FLOOR ? ["TX behind real time"] : []),
    ...(worstRx < REALTIME_FLOOR ? ["RX behind real time"] : []),
  ];

  const framesTotal = calls.reduce((sum, c) => sum + (c.power?.framesTotal ?? 0), 0);
  const framesBad = calls.reduce((sum, c) => sum + (c.power?.framesCrcError ?? 0), 0);
  const reverseFer = framesTotal > 0 ? (100 * framesBad) / framesTotal : null;
  const forwardReports = calls.filter((c) => (c.power?.forwardPmrmCount ?? 0) > 0);
  const forwardFer =
    forwardReports.length > 0
      ? forwardReports.reduce((sum, c) => sum + (c.power?.forwardLastFerPct ?? 0), 0) / forwardReports.length
      : null;

  const onVoice = mobiles.filter((m) => m.trafficWalshCode != null && !m.state.startsWith("HRPD")).length;
  const onData = mobiles.filter((m) => m.state.startsWith("HRPD") && m.trafficWalshCode != null).length;
  const smsToday = smsHistory.filter((s) => isToday(s.createdAt));
  const smsDelivered = smsToday.filter((s) => s.state === "delivered" || s.state === "sent").length;
  const smsPending = smsHistory.filter((s) => smsInFlight(s.state)).length;
  const activeSessions = packetSessions.filter((s) => s.phase === "active");
  const sessionBytes = activeSessions.reduce((sum, s) => sum + s.uplinkBytes + s.downlinkBytes, 0);

  const recentMobiles = [...mobiles].sort((a, b) => (b.lastHeardMs ?? 0) - (a.lastHeardMs ?? 0));
  const sid = networkCells[0]?.sid;
  const nid = networkCells[0]?.nid;

  return (
    <div className="max-w-7xl mx-auto space-y-4">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="text-xl font-semibold">Dashboard</h1>
            {problems.length === 0 ? (
              <span className="text-xs px-2 py-0.5 rounded bg-badge-green-bg text-badge-green-text">All systems normal</span>
            ) : (
              <span
                className="text-xs px-2 py-0.5 rounded bg-badge-yellow-bg text-badge-yellow-text"
                title={problems.join("\n")}
              >
                Degraded · {problems[0]}
                {problems.length > 1 && ` +${problems.length - 1}`}
              </span>
            )}
            {policy && (
              <span
                className={`text-xs px-2 py-0.5 rounded ${
                  closed ? "bg-badge-red-bg text-badge-red-text" : "bg-surface-raised text-secondary"
                }`}
              >
                Access: {closed ? "Closed" : "Open"}
              </span>
            )}
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted">
            {(["msc", "hlr", "smsc"] as const).map((c) => (
              <span key={c} className="inline-flex items-center gap-1.5">
                <span
                  className={`inline-block w-2 h-2 rounded-full ${
                    core == null ? "bg-signal-bar-off" : core[c] ? "bg-accent-green" : "bg-accent-red"
                  }`}
                />
                {c.toUpperCase()}
              </span>
            ))}
            {sid != null && (
              <span className="font-mono">
                SID {sid} · NID {nid}
              </span>
            )}
            {Number.isFinite(worstTx) && (
              <span className="font-mono" title="Worst real-time ratio across cells">
                RT {worstTx.toFixed(2)}x TX · {Number.isFinite(worstRx) ? worstRx.toFixed(2) : "-"}x RX
              </span>
            )}
          </div>
        </div>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3">
        <SummaryTile
          label="Cells"
          value={String(inServiceCells.length)}
          unit={`/ ${networkCells.length}`}
          detail={
            networkCells.length === inServiceCells.length
              ? "in service"
              : `${networkCells.length - inServiceCells.length} out of service`
          }
          detailClass={networkCells.length === inServiceCells.length ? "text-accent-green" : "text-accent-amber"}
        />
        <SummaryTile label="Active calls" value={String(calls.length)}>
          <CountSparkline values={callHistory.counts} />
        </SummaryTile>
        <SummaryTile
          label="Mobiles"
          value={String(mobiles.length)}
          detail={`${onVoice} on traffic · ${onData} data · ${mobiles.length - onVoice - onData} idle`}
        />
        <SummaryTile
          label="Reverse FER"
          value={reverseFer != null ? reverseFer.toFixed(2) : "-"}
          unit={reverseFer != null ? "%" : undefined}
          valueClass={reverseFer != null ? ferColor(reverseFer) : undefined}
          detail={forwardFer != null ? `fwd ${forwardFer.toFixed(1)}% · active calls` : "active calls"}
        />
        <SummaryTile
          label="SMS today"
          value={String(smsDelivered)}
          detail={smsPending > 0 ? `delivered · ${smsPending} pending` : "delivered"}
          detailClass={smsPending > 0 ? "text-accent-amber" : undefined}
        />
        <SummaryTile
          label="Data sessions"
          value={String(activeSessions.length)}
          detail={activeSessions.length > 0 ? `${formatBytes(sessionBytes)} transferred` : "none active"}
        />
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <Card title="Network" action={<Link href="/base-stations" className="text-xs text-accent-green">Base stations &rarr;</Link>}>
          {nodes.length === 0 ? (
            <p className="text-dimmed text-sm">Unavailable</p>
          ) : (
            <div className="-mx-4 -my-3">
              {nodes.map((node) => (
                <NetworkNode
                  key={node.managementEndpoint}
                  node={node}
                  cells={networkCells.filter((c) => c.baseStation === node.managementEndpoint)}
                  mobiles={mobiles}
                />
              ))}
            </div>
          )}
        </Card>
        <AccessSummaryCard />
        <HrpdSummaryCard />
      </div>

      <TrafficChannelsCard
        channels={channelData?.channels ?? []}
        channelsKey="/api/channels"
        baseStation=""
        showCell
        unavailable={!channelData}
        bodyClassName="h-64 overflow-y-auto"
        footer={
          <div className="mt-2 pt-2 border-t border-border flex items-center justify-between">
            <span className="text-[11px] text-dimmed">
              {channelData &&
                `Walsh: ${channelData.channels.filter((c) => c.direction === "forward" && c.channelType !== "traffic").length} overhead · ${calls.length} traffic`}
            </span>
            <Link href="/channels" className="text-xs text-accent-green hover:text-accent-green">
              All channels &rarr;
            </Link>
          </div>
        }
      />

      <Card
        title={`Mobiles (${mobiles.length})`}
        action={<span className="text-[11px] text-muted">by last heard</span>}
      >
        {mobiles.length === 0 ? (
          <p className="text-dimmed text-sm">No mobile stations registered.</p>
        ) : (
          <MobilesTable mobiles={recentMobiles.slice(0, MOBILE_ROWS)} cells={networkCells} closed={closed} />
        )}
        <div className="mt-2 pt-2 border-t border-border flex items-center justify-between">
          <Link href="/mobiles" className="text-xs text-accent-green hover:text-accent-green">
            All mobiles &rarr;
          </Link>
          {mobiles.length > MOBILE_ROWS && (
            <span className="text-[11px] text-dimmed">
              Showing {MOBILE_ROWS} of {mobiles.length}
            </span>
          )}
        </div>
      </Card>

      <div className="glass-card p-4">
        <div className="-mt-1 mb-3 flex gap-1 overflow-x-auto border-b border-border">
          {(
            [
              ["messages", "Recent messages", undefined],
              ["sms", "SMS", smsPending > 0 ? smsPending : undefined],
              ["packets", "Packet data", activeSessions.length > 0 ? activeSessions.length : undefined],
            ] as const
          ).map(([id, label, badge]) => (
            <button
              key={id}
              type="button"
              onClick={() => setTab(id)}
              className={`-mb-px whitespace-nowrap border-b-2 px-3.5 py-2 text-sm transition-colors ${
                tab === id ? "border-accent-green text-primary" : "border-transparent text-muted hover:text-secondary"
              }`}
            >
              {label}
              {badge != null && (
                <span className="ml-1.5 rounded-full bg-surface-raised px-1.5 py-px text-[10px] text-accent-green">
                  {badge}
                </span>
              )}
            </button>
          ))}
        </div>

        {tab === "messages" && (
          <>
            {recentEvents.length > 0 ? (
              <>
                <MessageListHeader subject={nodes.length > 1 ? "Base station" : undefined} />
                {recentEvents.map(({ entry, source }) => (
                  <MessageRow
                    key={entry.id}
                    entry={entry}
                    view={messageView(entry, { mobile: {}, verdicts: [], cells: [] })}
                    open={openRecent.has(entry.id)}
                    onToggle={() =>
                      setOpenRecent((prev) => {
                        const next = new Set(prev);
                        if (next.has(entry.id)) next.delete(entry.id);
                        else next.add(entry.id);
                        return next;
                      })
                    }
                    subject={
                      nodes.length > 1 ? (
                        <span className="font-mono text-xs text-dimmed" title={source}>
                          {source ?? "-"}
                        </span>
                      ) : undefined
                    }
                  />
                ))}
              </>
            ) : (
              <p className="text-dimmed text-xs">Waiting for messages...</p>
            )}
            <TabFooter href="/messages" label="Full message log" />
          </>
        )}

        {tab === "sms" && (
          <>
            {smsHistory.length > 0 ? (
              <div className="space-y-1.5">
                {smsHistory.slice(0, SMS_ROWS).map((sms) => (
                  <div key={sms.smsId} className="flex items-center gap-3 text-xs">
                    <span className="text-muted font-mono">{formatIsoTime(sms.createdAt)}</span>
                    <span className="text-secondary font-mono w-28 truncate">{sms.destinationNumber}</span>
                    <span className="text-muted truncate min-w-0 flex-1">{sms.text}</span>
                    <span className={`px-1.5 py-0.5 rounded shrink-0 ${smsStateColor(sms.state)}`}>{sms.state}</span>
                  </div>
                ))}
              </div>
            ) : (
              <p className="text-dimmed text-xs">No SMS submissions</p>
            )}
            <TabFooter href="/smsc" label="SMSC" />
          </>
        )}

        {tab === "packets" && (
          <>
            {packetSessions.length > 0 ? (
              <div className="space-y-1.5">
                {packetSessions.slice(0, SMS_ROWS).map((s) => (
                  <div key={s.sessionId} className="flex items-center gap-3 text-xs">
                    <span
                      className={`px-1.5 py-0.5 rounded ${
                        s.phase === "active"
                          ? "bg-badge-green-bg text-badge-green-text"
                          : s.phase === "closed"
                            ? "bg-surface-raised text-muted"
                            : "bg-badge-yellow-bg text-badge-yellow-text"
                      }`}
                    >
                      {packetPhaseLabel(s.phase)}
                    </span>
                    <Link
                      href={`/packets/${encodeURIComponent(s.sessionId)}`}
                      className="font-mono text-accent-green hover:text-accent-green"
                    >
                      {s.sessionId.slice(0, 8)}...
                    </Link>
                    <span className="text-muted truncate min-w-0 flex-1">
                      {s.phoneNumber || s.mobileAddress || s.peerIp || "-"}
                    </span>
                    <span className="text-muted font-mono">
                      ↓{formatBytes(s.downlinkBytes)} ↑{formatBytes(s.uplinkBytes)}
                    </span>
                    <span className="text-muted">
                      {s.accessTechnology || "1x"} SO{s.serviceOption}
                    </span>
                  </div>
                ))}
              </div>
            ) : (
              <p className="text-dimmed text-xs">No packet sessions</p>
            )}
            <TabFooter href="/packets" label="Packet sessions" />
          </>
        )}
      </div>
    </div>
  );
}

function CountSparkline({ values }: { values: number[] }) {
  if (values.length < 2) return <div className="mt-1 h-6" />;
  const hi = Math.max(1, ...values);
  const path = values
    .map((v, i) => `${i ? "L" : "M"}${((i / (values.length - 1)) * 100).toFixed(1)},${(22 - (v / hi) * 20).toFixed(1)}`)
    .join(" ");
  return (
    <svg viewBox="0 0 100 24" preserveAspectRatio="none" className="mt-1 block h-6 w-full" aria-hidden="true">
      <path d={path} fill="none" stroke="var(--accent-green)" strokeWidth={1.5} vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function NetworkNode({
  node,
  cells,
  mobiles,
}: {
  node: { managementEndpoint: string; nodeId: string; a1Addr: string; attached: boolean; statusDetail?: string };
  cells: NetworkCell[];
  mobiles: MobileInfo[];
}) {
  return (
    <div className="border-b border-border-subtle last:border-b-0">
      <div className="flex items-center justify-between gap-3 px-4 py-2 bg-hover">
        <span className="flex items-center gap-2 min-w-0">
          <span className={`inline-block w-2 h-2 shrink-0 rounded-full ${node.attached ? "bg-accent-green" : "bg-accent-amber"}`} />
          <span className="text-sm text-primary truncate">{node.nodeId || shortBaseStation(node.managementEndpoint)}</span>
          <span className="text-[11px] text-muted font-mono truncate">
            {node.attached ? `A1 ${node.a1Addr}` : "not attached"}
          </span>
        </span>
        <span className={`text-[11px] shrink-0 ${node.attached ? "text-muted" : "text-accent-amber"}`}>
          {node.attached ? `${cells.length} ${cells.length === 1 ? "cell" : "cells"}` : (node.statusDetail ?? "")}
        </span>
      </div>
      {cells.map((cell) => (
        <NetworkCellRow key={cell.peerId} cell={cell} mobiles={mobiles} />
      ))}
    </div>
  );
}

function NetworkCellRow({ cell, mobiles }: { cell: NetworkCell; mobiles: MobileInfo[] }) {
  const inService = cell.state === BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE;
  const femto = cell.kind === CellKind.CELL_KIND_FEMTOCELL;
  const probe = mobiles
    .filter(
      (m) =>
        m.baseStation === cell.baseStation &&
        sameCell(m.servingCell, cell.cell) &&
        (m.accessSignal?.noiseFloorDbm != null || m.accessSignal?.noiseFloorDbfs != null),
    )
    .sort((a, b) => (b.accessSignal?.measuredAtMs ?? 0) - (a.accessSignal?.measuredAtMs ?? 0))[0]?.accessSignal;

  return (
    <div className="flex items-start justify-between gap-3 py-2 pl-8 pr-4 border-t border-border-subtle">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <Link
            href={`/bts/${encodeURIComponent(cell.peerId || "")}?baseStation=${encodeURIComponent(cell.baseStation || "")}`}
            className="text-sm text-secondary hover:text-accent-green truncate"
          >
            {cell.name || cellLabel(cell.cell)}
          </Link>
          <span
            className={`rounded px-1.5 py-px text-[10px] ${
              femto ? "bg-badge-blue-bg text-badge-blue-text" : "bg-badge-purple-bg text-badge-purple-text"
            }`}
          >
            {femto ? "Femto" : "SDR"}
          </span>
          {cell.evdoEnabled && (
            <span className="rounded px-1.5 py-px text-[10px] bg-surface-raised text-muted">EV-DO</span>
          )}
        </div>
        <div className="text-[11px] text-muted font-mono">
          {[
            cell.bandClass ? `${cell.bandClass} CH ${cell.cdmaChannel}` : null,
            `PN ${cell.pilotPn}`,
            cell.softwareVersion ? `v${cell.softwareVersion}` : null,
          ]
            .filter(Boolean)
            .join(" · ")}
        </div>
        {!inService && (
          <div className="text-[11px] text-accent-amber">
            {attachStateLabel(cell.state)}
            {cell.statusDetail && ` · ${cell.statusDetail}`}
          </div>
        )}
      </div>
      <div className="text-right text-[11px] text-muted shrink-0">
        <div>
          {cell.servedMobiles ?? 0} {cell.servedMobiles === 1 ? "mobile" : "mobiles"}
        </div>
        {probe && (
          <div className="font-mono" title={probe.noiseFloorStale ? "No quiet period to update from recently" : "Reverse noise floor"}>
            noise <RxPower dbm={probe.noiseFloorDbm} dbfs={probe.noiseFloorDbfs} />
          </div>
        )}
      </div>
    </div>
  );
}

function TabFooter({ href, label }: { href: string; label: string }) {
  return (
    <div className="mt-2 pt-2 border-t border-border">
      <Link href={href} className="text-xs text-accent-green hover:text-accent-green">
        {label} &rarr;
      </Link>
    </div>
  );
}
