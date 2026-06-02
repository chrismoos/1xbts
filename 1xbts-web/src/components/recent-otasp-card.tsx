"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import { Pagination } from "@/components/pagination";
import { formatTimeMs as formatTime } from "@/lib/format";
import { outcomeBadgeColor, outcomeNameToLabel } from "@/lib/otasp";

// Mirrors `hlr.v1.OtaspSessionSummary` after proto toJSON (timestamps
// arrive as ISO strings, optional fields are absent rather than null).
interface SessionRow {
  sessionId: string;
  subscriberId?: string;
  esn?: number;
  meid?: string;
  startedAt?: string;
  endedAt?: string;
  outcome?: string | number;
  featureCode?: string;
  serviceOption?: number;
  completedBlocks?: number;
  eventCount?: number;
}

function formatIsoTime(value?: string): string {
  if (!value) return "-";
  const ts = Date.parse(value);
  return Number.isFinite(ts) ? formatTime(ts) : "-";
}

function relativeTime(value?: string): string {
  if (!value) return "";
  const ts = Date.parse(value);
  if (!Number.isFinite(ts)) return "";
  const deltaMs = Date.now() - ts;
  const sec = Math.round(deltaMs / 1000);
  if (sec < 60) return `${sec}s ago`;
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return `${Math.round(hr / 24)}d ago`;
}

export function RecentOtaspCard({
  subscriberId,
  esn,
  meid,
}: {
  // Exactly one filter is supplied. The subscriber detail page passes
  // `subscriberId`; the mobile detail page passes `esn` / `meid`.
  subscriberId?: string;
  esn?: number | string;
  meid?: string;
}) {
  const [rows, setRows] = useState<SessionRow[]>([]);
  const [total, setTotal] = useState(0);
  const [offset, setOffset] = useState(0);
  const [limit, setLimit] = useState(10);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const params = new URLSearchParams();
        if (subscriberId) params.set("subscriberId", subscriberId);
        if (esn != null) params.set("esn", String(esn));
        if (meid) params.set("meid", meid.toLowerCase());
        params.set("limit", String(limit));
        params.set("offset", String(offset));
        const res = await fetch(`/api/otasp-sessions?${params}`);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        if (!alive) return;
        if (data.error) throw new Error(data.error);
        setRows(data.sessions || []);
        setTotal(data.total ?? 0);
        setError(null);
      } catch (err) {
        if (!alive) return;
        setError(err instanceof Error ? err.message : "unknown");
      } finally {
        if (alive) setLoading(false);
      }
    };
    load();
    // Polling keeps the current page fresh — a session that completes
    // while the operator is looking at page 1 will appear within 10 s.
    const interval = setInterval(load, 10000);
    return () => {
      alive = false;
      clearInterval(interval);
    };
  }, [subscriberId, esn, meid, limit, offset]);

  return (
    <Card title="Recent OTASP Sessions">
      {loading && rows.length === 0 ? (
        <p className="text-dimmed text-sm">Loading...</p>
      ) : error ? (
        <p className="text-accent-red text-sm">{error}</p>
      ) : rows.length === 0 ? (
        <p className="text-dimmed text-sm">No OTASP sessions for this device.</p>
      ) : (
        <>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-muted text-xs">
                  <th className="text-left py-1 pr-6">Started</th>
                  <th className="text-left py-1 pr-6">Ended</th>
                  <th className="text-left py-1 pr-6">Outcome</th>
                  <th className="text-left py-1 pr-6">Blocks</th>
                  <th className="text-left py-1 pr-6">Events</th>
                  <th className="text-left py-1">Session</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={row.sessionId} className="border-t border-border hover:bg-hover">
                    <td className="py-1.5 pr-6 text-muted font-mono text-xs whitespace-nowrap">
                      <span title={formatIsoTime(row.startedAt)}>{relativeTime(row.startedAt) || "-"}</span>
                    </td>
                    <td className="py-1.5 pr-6 text-muted font-mono text-xs whitespace-nowrap">
                      <span title={formatIsoTime(row.endedAt)}>{row.endedAt ? relativeTime(row.endedAt) : "in progress"}</span>
                    </td>
                    <td className="py-1.5 pr-6 text-xs whitespace-nowrap">
                      <span className={`px-2 py-0.5 rounded ${outcomeBadgeColor(row.outcome)}`}>
                        {outcomeNameToLabel(row.outcome)}
                      </span>
                    </td>
                    <td className="py-1.5 pr-6 text-xs text-muted font-mono">{row.completedBlocks ?? 0}</td>
                    <td className="py-1.5 pr-6 text-xs text-muted font-mono">{row.eventCount ?? 0}</td>
                    <td className="py-1.5 text-xs">
                      <Link
                        href={`/otasp/${encodeURIComponent(row.sessionId)}`}
                        className="text-accent-green hover:text-accent-green font-mono"
                      >
                        {row.sessionId.slice(0, 8)}…
                      </Link>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <Pagination
            total={total}
            offset={offset}
            limit={limit}
            onPageChange={(next) => setOffset(next)}
            onLimitChange={(next) => {
              setLimit(next);
              setOffset(0);
            }}
          />
        </>
      )}
    </Card>
  );
}
