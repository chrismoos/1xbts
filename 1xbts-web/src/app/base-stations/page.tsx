"use client";

import Link from "next/link";
import { Card } from "@/components/card";
import { cellLabel } from "@/lib/cell";
import type { BaseStationSummary } from "@/lib/proto/msc_management/v1/service";
import { useBaseStations } from "@/lib/use-base-stations";

function stateBadge(node: BaseStationSummary) {
  if (node.attached) {
    return (
      <span className="rounded px-1.5 py-0.5 text-[11px] bg-badge-green-bg text-badge-green-text">
        attached
      </span>
    );
  }
  return (
    <span
      className="rounded px-1.5 py-0.5 text-[11px] bg-surface-raised text-muted"
      title={node.statusDetail ?? undefined}
    >
      {node.nodeId ? "detached" : "not enrolled"}
    </span>
  );
}

export default function BaseStationsPage() {
  const { nodes, error, loading } = useBaseStations(3000);
  const attached = nodes.filter((node) => node.attached).length;

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <div className="flex items-center gap-4">
        <h1 className="text-lg font-bold">Base Stations</h1>
        {nodes.length > 0 && (
          <div className="flex gap-3 text-xs text-muted">
            <span>{nodes.length} configured</span>
            <span>{attached} attached</span>
          </div>
        )}
      </div>

      <Card title={`Nodes the MSC serves (${nodes.length})`}>
        {loading ? (
          <p className="text-dimmed text-sm">Loading...</p>
        ) : error ? (
          <p className="text-accent-red text-sm">{error}</p>
        ) : nodes.length === 0 ? (
          <p className="text-dimmed text-sm">No base stations configured on the MSC.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-muted text-xs">
                  <th className="text-left py-1">Node</th>
                  <th className="text-left py-1">State</th>
                  <th className="text-left py-1 pl-4">Management</th>
                  <th className="text-left py-1 pl-4">A1</th>
                  <th className="text-left py-1 pl-4">Cells</th>
                </tr>
              </thead>
              <tbody>
                {nodes.map((node) => (
                  <tr
                    key={node.managementEndpoint}
                    className="border-t border-border-subtle"
                  >
                    <td className="py-1.5 font-mono text-primary">
                      <Link
                        href={`/base-stations/${encodeURIComponent(node.managementEndpoint)}`}
                        className="text-accent-green hover:underline"
                      >
                        {node.nodeId || node.managementEndpoint}
                      </Link>
                    </td>
                    <td className="py-1.5">
                      {stateBadge(node)}
                      {!node.attached && node.statusDetail && (
                        <span className="ml-2 text-xs text-muted">{node.statusDetail}</span>
                      )}
                    </td>
                    <td className="py-1.5 pl-4 font-mono text-xs text-secondary">
                      {node.managementEndpoint}
                    </td>
                    <td className="py-1.5 pl-4 font-mono text-xs text-secondary">
                      {node.a1Addr || "-"}
                    </td>
                    <td className="py-1.5 pl-4 text-xs">
                      {node.cells.length === 0 ? (
                        <span className="text-dimmed">none</span>
                      ) : (
                        <div className="flex flex-wrap gap-1">
                          {node.cells.map((served) => {
                            const label = `${cellLabel(served.cell)} SID ${served.sid} NID ${served.nid}`;
                            return (
                              <span
                                key={label}
                                className={`rounded px-1.5 py-0.5 font-mono ${
                                  served.inService && node.attached
                                    ? "bg-badge-green-bg text-badge-green-text"
                                    : "bg-surface-raised text-muted"
                                }`}
                                title={label}
                              >
                                {cellLabel(served.cell)}
                              </span>
                            );
                          })}
                        </div>
                      )}
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
