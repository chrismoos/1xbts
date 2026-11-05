"use client";

import Link from "next/link";
import { Card } from "@/components/card";
import { attachStateBadgeClass, attachStateLabel } from "@/lib/cell";
import { BtsAttachState } from "@/lib/proto/bts_management/v1/service";
import { useBtsList } from "@/lib/use-bts-list";
import { ErrorNotice } from "@/components/error-notice";

export default function BtsPage() {
  const { cells, error, loading } = useBtsList();

  const inService = cells.filter(
    (summary) => summary.state === BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE,
  ).length;

  return (
    <div className="page-container space-y-6">
      <div className="flex items-center gap-4">
        <h1 className="text-lg font-bold">Cells</h1>
        {cells.length > 0 && (
          <div className="flex gap-3 text-xs text-muted">
            <span>{cells.length} configured</span>
            <span>{inService} in service</span>
          </div>
        )}
      </div>

      <Card title={`Base Stations (${cells.length})`}>
        {loading ? (
          <p className="text-dimmed text-sm">Loading...</p>
        ) : error ? (
          <ErrorNotice error={error} />
        ) : cells.length === 0 ? (
          <p className="text-dimmed text-sm">No base stations configured.</p>
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
                {cells.map((summary) => {
                  return (
                    <tr
                      key={`${summary.baseStation}-${summary.peerId}`}>
                      <td className="text-primary">
                        <Link
                          href={`/bts/${encodeURIComponent(summary.peerId)}?baseStation=${encodeURIComponent(summary.baseStation)}`}
                          className="hover:text-accent-green transition-colors"
                        >
                          {summary.cell ? (
                            <>
                              <div className="font-medium">
                                {summary.name || `Cell ${summary.cell.cell}`}
                              </div>
                              <div className="text-[11px] text-muted font-mono">
                                {summary.name ? `Cell ${summary.cell.cell}` : `Sector ${summary.cell.sector}`}
                              </div>
                            </>
                          ) : (
                            <>
                              <div className="font-medium">{summary.name || summary.peerId}</div>
                              {summary.managementEndpoint && (
                                <div className="text-[11px] text-muted font-mono">
                                  {summary.managementEndpoint}
                                </div>
                              )}
                            </>
                          )}
                        </Link>
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
                          href={`/bts/${encodeURIComponent(summary.peerId)}?baseStation=${encodeURIComponent(summary.baseStation)}`}
                          className="text-xs text-accent-green hover:text-accent-green transition-colors"
                        >
                          View &rarr;
                        </Link>
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
