"use client";

import Link from "next/link";
import { Card } from "@/components/card";
import { EvdoSessionTable, useEvdoSessionRows } from "@/components/evdo-session-table";
import { errorBodyOf } from "@/lib/api-client";
import { ErrorNotice } from "@/components/error-notice";

const DASHBOARD_SESSIONS = 3;

export function HrpdSummaryCard() {
  const { rows, error, loading } = useEvdoSessionRows("/api/an-sessions");
  const connected = rows.filter((row) => row.link === "connected").length;
  const dormant = rows.filter((row) => row.link === "dormant").length;

  return (
    <Card title="EV-DO">
      {error ? (
        errorBodyOf(error).reason === "EVDO_NOT_ENABLED" ? (
          <p className="text-dimmed text-sm">Not enabled on this cell</p>
        ) : (
          <ErrorNotice error={error} className="text-xs" />
        )
      ) : (
        <div className="space-y-2">
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted">
            <span>
              <span className="font-mono text-primary">{rows.length}</span> sessions
            </span>
            <span>
              <span className="font-mono text-accent-cyan">{connected}</span> connected
            </span>
            <span>
              <span className="font-mono text-accent-amber">{dormant}</span> dormant
            </span>
          </div>
          {!loading && (
            <div>
              <EvdoSessionTable rows={rows.slice(0, DASHBOARD_SESSIONS)} compact />
            </div>
          )}
        </div>
      )}
      <div className="mt-2 flex items-center justify-between border-t border-border pt-2 text-xs">
        <Link href="/evdo" className="text-accent-green hover:text-accent-green">
          EV-DO sessions →
        </Link>
        {rows.length > DASHBOARD_SESSIONS && <span className="text-muted">+{rows.length - DASHBOARD_SESSIONS} more</span>}
      </div>
    </Card>
  );
}
