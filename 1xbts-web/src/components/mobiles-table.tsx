"use client";

import Link from "next/link";
import { esnManufacturer } from "@/lib/esn-manufacturer";
import { formatEsn, formatMeid, formatTimeMs as formatTime } from "@/lib/format";
import { radioConfigPairName } from "@/lib/radio-config";
import { cellLabel, peerIdForCell } from "@/lib/cell";
import { AccessBadge } from "@/components/access-activity";
import { type AdmissionVerdict, accessClass, reasonLabel } from "@/lib/admission";
import { RxPowerCell, SignalCell } from "@/components/signal-quality";
import type { AccessSignal } from "@/lib/proto/bsc/v1/service";
import type { CellEntry } from "@/lib/use-bts-list";

export interface MobileInfo {
  address: string;
  pageAddress: string;
  state: string;
  admission?: AdmissionVerdict;
  mobPRev: number;
  esn?: number;
  imsi?: string;
  meid?: string;
  imsiMS1?: number;
  imsiMS2?: number;
  pgslot?: number;
  slotCycleIndex: number;
  accessSignal?: AccessSignal;
  lastHeardMs?: number;
  phoneNumber?: string;
  subscriberDisplayName?: string;
  subscriberId?: string;
  trafficWalshCode?: number;
  trafficServiceOption?: number;
  baseStation: string;
  baseStations: string[];
  trafficPower?: {
    forwardRadioConfig: number;
    reverseRadioConfig: number;
  };
  servingCell?: { cell: number; sector: number };
}

function formatMobileLabel(ms: MobileInfo): string {
  const parts: string[] = [];
  if (ms.esn != null) parts.push(`ESN ${formatEsn(ms.esn)}`);
  if (ms.meid) parts.push(`MEID ${formatMeid(ms.meid)}`);
  return parts.length > 0 ? parts.join(" / ") : ms.address;
}

function formatSubscriberId(subscriberId: string): string {
  if (subscriberId.length <= 20) return subscriberId;
  return `${subscriberId.slice(0, 8)}...${subscriberId.slice(-8)}`;
}

