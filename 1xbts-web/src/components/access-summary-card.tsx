"use client";

import Link from "next/link";
import { MobileIdentityLink } from "@/components/activity-links";
import { useLive } from "@/lib/live";
import { useMobileDirectory } from "@/lib/mobile-directory";
import { Card, Stat } from "@/components/card";
import { AccessBadge, useAdmissionVerdicts } from "@/components/access-activity";
import type { ObservedMobile } from "@/lib/admission";
import { accessClass } from "@/lib/admission";
import { formatEsn, formatMeid } from "@/lib/format";

const RECENT = 3;

function identityLabel(event: { esn?: number; meid?: string; imsi?: string }): string {
  if (event.meid) return formatMeid(event.meid);
  if (event.esn != null) return formatEsn(event.esn);
  return event.imsi ?? "-";
}

const NO_MOBILES: ObservedMobile[] = [];

export function AccessSummaryCard() {
  const directory = useMobileDirectory();
  const { events, policy, error } = useAdmissionVerdicts({}, 0, true);
  const closed = policy?.accessMode === "closed";
  const restricted =
    useLive<{ mobiles?: ObservedMobile[] }>(closed ? "/api/mobiles-seen?filter=restricted&limit=500" : null).data?.mobiles ??
    NO_MOBILES;


  const count = (kind: string) => restricted.filter((m) => accessClass(m.admissionClass) === kind).length;
  const limited = new Set(
    events.filter((e) => accessClass(e.admissionClass) === "limited").map((e) => `${e.imsi}/${e.esn}`),
  ).size;
  const recent = events.filter((e) => accessClass(e.admissionClass) !== "admitted" || e.serviceRejected).slice(0, RECENT);

  return (
    <Card title="Access">
      {error ? (
        <p className="text-xs text-accent-red">{error}</p>
      ) : !policy ? (
        <p className="text-xs text-dimmed">Loading...</p>
      ) : (
        <>
          <div className="flex justify-between py-0.5">
            <span className="text-muted text-sm">Mode</span>
            <span
              className={`text-xs px-2 py-0.5 rounded ${
                closed ? "bg-badge-red-bg text-badge-red-text" : "bg-badge-green-bg text-badge-green-text"
              }`}
            >
              {closed ? "Closed" : "Open"}
            </span>
          </div>
          {closed && <Stat label="Registration" value={policy.registerPolicy.replace("_", " ")} />}
          <Stat label="Authentication" value={policy.authenticationMode} />
          {closed && (
            <>
              <div className="grid grid-cols-3 gap-2 my-3">
                <div>
                  <div className="text-xl font-bold text-accent-red">{count("denied")}</div>
                  <div className="text-[11px] text-muted">denied</div>
                </div>
                <div>
                  <div className="text-xl font-bold text-accent-blue">{count("provisioning")}</div>
                  <div className="text-[11px] text-muted">provisioning</div>
                </div>
                <div>
                  <div className="text-xl font-bold text-accent-amber">{limited}</div>
                  <div className="text-[11px] text-muted">limited</div>
                </div>
              </div>
              <div className="border-t border-border-subtle pt-2 space-y-1">
                {recent.length === 0 ? (
                  <p className="text-xs text-dimmed">No refusals since the MSC started.</p>
                ) : (
                  recent.map((event, index) => (
                    <div key={`${event.time}-${index}`} className="flex justify-between items-center text-[11.5px]">
                      <span className="font-mono text-muted">
                        {event.time ? new Date(event.time).toLocaleTimeString() : "-"} · <MobileIdentityLink identity={event} mobiles={directory}>{identityLabel(event)}</MobileIdentityLink>
                      </span>
                      <AccessBadge value={event.admissionClass} />
                    </div>
                  ))
                )}
              </div>
              <Link href="/mobiles" className="block mt-2 text-xs text-accent-green">
                View all &rarr;
              </Link>
            </>
          )}
        </>
      )}
    </Card>
  );
}
