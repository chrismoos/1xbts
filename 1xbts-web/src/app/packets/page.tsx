"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Card } from "@/components/card";

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
  trafficWalshCode: number;
  rlpState: string;
  lcpState: string;
  ipcpState: string;
  captureEnabled: boolean;
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatTimestamp(ms?: number): string {
  if (!ms) return "-";
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "-";
  return d.toLocaleTimeString();
}

function formatAge(ms?: number): string {
  if (!ms) return "-";
  const delta = Math.max(0, Date.now() - ms);
  const seconds = Math.floor(delta / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const rem = seconds % 60;
  if (minutes < 60) return `${minutes}m ${rem}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

function formatPhase(phase: string): string {
  switch (phase) {
    case "rlp_sync":
      return "RLP Sync";
    case "lcp":
      return "LCP";
    case "ipcp":
      return "IPCP";
    case "active":
      return "Active";
    case "closed":
      return "Closed";
    default:
      return phase;
  }
}

function formatStateLabel(value: string): string {
  if (!value) return "-";
  return value
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function serviceOptionName(so: number): string {
  switch (so) {
    case 7:
      return "SO 7";
    case 33:
      return "SO 33";
    default:
      return `SO ${so}`;
  }
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

export default function PacketsPage() {
  const [sessions, setSessions] = useState<PacketSessionInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchSessions = useCallback(async () => {
    try {
      const res = await fetch("/api/packet-sessions", { cache: "no-store" });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setSessions(data.sessions || []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchSessions();
    const interval = setInterval(fetchSessions, 3000);
    return () => clearInterval(interval);
  }, [fetchSessions]);

  const sortedSessions = useMemo(() => {
    const phaseRank: Record<string, number> = {
      active: 0,
      ipcp: 1,
      lcp: 2,
      rlp_sync: 3,
      closed: 4,
    };
    return [...sessions].sort((left, right) => {
      const phaseDelta = (phaseRank[left.phase] ?? 99) - (phaseRank[right.phase] ?? 99);
      if (phaseDelta !== 0) return phaseDelta;
      return (right.lastActivityAtMs || 0) - (left.lastActivityAtMs || 0);
    });
  }, [sessions]);

  const activeCount = sessions.filter((session) => session.phase === "active").length;
  const negotiatingCount = sessions.filter(
    (session) => session.phase !== "active" && session.phase !== "closed"
  ).length;
  const captureCount = sessions.filter((session) => session.captureEnabled).length;
  const stalledCount = sessions.filter((session) => healthForSession(session).label === "Stalled").length;

  return (
    <div className="max-w-7xl mx-auto space-y-4">
      <div className="flex items-center gap-4">
        <h1 className="text-lg font-bold">Packet Data Sessions</h1>
        <span className="text-xs text-muted">
          {sessions.length} session{sessions.length !== 1 ? "s" : ""}
        </span>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <Card title="Active">
          <p className="text-2xl font-mono text-accent-green">{activeCount}</p>
          <p className="mt-1 text-xs text-muted">IPCP complete and forwarding</p>
        </Card>
        <Card title="Negotiating">
          <p className="text-2xl font-mono text-accent-blue">{negotiatingCount}</p>
          <p className="mt-1 text-xs text-muted">RLP, LCP, or IPCP in progress</p>
        </Card>
        <Card title="Captured">
          <p className="text-2xl font-mono text-accent-cyan">{captureCount}</p>
          <p className="mt-1 text-xs text-muted">Sessions with IP capture enabled</p>
        </Card>
        <Card title="Stalled">
          <p className="text-2xl font-mono text-accent-red">{stalledCount}</p>
          <p className="mt-1 text-xs text-muted">RLP sync sessions lingering &gt;10s</p>
        </Card>
      </div>

      {loading ? (
        <p className="text-dimmed text-sm">Loading...</p>
      ) : error ? (
        <p className="text-accent-red text-sm">{error}</p>
      ) : sortedSessions.length === 0 ? (
        <Card title="Sessions">
          <p className="text-dimmed text-sm">
            No packet data sessions are currently registered.
          </p>
        </Card>
      ) : (
        <Card title="Sessions">
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-xs text-muted">
                  <th className="text-left py-1">Session</th>
                  <th className="text-left py-1">Mobile</th>
                  <th className="text-left py-1">Phase</th>
                  <th className="text-left py-1">Health</th>
                  <th className="text-left py-1">RLP / PPP</th>
                  <th className="text-left py-1">Last Activity</th>
                  <th className="text-left py-1">Bearer</th>
                  <th className="text-right py-1">Traffic</th>
                </tr>
              </thead>
              <tbody>
                {sortedSessions.map((session) => {
                  const health = healthForSession(session);
                  return (
                    <tr
                      key={session.sessionId}
                      className="border-t border-border hover:bg-hover"
                    >
                      <td className="py-2 align-top">
                        <Link
                          href={`/packets/${encodeURIComponent(session.sessionId)}`}
                          className="font-mono text-xs text-accent-green hover:text-accent-green"
                        >
                          {session.sessionId.slice(0, 8)}...{session.sessionId.slice(-8)}
                        </Link>
                        <div className="mt-1 text-[11px] text-muted">
                          age {formatAge(session.createdAtMs)}
                        </div>
                      </td>
                      <td className="py-2 align-top">
                        {session.mobileAddress ? (
                          <div className="space-y-1">
                            <Link
                              href={`/mobiles/${encodeURIComponent(session.mobileAddress)}`}
                              className="font-mono text-xs text-accent-cyan hover:text-accent-cyan"
                            >
                              {session.phoneNumber || session.mobileAddress}
                            </Link>
                            <div className="text-[11px] text-muted font-mono">
                              {session.mobileAddress}
                            </div>
                            {session.subscriberId && (
                              <div className="text-[11px] text-dimmed font-mono">
                                {session.subscriberId.slice(0, 8)}...{session.subscriberId.slice(-8)}
                              </div>
                            )}
                          </div>
                        ) : (
                          <span className="text-xs text-dimmed">-</span>
                        )}
                      </td>
                      <td className="py-2 align-top">
                        <span className="rounded bg-surface-raised px-2 py-0.5 text-xs text-primary">
                          {formatPhase(session.phase)}
                        </span>
                        {session.captureEnabled && (
                          <div className="mt-1 text-[11px] text-accent-cyan">capture enabled</div>
                        )}
                      </td>
                      <td className="py-2 align-top">
                        <span className={`rounded px-2 py-0.5 text-xs ${health.className}`}>
                          {health.label}
                        </span>
                      </td>
                      <td className="py-2 align-top">
                        <div className="text-xs text-secondary">
                          RLP {formatStateLabel(session.rlpState)}
                        </div>
                        <div className="text-[11px] text-muted">
                          LCP {formatStateLabel(session.lcpState)} / IPCP {formatStateLabel(session.ipcpState)}
                        </div>
                      </td>
                      <td className="py-2 align-top">
                        <div className="text-xs font-mono text-secondary">
                          {formatTimestamp(session.lastActivityAtMs)}
                        </div>
                        <div className="text-[11px] text-muted">
                          UL {session.lastUplinkRateBps || 0} / DL {session.lastDownlinkRateBps || 0}
                        </div>
                      </td>
                      <td className="py-2 align-top">
                        <div className="text-xs text-secondary">
                          {serviceOptionName(session.serviceOption)}
                          {session.trafficWalshCode ? ` · W${session.trafficWalshCode}` : ""}
                        </div>
                        <div className="text-[11px] text-muted font-mono">
                          {session.peerIp || "-"} / {session.ourIp || "-"}
                        </div>
                        <div className="text-[11px] text-dimmed font-mono">
                          {session.tunDevice || "no TUN"}
                        </div>
                      </td>
                      <td className="py-2 align-top text-right">
                        <div className="text-xs font-mono text-secondary">
                          {formatBytes(session.uplinkBytes)} / {formatBytes(session.downlinkBytes)}
                        </div>
                        <div className="text-[11px] text-muted font-mono">
                          {session.uplinkFrames.toLocaleString()} / {session.downlinkFrames.toLocaleString()} frames
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </Card>
      )}
    </div>
  );
}
