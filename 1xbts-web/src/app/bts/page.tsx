"use client";

import Link from "next/link";
import { Card } from "@/components/card";
import { attachStateBadgeClass, attachStateLabel } from "@/lib/cell";
import { BtsAttachState } from "@/lib/proto/bts_management/v1/service";
import { useBtsList } from "@/lib/use-bts-list";

export default function BtsPage() {
  const { cells, error, loading } = useBtsList(3000);

  const inService = cells.filter(
    (summary) => summary.state === BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE,
  ).length;

  return (
    <div className="max-w-7xl mx-auto space-y-6">
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
          <p className="text-accent-red text-sm">{error}</p>
        ) : cells.length === 0 ? (
          <p className="text-dimmed text-sm">No base stations configured.</p>
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
                {cells.map((summary) => {
                  return (
                    <tr
                      key={`${summary.baseStation}-${summary.peerId}`}
                      className="border-t border-border hover:bg-hover"
                    >
                      <td className="py-2 font-mono text-xs text-primary">
                        <Link
                          href={`/bts/${encodeURIComponent(summary.peerId)}?baseStation=${encodeURIComponent(summary.baseStation)}`}
                          className="hover:text-accent-green transition-colors"
                        >
                          {summary.cell ? (
                            <>
                              <div>{`Cell ${summary.cell.cell}`}</div>
                              <div className="text-[11px] text-muted">
                                {`Sector ${summary.cell.sector}`}
                              </div>
                            </>
                          ) : (
                            <>
                              <div>{summary.peerId}</div>
                              {summary.managementEndpoint && (
                                <div className="text-[11px] text-muted">
                                  {summary.managementEndpoint}
                                </div>
                              )}
                            </>
                          )}
                        </Link>
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
