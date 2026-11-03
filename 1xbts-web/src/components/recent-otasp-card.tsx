"use client";

import { useState } from "react";
import { useLive } from "@/lib/live";
import Link from "next/link";
import { Card } from "@/components/card";
import { StartOtasp } from "@/components/start-otasp";
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

const NO_ROWS: SessionRow[] = [];

export function RecentOtaspCard({
  subscriberId,
  esn,
  meid,
  title = "Recent OTASP Sessions",
}: {
  // Exactly one filter is supplied. The subscriber detail page passes
  // `subscriberId`; the mobile detail page passes `esn` / `meid`.
  subscriberId?: string;
  esn?: number | string;
  meid?: string;
  title?: string;
}) {
  const [offset, setOffset] = useState(0);
  const [limit, setLimit] = useState(10);
  const params = new URLSearchParams();
  if (subscriberId) params.set("subscriberId", subscriberId);
  if (esn != null) params.set("esn", String(esn));
  if (meid) params.set("meid", meid.toLowerCase());
  params.set("limit", String(limit));
  params.set("offset", String(offset));
  const live = useLive<{ sessions?: SessionRow[]; total?: number }>(`/api/otasp-sessions?${params}`);
  const rows = live.data?.sessions ?? NO_ROWS;
  const total = live.data?.total ?? 0;
  const { loading, error } = live;

  return (
    <Card title={title}>
      {subscriberId && <div className="mb-4"><StartOtasp subscriberId={subscriberId} /></div>}
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
