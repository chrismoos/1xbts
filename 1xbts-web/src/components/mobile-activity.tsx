"use client";

import { useState } from "react";
import { BaseStationLink } from "@/components/activity-links";
import { Card } from "@/components/card";
import {
  AccessBadge,
  AccessMark,
  type AdmissionFilter,
  useAdmissionVerdicts,
} from "@/components/access-activity";
import { AuthenticationMark, useAuthenticationVerdicts } from "@/components/authentication-activity";
import { Pager } from "@/components/pager";
import { useBaseStations } from "@/lib/use-base-stations";
import { procedureName, reasonLabel, serviceLabel, type AdmissionVerdict } from "@/lib/admission";
import { procedureLabel, resultLabel, type AuthVerdict } from "@/lib/authentication-verdict";
import { ErrorNotice } from "@/components/error-notice";

const PAGE_SIZES = [10, 25, 50] as const;
const DEFAULT_PAGE_SIZE = 10;

type View = "all" | "access" | "auth";
type Row =
  | { kind: "access"; time: number; verdict: AdmissionVerdict }
  | { kind: "auth"; time: number; verdict: AuthVerdict };

const VIEWS: { id: View; label: string }[] = [
  { id: "all", label: "All" },
  { id: "access", label: "Access" },
  { id: "auth", label: "Authentication" },
];

function timeOf(value?: string): number {
  const ms = value ? Date.parse(value) : NaN;
  return Number.isFinite(ms) ? ms : 0;
}

export function MobileActivity({ filter }: { filter: AdmissionFilter }) {
  const { nodes } = useBaseStations();
  const access = useAdmissionVerdicts(filter);
  const auth = useAuthenticationVerdicts(filter.imsis ?? [], filter.esn);
  const [view, setView] = useState<View>("all");
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState<number>(DEFAULT_PAGE_SIZE);

  const rows: Row[] = [
    ...(view === "auth" ? [] : access.events.map((verdict) => ({ kind: "access" as const, time: timeOf(verdict.time), verdict }))),
    ...(view === "access" ? [] : auth.events.map((verdict) => ({ kind: "auth" as const, time: timeOf(verdict.time), verdict }))),
  ].sort((a, b) => b.time - a.time);
  const pages = Math.max(1, Math.ceil(rows.length / pageSize));
  const current = Math.min(page, pages - 1);
  const shown = rows.slice(current * pageSize, (current + 1) * pageSize);
  const error = access.error ?? auth.error;

  const viewSwitch = (
    <span className="inline-flex rounded border border-border bg-surface p-0.5">
      {VIEWS.map((v) => (
        <button
          key={v.id}
          type="button"
          onClick={() => {
            setView(v.id);
            setPage(0);
          }}
          className={`rounded px-2.5 py-0.5 text-xs transition-colors ${
            view === v.id ? "bg-surface-raised text-secondary" : "text-muted hover:text-secondary"
          }`}
        >
          {v.label}
        </button>
      ))}
    </span>
  );

  return (
    <Card title="Activity" action={viewSwitch}>
      <ErrorNotice error={error} className="text-xs mb-2" />
      {rows.length === 0 ? (
        <p className="text-xs text-dimmed">
          {view === "auth"
            ? "No authentication activity since the MSC started."
            : access.policy?.accessMode === "closed" || view === "all"
              ? "No access or authentication activity since the MSC started."
              : "Access is open. Decisions are recorded for provisioning refusals only."}
        </p>
      ) : (
        <>
          <div className="overflow-x-auto">
            <table className="data-table">
              <thead>
                <tr>
                  <th className="font-normal">Time</th>
                  <th className="font-normal w-6" />
                  <th className="font-normal">Kind</th>
                  <th className="font-normal">Procedure</th>
                  <th className="font-normal">Result</th>
                  <th className="font-normal">Detail</th>
                  <th className="font-normal">Base station</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((row, index) => (
                  <tr key={`${row.kind}-${row.verdict.time}-${index}`} className="border-border-subtle">
                    <td className="font-mono whitespace-nowrap">
                      {row.verdict.time ? new Date(row.verdict.time).toLocaleTimeString() : "—"}
                    </td>
                    {row.kind === "access" ? (
                      <>
                        <td><AccessMark verdict={row.verdict} /></td>
                        <td>
                          <span className="rounded px-1.5 py-0.5 bg-badge-blue-bg text-badge-blue-text">Access</span>
                        </td>
                        <td className="capitalize">{procedureName(row.verdict.procedure)}</td>
                        <td><AccessBadge value={row.verdict.admissionClass} /></td>
                        <td>
                          {reasonLabel(row.verdict.reason)}
                          {row.verdict.serviceRejected && (
                            <span className="text-muted"> · {serviceLabel(row.verdict.service)} refused</span>
                          )}
                        </td>
                      </>
                    ) : (
                      <>
                        <td><AuthenticationMark verdict={row.verdict} /></td>
                        <td>
                          <span className="rounded px-1.5 py-0.5 bg-badge-purple-bg text-badge-purple-text">Auth</span>
                        </td>
                        <td>{procedureLabel(row.verdict)}</td>
                        <td>{resultLabel(row.verdict)}</td>
                        <td className={`${row.verdict.serviceRejected ? "text-accent-red" : "text-muted"}`}>
                          {row.verdict.serviceRejected ? "service rejected" : "service allowed"}
                        </td>
                      </>
                    )}
                    <td className="text-muted"><BaseStationLink baseStation={row.verdict.baseStation} nodes={nodes} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <Pager
            page={current}
            pageSize={pageSize}
            total={rows.length}
            pageSizes={PAGE_SIZES}
            onPage={setPage}
            onPageSize={(size) => {
              setPageSize(size);
              setPage(0);
            }}
          />
        </>
      )}
    </Card>
  );
}
