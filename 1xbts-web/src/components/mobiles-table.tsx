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
import { evdoLinkState, type EvdoSummary } from "@/lib/evdo";

const EVDO_LINK_LABEL = { connected: "connected", dormant: "dormant", idle: "idle" } as const;

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
  evdo?: EvdoSummary;
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
  hideCell = false,
}: {
  mobiles: MobileInfo[];
  cells: CellEntry[];
  closed: boolean;
  hideCell?: boolean;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="data-table">
        <thead>
          <tr>
            <th>Address</th>
            <th>Subscriber</th>
            <th>State</th>
            {closed && <th>Access</th>}
            {!hideCell && <th>Cell</th>}
            <th>Traffic</th>
            <th>Signal</th>
            <th>RX Power</th>
            <th>Last Heard</th>
            <th>SCI</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {mobiles.map((ms, i) => {
            const id = encodeURIComponent(ms.address);
            const isHrpd = ms.state.startsWith("EV-DO");
            const servingPeerId = peerIdForCell(cells, ms.servingCell);
            return (
              <tr key={i}>
                <td className="text-secondary font-mono">
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
                <td>
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
                <td>
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
                  {ms.evdo && !isHrpd && (
                    <div className="mt-1">
                      <span className="text-[11px] px-1.5 py-0.5 rounded bg-accent-purple-bg text-accent-purple">
                        EV-DO {EVDO_LINK_LABEL[evdoLinkState(ms.evdo.connected, ms.evdo.packetPhase)]}
                      </span>
                    </div>
                  )}
                  {ms.evdo?.verdict === "refused" && (
                    <div className="mt-0.5 text-[11px] text-accent-red">
                      EV-DO refused{ms.evdo.refusedReason ? ` · ${reasonLabel(ms.evdo.refusedReason)}` : ""}
                    </div>
                  )}
                </td>
                {closed && (
                  <td>
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
                {!hideCell && (
                  <td className="font-mono">
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
                )}
                <td className="font-mono text-secondary">
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
                <td>
                  <SignalCell signal={ms.accessSignal} />
                </td>
                <td>
                  <RxPowerCell signal={ms.accessSignal} />
                </td>
                <td className="text-muted font-mono">
                  {ms.lastHeardMs ? formatTime(ms.lastHeardMs) : "-"}
                </td>
                <td className="text-muted">{ms.slotCycleIndex}</td>
                <td className="text-right">
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
