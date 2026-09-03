"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useParams } from "next/navigation";
import { Card } from "@/components/card";
import { attachStateBadgeClass, attachStateLabel } from "@/lib/cell";
import { BtsAttachState, type BtsSummary } from "@/lib/proto/bts_management/v1/service";
import { useBaseStations } from "@/lib/use-base-stations";

export default function BaseStationDetailPage() {
  const raw = useParams().id;
  const id = decodeURIComponent(Array.isArray(raw) ? raw[0] : (raw ?? ""));
  const { nodes } = useBaseStations(5000);
  const node = nodes.find((candidate) => candidate.managementEndpoint === id);

  const [cells, setCells] = useState<BtsSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      fetch(`/api/bts?baseStation=${encodeURIComponent(id)}`, { cache: "no-store" })
        .then((r) => r.json())
        .then((data: { bts?: BtsSummary[]; error?: string }) => {
          if (cancelled) return;
          if (data.error) {
            setError(data.error);
            setCells([]);
          } else {
            setError(null);
            setCells(data.bts ?? []);
          }
          setLoading(false);
        })
        .catch((err) => {
          if (cancelled) return;
          setError(err instanceof Error ? err.message : "unknown error");
          setLoading(false);
        });
    };
    load();
    const interval = setInterval(load, 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [id]);

  const inService = cells.filter(
    (summary) => summary.state === BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE,
  ).length;

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <div className="flex flex-wrap items-center gap-4">
        <Link href="/base-stations" className="text-xs text-accent-green hover:underline">
          &larr; Base Stations
        </Link>
        <h1 className="text-lg font-bold font-mono">{node?.nodeId || id}</h1>
        {cells.length > 0 && (
          <div className="flex gap-3 text-xs text-muted">
            <span>{cells.length} cells</span>
            <span>{inService} in service</span>
          </div>
        )}
      </div>

      <Card title="Base Station">
        <dl className="grid grid-cols-2 gap-x-6 gap-y-2 text-sm sm:grid-cols-4">
          <div>
            <dt className="text-xs text-muted">Node</dt>
            <dd className="font-mono text-primary">
              {node?.nodeId || <span className="text-dimmed">not enrolled</span>}
            </dd>
          </div>
          <div>
            <dt className="text-xs text-muted">Management</dt>
            <dd className="font-mono text-xs text-secondary">{id}</dd>
          </div>
          <div>
            <dt className="text-xs text-muted">A1</dt>
            <dd className="font-mono text-xs text-secondary">{node?.a1Addr || "-"}</dd>
          </div>
          <div>
            <dt className="text-xs text-muted">State</dt>
            <dd>
              <span
                className={`rounded px-1.5 py-0.5 text-[11px] ${
                  node?.attached
                    ? "bg-badge-green-bg text-badge-green-text"
                    : "bg-surface-raised text-muted"
                }`}
              >
                {node?.attached ? "attached" : node?.nodeId ? "detached" : "not enrolled"}
              </span>
            </dd>
          </div>
        </dl>
      </Card>

      <Card title={`Cells (${cells.length})`}>
        {loading ? (
          <p className="text-dimmed text-sm">Loading...</p>
        ) : error ? (
          <p className="text-accent-red text-sm">{error}</p>
        ) : cells.length === 0 ? (
          <p className="text-dimmed text-sm">No cells on this base station.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-muted text-xs">
                  <th className="text-left py-1">Cell</th>
                  <th className="text-left py-1">State</th>
                  <th className="text-right py-1">Pilot PN</th>
                  <th className="text-left py-1 pl-4">Band Class</th>
                  <th className="text-right py-1">Channel</th>
                  <th className="text-right py-1">SID</th>
                  <th className="text-right py-1">NID</th>
                  <th className="text-left py-1 pl-4">EV-DO</th>
                  <th className="text-right py-1">Mobiles</th>
                  <th className="text-left py-1"></th>
                </tr>
              </thead>
              <tbody>
                {cells.map((summary) => (
                  <tr key={summary.peerId} className="border-t border-border hover:bg-hover">
                    <td className="py-2 font-mono text-xs text-primary">
                      {summary.cell ? (
                        <>
                          <div>{`Cell ${summary.cell.cell}`}</div>
                          <div className="text-[11px] text-muted">
                            {`Sector ${summary.cell.sector}`}
                          </div>
                        </>
                      ) : (
                        <div>{summary.peerId}</div>
                      )}
                    </td>
                    <td className="py-2">
                      <span
                        className={`text-xs px-2 py-0.5 rounded ${attachStateBadgeClass(summary.state)}`}
                      >
                        {attachStateLabel(summary.state)}
                      </span>
                      {summary.statusDetail && (
                        <div className="text-[11px] text-muted mt-1">
                          {summary.statusDetail}
                        </div>
                      )}
                    </td>
                    <td className="py-2 text-right font-mono text-xs text-secondary">
                      {summary.cell ? summary.pilotPn : "-"}
                    </td>
                    <td className="py-2 pl-4 font-mono text-xs text-secondary">
                      {summary.bandClass || "-"}
                    </td>
                    <td className="py-2 text-right font-mono text-xs text-secondary">
                      {summary.cell ? summary.cdmaChannel : "-"}
                    </td>
                    <td className="py-2 text-right font-mono text-xs text-secondary">
                      {summary.cell ? summary.sid : "-"}
                    </td>
                    <td className="py-2 text-right font-mono text-xs text-secondary">
                      {summary.cell ? summary.nid : "-"}
                    </td>
                    <td className="py-2 pl-4">
                      <span
                        className={`text-xs px-2 py-0.5 rounded ${
                          summary.evdoEnabled
                            ? "bg-badge-purple-bg text-badge-purple-text"
                            : "bg-surface-raised text-muted"
                        }`}
                      >
                        {summary.evdoEnabled ? "Enabled" : "Off"}
                      </span>
                    </td>
                    <td className="py-2 text-right font-mono text-xs text-secondary">
                      {summary.servedMobiles != null ? summary.servedMobiles : "-"}
                    </td>
                    <td className="py-2 text-right">
                      <Link
                        href={`/bts/${encodeURIComponent(summary.peerId)}?baseStation=${encodeURIComponent(id)}`}
                        className="text-xs text-accent-green hover:text-accent-green transition-colors"
                      >
                        View &rarr;
                      </Link>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
