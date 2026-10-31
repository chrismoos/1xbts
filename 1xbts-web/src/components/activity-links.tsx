import Link from "next/link";
import type { ReactNode } from "react";
import type { BaseStationSummary } from "@/lib/proto/msc_management/v1/service";
import type { MobileDirectoryEntry } from "@/lib/mobile-directory";

export function BaseStationLink({
  baseStation,
  nodes,
}: {
  baseStation?: string;
  nodes: BaseStationSummary[];
}) {
  if (!baseStation) return <>—</>;
  const node = nodes.find(
    (candidate) => candidate.nodeId === baseStation || candidate.managementEndpoint === baseStation,
  );
  return (
    <Link
      href={node ? `/base-stations/${encodeURIComponent(node.managementEndpoint)}` : "/base-stations"}
      className="hover:text-accent-green hover:underline transition-colors"
    >
      {baseStation}
    </Link>
  );
}

export function MobileIdentityLink({
  identity,
  mobiles,
  children,
}: {
  identity: { imsi?: string; esn?: number; meid?: string; subscriberId?: string };
  mobiles: MobileDirectoryEntry[];
  children: ReactNode;
}) {
  const matches = mobiles.filter((candidate) =>
    identity.meid && candidate.meid
      ? identity.meid.toUpperCase() === candidate.meid.toUpperCase()
      : identity.esn != null && candidate.esn != null
        ? identity.esn === candidate.esn && (!identity.imsi || candidate.imsi === identity.imsi)
        : !!identity.imsi && candidate.imsi === identity.imsi,
  );
  const mobile = matches.length === 1 ? matches[0] : undefined;
  const href = mobile
    ? `/mobiles/${encodeURIComponent(mobile.address)}`
    : identity.subscriberId
      ? `/subscribers/${encodeURIComponent(identity.subscriberId)}`
      : undefined;
  return href ? (
    <Link href={href} className="group hover:text-accent-green hover:underline transition-colors">
      {children}
    </Link>
  ) : <>{children}</>;
}
