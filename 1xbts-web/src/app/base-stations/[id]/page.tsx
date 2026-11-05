"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useParams } from "next/navigation";
import { Card } from "@/components/card";
import { attachStateBadgeClass, attachStateLabel } from "@/lib/cell";
import { BtsAttachState, type BtsSummary } from "@/lib/proto/bts_management/v1/service";
import { useBaseStations } from "@/lib/use-base-stations";
import { apiFetch } from "@/lib/api-client";
import { ErrorNotice } from "@/components/error-notice";

export default function BaseStationDetailPage() {
  const raw = useParams().id;
  const id = decodeURIComponent(Array.isArray(raw) ? raw[0] : (raw ?? ""));
  const { nodes } = useBaseStations();
  const node = nodes.find((candidate) => candidate.managementEndpoint === id);

  const [cells, setCells] = useState<BtsSummary[]>([]);
  const [error, setError] = useState<unknown>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      apiFetch<{ bts?: BtsSummary[] }>(
        `/api/bts?baseStation=${encodeURIComponent(id)}`,
        { cache: "no-store" },
        "load cells",
      )
        .then((data) => {
          if (cancelled) return;
          setError(null);
          setCells(data.bts ?? []);
          setLoading(false);
        })
        .catch((err) => {
          if (cancelled) return;
          setError(err);
          setCells([]);
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
    <div className="page-container space-y-6">
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
          <ErrorNotice error={error} />
        ) : cells.length === 0 ? (
          <p className="text-dimmed text-sm">No cells on this base station.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Cell</th>
                  <th>State</th>
                  <th className="text-right">Pilot PN</th>
                  <th>Band Class</th>
                  <th className="text-right">Channel</th>
                  <th className="text-right">SID</th>
                  <th className="text-right">NID</th>
                  <th>EV-DO</th>
                  <th className="text-right">Mobiles</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {cells.map((summary) => (
                  <tr key={summary.peerId}>
                    <td className="font-mono text-primary">
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
                    <td>
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
                    <td className="text-right font-mono text-secondary">
                      {summary.cell ? summary.pilotPn : "-"}
                    </td>
                    <td className="font-mono text-secondary">
                      {summary.bandClass || "-"}
                    </td>
                    <td className="text-right font-mono text-secondary">
                      {summary.cell ? summary.cdmaChannel : "-"}
                    </td>
                    <td className="text-right font-mono text-secondary">
                      {summary.cell ? summary.sid : "-"}
                    </td>
                    <td className="text-right font-mono text-secondary">
                      {summary.cell ? summary.nid : "-"}
                    </td>
                    <td>
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
                    <td className="text-right font-mono text-secondary">
                      {summary.servedMobiles != null ? summary.servedMobiles : "-"}
                    </td>
                    <td className="text-right">
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
