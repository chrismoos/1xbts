"use client";

import { useLive } from "@/lib/live";
import Link from "next/link";
import { useMemo } from "react";
import { Card } from "@/components/card";
import {
  mobileLabel,
  useMobileDirectory,
  type MobileDirectoryEntry,
} from "@/lib/mobile-directory";
import { packetBearer, packetSubscriber } from "@/lib/packet-session";
import { ErrorNotice } from "@/components/error-notice";

interface PacketSessionInfo {
  sessionId: string;
  phase: string;
  serviceOption: number;
  peerIp: string;
  ourIp: string;
  tunDevice: string;
  uplinkFrames: number;
  downlinkFrames: number;
  uplinkBytes: number;
  downlinkBytes: number;
  createdAtMs: number;
  lastPhaseChangeAtMs: number;
  lastUplinkAtMs: number;
  lastDownlinkAtMs: number;
  lastActivityAtMs: number;
  lastUplinkRateBps: number;
  lastDownlinkRateBps: number;
  mobileAddress: string;
  subscriberId: string;
  phoneNumber: string;
  imsi: string;
  meid: string;
  mnId: string;
  mnIdSource: string;
  subscriberImsi: string;
  esn: number;
  trafficWalshCode: number;
  rlpState: string;
  lcpState: string;
  ipcpState: string;
  captureEnabled: boolean;
  accessTechnology: string;
  pcfSessionId?: number;
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatAge(ms?: number): string {
  if (!ms) return "-";
  const seconds = Math.floor(Math.max(0, Date.now() - ms) / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

function formatPhase(phase: string): string {
  switch (phase) {
    case "rlp_sync":
      return "RLP sync";
    case "lcp":
      return "LCP";
    case "ipcp":
      return "IPCP";
    default:
      return phase;
  }
}

function formatRate(bps: number): string {
  if (bps < 1000) return `${bps} bps`;
  return `${(bps / 1000).toFixed(1)} kbps`;
}

function healthForSession(session: PacketSessionInfo): {
  label: string;
  className: string;
} {
  const idleMs = session.lastActivityAtMs ? Date.now() - session.lastActivityAtMs : Number.MAX_SAFE_INTEGER;
  if (session.phase === "active") {
    return { label: "Active", className: "bg-badge-green-bg text-badge-green-text" };
  }
  if (session.phase === "closed") {
    return { label: "Closed", className: "bg-surface-raised text-muted" };
  }
  if (session.phase === "rlp_sync" && Date.now() - session.createdAtMs > 10000) {
    return { label: "Stalled", className: "bg-accent-red-bg text-accent-red" };
  }
  if (idleMs > 10000) {
    return { label: "Idle", className: "bg-accent-amber-bg text-accent-amber" };
  }
  return { label: "Negotiating", className: "bg-badge-blue-bg text-badge-blue-text" };
}

const NO_SESSIONS: PacketSessionInfo[] = [];

const PHASE_RANK: Record<string, number> = { active: 0, ipcp: 1, lcp: 2, rlp_sync: 3, closed: 4 };

function SubscriberCell({ session, mobile }: { session: PacketSessionInfo; mobile?: MobileDirectoryEntry }) {
  const imsi = session.subscriberImsi || session.imsi || session.mnId;
  const name = mobile?.subscriberDisplayName || mobile?.phoneNumber || (mobile ? mobileLabel(mobile).value : undefined);
  return (
    <div>
      {mobile && name ? (
        <Link href={`/mobiles/${encodeURIComponent(mobile.address)}`} className="font-semibold text-primary hover:underline">
          {name}
        </Link>
      ) : (
        <span className="font-semibold text-muted">{session.phoneNumber || "Unknown subscriber"}</span>
      )}
      {mobile?.subscriberDisplayName && mobile.phoneNumber && (
        <div className="text-xs text-muted">{mobile.phoneNumber}</div>
      )}
      {imsi && <div className="font-mono text-[11px] text-dimmed">IMSI {imsi}</div>}
    </div>
  );
}

export default function PacketsPage() {
  const live = useLive<{ sessions?: PacketSessionInfo[] }>("/api/packet-sessions");
  const sessions = live.data?.sessions ?? NO_SESSIONS;
  const { loading } = live;
  const error = live.errorBody ?? live.error;
  const mobiles = useMobileDirectory();

  const rows = useMemo(
    () =>
      [...sessions]
        .sort(
          (left, right) =>
            (PHASE_RANK[left.phase] ?? 99) - (PHASE_RANK[right.phase] ?? 99) ||
            (right.lastActivityAtMs || 0) - (left.lastActivityAtMs || 0),
        )
        .map((session) => ({ session, ...packetSubscriber(session, mobiles) })),
    [sessions, mobiles],
  );

  const counts = {
    active: sessions.filter((session) => session.phase === "active").length,
    negotiating: sessions.filter((session) => session.phase !== "active" && session.phase !== "closed").length,
    stalled: sessions.filter((session) => healthForSession(session).label === "Stalled").length,
    captured: sessions.filter((session) => session.captureEnabled).length,
  };

  return (
    <div className="page-container space-y-4">
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="text-lg font-bold">Packet Data Sessions</h1>
        <div className="flex flex-wrap gap-2">
          <Pill value={sessions.length} label="sessions" />
          <Pill value={counts.active} label="active" className="text-accent-green" />
          <Pill value={counts.negotiating} label="negotiating" className="text-accent-blue" />
          {counts.stalled > 0 && <Pill value={counts.stalled} label="stalled" className="text-accent-red" />}
          {counts.captured > 0 && <Pill value={counts.captured} label="captured" className="text-accent-cyan" />}
        </div>
      </div>

      <Card title="Sessions">
        {loading ? (
          <p className="text-dimmed text-sm">Loading...</p>
        ) : error ? (
          <ErrorNotice error={error} />
        ) : rows.length === 0 ? (
          <p className="text-dimmed text-sm">No packet data sessions are currently registered.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Subscriber</th>
                  <th>Bearer</th>
                  <th>Status</th>
                  <th>Mobile IP</th>
                  <th>Last activity</th>
                  <th className="text-right">Up / Down</th>
                </tr>
              </thead>
              <tbody>
                {rows.map(({ session, mobile, evdoLinked }) => {
                  const health = healthForSession(session);
                  const bearer = packetBearer(session, evdoLinked);
                  const rate =
                    session.lastUplinkRateBps || session.lastDownlinkRateBps
                      ? `↑ ${formatRate(session.lastUplinkRateBps)} · ↓ ${formatRate(session.lastDownlinkRateBps)}`
                      : null;
                  return (
                    <tr key={session.sessionId} className="align-top">
                      <td>
                        <SubscriberCell session={session} mobile={mobile} />
                      </td>
                      <td>
                        <span className={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${bearer.className}`}>
                          {bearer.label}
                        </span>
                        <div className="mt-0.5">
                          <Link
                            href={`/packets/${encodeURIComponent(session.sessionId)}`}
                            className="font-mono text-[11px] text-accent-blue hover:underline"
                          >
                            {bearer.detail || "Session"}
                          </Link>
                        </div>
                      </td>
                      <td>
                        <span className={`rounded px-2 py-0.5 text-xs ${health.className}`}>{health.label}</span>
                        {session.phase !== "active" && session.phase !== "closed" && (
                          <div className="mt-1 text-[11px] text-muted">
                            {formatPhase(session.phase)} · LCP {session.lcpState || "-"} · IPCP {session.ipcpState || "-"}
                          </div>
                        )}
                        {session.captureEnabled && <div className="mt-1 text-[11px] text-accent-cyan">capture on</div>}
                      </td>
                      <td className="font-mono">{session.peerIp || "-"}</td>
                      <td>
                        <div className="text-xs text-secondary">{formatAge(session.lastActivityAtMs)} ago</div>
                        <div className="text-[11px] text-dimmed">up {formatAge(session.createdAtMs)}</div>
                        {rate && <div className="text-[11px] text-muted font-mono">{rate}</div>}
                      </td>
                      <td className="text-right">
                        <div className="font-mono text-xs text-secondary">
                          {formatBytes(session.uplinkBytes)} / {formatBytes(session.downlinkBytes)}
                        </div>
                        <div className="font-mono text-[11px] text-dimmed">
                          {session.uplinkFrames.toLocaleString()} / {session.downlinkFrames.toLocaleString()} frames
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}

function Pill({ value, label, className }: { value: number; label: string; className?: string }) {
  return (
    <div className="flex items-baseline gap-2 rounded-xl border border-border bg-surface px-3 py-1.5">
      <span className={`text-base font-semibold ${className ?? "text-primary"}`}>{value}</span>
      <span className="text-xs text-muted">{label}</span>
    </div>
  );
}
