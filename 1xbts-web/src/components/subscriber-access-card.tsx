"use client";

import { BaseStationLink, MobileIdentityLink } from "@/components/activity-links";
import { useBaseStations } from "@/lib/use-base-stations";
import { useMobileDirectory } from "@/lib/mobile-directory";
import { Card, Stat } from "@/components/card";
import { AccessBadge, useAdmissionVerdicts } from "@/components/access-activity";
import { StartOtasp } from "@/components/start-otasp";
import { CLASS_DETAIL, accessClass, latestAdmissionForMobile, reasonLabel } from "@/lib/admission";
import { formatEsn, formatMeid } from "@/lib/format";

interface Identity {
  imsi?: string;
  esn?: number;
  meid?: string;
  isPrimary?: boolean;
}

export function SubscriberAccessCard({
  subscriberId,
  identities,
}: {
  subscriberId: string;
  identities: Identity[];
}) {
  const { nodes } = useBaseStations();
  const directory = useMobileDirectory();
  const primary = identities.find((identity) => identity.isPrimary) ?? identities[0];
  const { events, policy } = useAdmissionVerdicts({
    imsis: identities.map((identity) => identity.imsi).filter((imsi): imsi is string => !!imsi),
    esn: primary?.esn,
    meid: primary?.meid,
  }, 0, true);
  if (policy?.accessMode !== "closed") return null;
  const verdict = events.find((event) => event.subscriberId === subscriberId || (!event.subscriberId && identities.some((identity) => latestAdmissionForMobile([event], identity))));
  const kind = accessClass(verdict?.admissionClass);
  const matchedBy = verdict?.meid
    ? `MEID ${formatMeid(verdict.meid)}`
    : verdict?.esn != null
      ? `ESN ${formatEsn(verdict.esn)}`
      : "-";
  return (
    <Card title="Access">
      {!verdict || !kind ? (
        <p className="text-sm text-muted">No access decision since the MSC started.</p>
      ) : (
        <>
          <div className="flex justify-between items-center py-0.5">
            <span className="text-muted text-sm">Latest</span>
            <AccessBadge value={verdict.admissionClass} />
          </div>
          {kind !== "admitted" && <Stat label="Reason" value={reasonLabel(verdict.reason)} />}
          <div className="flex justify-between gap-4 py-0.5">
            <span className="text-muted text-sm shrink-0">{kind === "provisioning" ? "Matched by" : "Device"}</span>
            <span className="text-secondary text-sm font-mono text-right">
              <MobileIdentityLink identity={verdict} mobiles={directory}>{matchedBy}</MobileIdentityLink>
            </span>
          </div>
          <Stat label="Handset" value={`IMSI ${verdict.imsi || "-"}`} mono />
          {verdict.configuredImsi && verdict.configuredImsi !== verdict.imsi && (
            <Stat label="Configured" value={`IMSI ${verdict.configuredImsi}`} mono />
          )}
          <div className="flex justify-between gap-4 py-0.5">
            <span className="text-muted text-sm shrink-0">At</span>
            <span className="text-secondary text-sm font-mono text-right">
              {verdict.time ? new Date(verdict.time).toLocaleTimeString() : "-"}
              {verdict.baseStation && <> · <BaseStationLink baseStation={verdict.baseStation} nodes={nodes} /></>}
            </span>
          </div>
          <p className="text-xs text-muted mt-2">{CLASS_DETAIL[kind]}</p>
          {kind === "provisioning" && (
            <div className="mt-3">
              <StartOtasp subscriberId={subscriberId} imsi={verdict.imsi || undefined} />
            </div>
          )}
        </>
      )}
    </Card>
  );
}