export function MobilesTable({
  mobiles,
  cells,
  closed,
}: {
  mobiles: MobileInfo[];
  cells: CellEntry[];
  closed: boolean;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-muted text-xs">
            <th className="text-left py-1">Address</th>
            <th className="text-left py-1">Subscriber</th>
            <th className="text-left py-1">State</th>
            {closed && <th className="text-left py-1">Access</th>}
            <th className="text-left py-1">Cell</th>
            <th className="text-left py-1">Traffic</th>
            <th className="text-left py-1">Signal</th>
            <th className="text-left py-1 pl-4">RX Power</th>
            <th className="text-left py-1 pl-4">Last Heard</th>
            <th className="text-left py-1">SCI</th>
            <th className="text-left py-1"></th>
          </tr>
        </thead>
        <tbody>
          {mobiles.map((ms, i) => {
            const id = encodeURIComponent(ms.address);
            const isHrpd = ms.state.startsWith("HRPD");
            const servingPeerId = peerIdForCell(cells, ms.servingCell);
            return (
              <tr key={i} className="border-t border-border hover:bg-hover">
                <td className="py-2 text-secondary font-mono text-xs">
                  <Link href={`/mobiles/${id}`} className="hover:text-accent-green hover:underline">
                    {formatMobileLabel(ms)}
                  </Link>
                  <div className="text-[11px] text-muted">
                    <div className="font-mono">
                      IMSI {ms.imsi || "Not Available"}
                    </div>
                    {ms.meid && (
                      <div className="font-mono">
                        MEID {formatMeid(ms.meid)}
                      </div>
                    )}
                    {ms.esn != null && esnManufacturer(ms.esn) && (
                      <span className="text-dimmed">{esnManufacturer(ms.esn)}</span>
                    )}
                  </div>
                </td>
                <td className="py-2 text-xs">
                  {ms.subscriberId ? (
                    <div>
                      <Link
                        href={`/subscribers/${encodeURIComponent(ms.subscriberId)}`}
                        className="text-secondary hover:text-accent-green transition-colors"
                      >
                        {ms.subscriberDisplayName || ms.phoneNumber || "Subscriber"}
                      </Link>
                      {ms.subscriberDisplayName && ms.phoneNumber && (
                        <div className="text-muted font-mono text-[11px]">
                          {ms.phoneNumber}
                        </div>
                      )}
                      <div className="text-muted font-mono text-[11px]" title={ms.subscriberId}>
                        {formatSubscriberId(ms.subscriberId)}
                      </div>
                    </div>
                  ) : (
                    <span className="text-dimmed">-</span>
                  )}
                </td>
                <td className="py-2">
                  <span
                    className={`text-xs px-2 py-0.5 rounded ${
                      ms.state === "RegistrationRejected"
                        ? "bg-badge-red-bg text-badge-red-text"
                        : ms.state === "Registering" || ms.state === "Paged"
                          ? "bg-badge-yellow-bg text-badge-yellow-text"
                          : ms.state === "Registered"
                            ? "bg-badge-green-bg text-badge-green-text"
                            : ms.state === "TrafficAssigning" || ms.state === "TrafficActive"
                              ? "bg-badge-purple-bg text-badge-purple-text"
                              : "bg-badge-blue-bg text-badge-blue-text"
                    }`}
                  >
                    {ms.state === "RegistrationRejected"
                      ? "Registration rejected"
                      : ms.state === "NotRegistered"
                        ? "Not registered"
                        : ms.state === "TrafficAssigning"
                          ? "Assigning"
                          : ms.state === "TrafficActive"
                            ? "Traffic active"
                            : ms.state}
                  </span>
                </td>
                {closed && (
                  <td className="py-2">
                    {(() => {
                      const verdict = ms.admission;
                      const kind = accessClass(verdict?.admissionClass);
                      return (
                        <>
                          <AccessBadge value={verdict?.admissionClass} />
                          {kind && kind !== "admitted" && (
                            <div className="text-[11px] text-muted mt-0.5">
                              {kind === "provisioning"
                                ? "hardware match · IMSI differs"
                                : kind === "limited"
                                  ? "suspended · MT SMS only"
                                  : reasonLabel(verdict?.reason)}
                            </div>
                          )}
                        </>
                      );
                    })()}
                  </td>
                )}
                <td className="py-2 font-mono text-xs">
                  {servingPeerId ? (
                    <Link
                      href={`/bts/${encodeURIComponent(servingPeerId)}`}
                      className="text-secondary hover:text-accent-green transition-colors"
                    >
                      {cellLabel(ms.servingCell)}
                    </Link>
                  ) : (
                    <span className="text-dimmed">-</span>
                  )}
                </td>
                <td className="py-2 font-mono text-xs text-secondary">
                  {ms.trafficWalshCode != null ? (
                    <span>
                      {isHrpd ? `A10 ${ms.trafficWalshCode}` : `W${ms.trafficWalshCode}`}
                      {ms.trafficServiceOption != null && (
                        <span className="text-muted ml-1">
                          SO{ms.trafficServiceOption}
                        </span>
                      )}
                      {radioConfigPairName(
                        ms.trafficPower?.forwardRadioConfig,
                        ms.trafficPower?.reverseRadioConfig,
                      ) && (
                        <span className="text-secondary ml-1">
                          {radioConfigPairName(
                            ms.trafficPower?.forwardRadioConfig,
                            ms.trafficPower?.reverseRadioConfig,
                          )}
                        </span>
                      )}
                    </span>
                  ) : "-"}
                </td>
                <td className="py-2">
                  <SignalCell signal={ms.accessSignal} />
                </td>
                <td className="py-2 pl-4">
                  <RxPowerCell signal={ms.accessSignal} />
                </td>
                <td className="py-2 pl-4 text-muted font-mono text-xs">
                  {ms.lastHeardMs ? formatTime(ms.lastHeardMs) : "-"}
                </td>
                <td className="py-2 text-muted text-xs">{ms.slotCycleIndex}</td>
                <td className="py-2 text-right">
                  <Link
                    href={`/mobiles/${id}`}
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
  );
}
