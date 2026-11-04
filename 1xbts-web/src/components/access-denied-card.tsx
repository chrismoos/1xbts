"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { BaseStationLink, MobileIdentityLink } from "@/components/activity-links";
import { useBaseStations } from "@/lib/use-base-stations";
import { useMobileDirectory } from "@/lib/mobile-directory";
import { Card } from "@/components/card";
import { StartOtasp } from "@/components/start-otasp";
import { AccessBadge } from "@/components/access-activity";
import { type ObservedMobile, accessClass, createSubscriberHref, reasonLabel } from "@/lib/admission";
import { esnManufacturer } from "@/lib/esn-manufacturer";
import { formatEsn, formatMeid } from "@/lib/format";
import { apiFetch } from "@/lib/api-client";
import { ErrorNotice } from "@/components/error-notice";

const REFRESH_MS = 5_000;
const PAGE_SIZE = 25;

function formatSeen(value?: string): string {
  if (!value) return "-";
  const date = new Date(value);
  const sameDay = date.toDateString() === new Date().toDateString();
  return sameDay ? date.toLocaleTimeString() : date.toLocaleString();
}

/** EV-DO decisions are recorded by Hardware ID alone, so they have no IMSI. */
function isHrpd(mobile: ObservedMobile): boolean {
  return !mobile.imsi;
}

function NetworkBadge({ mobile }: { mobile: ObservedMobile }) {
  return isHrpd(mobile) ? (
    <span className="text-xs px-2 py-0.5 rounded whitespace-nowrap text-accent-cyan bg-accent-cyan/10">
      EV-DO
    </span>
  ) : (
    <span className="text-xs px-2 py-0.5 rounded whitespace-nowrap text-accent-blue bg-accent-blue/10">
      1x
    </span>
  );
}

function Action({ mobile }: { mobile: ObservedMobile }) {
  const kind = accessClass(mobile.admissionClass);
  const subscriber = mobile.subscriber;
  if (subscriber && kind === "provisioning") {
    return (
      <StartOtasp
        subscriberId={subscriber.subscriberId}
        imsi={subscriber.configuredImsi || undefined}
        compact
      />
    );
  }
  if (subscriber) {
    return (
      <Link
        href={`/subscribers/${encodeURIComponent(subscriber.subscriberId)}`}
        className="text-xs px-3 py-1 rounded bg-surface-raised text-secondary border border-border-input whitespace-nowrap"
      >
        View subscriber
      </Link>
    );
  }
  // An EV-DO Hardware ID may be the handset's MEID or its IMEI's first 14
  // digits, and there is no IMSI, so it cannot fill in a subscriber.
  if (kind === "admitted" || isHrpd(mobile)) return null;
  return (
    <Link
      href={createSubscriberHref(mobile)}
      className="text-xs px-3 py-1 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 whitespace-nowrap"
    >
      Create subscriber
    </Link>
  );
}

export function AccessDeniedCard() {
  const { nodes } = useBaseStations();
  const directory = useMobileDirectory();
  const [offset, setOffset] = useState(0);
  const [mobiles, setMobiles] = useState<ObservedMobile[]>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    let cancelled = false;
    const load = () =>
      apiFetch<{ mobiles?: ObservedMobile[]; total?: number }>(
        `/api/mobiles-seen?filter=restricted&limit=${PAGE_SIZE}&offset=${offset}`,
        undefined,
        "load denied mobiles",
      )
        .then((data) => {
          if (cancelled) return;
          setMobiles(data.mobiles ?? []);
          setTotal(data.total ?? 0);
          setError(null);
        })
        .catch((err) => !cancelled && setError(err));
    load();
    const interval = setInterval(load, REFRESH_MS);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [offset]);

  return (
    <Card title={`Access Denied & Provisioning (${total})`}>
      {error ? (
        <ErrorNotice error={error} />
      ) : mobiles.length === 0 ? (
        <p className="text-dimmed text-sm">
          No mobile is currently denied or restricted.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="data-table">
            <thead>
              <tr>
                <th>Identity presented</th>
                <th>Network</th>
                <th>Access</th>
                <th>Reason</th>
                <th>Matched subscriber</th>
                <th>Last base station</th>
                <th>Last attempt</th>
                <th className="text-right">Denials</th>
                <th className="text-right"></th>
              </tr>
            </thead>
            <tbody>
              {mobiles.map((mobile, index) => {
                const reason = mobile.admissionReason;
                return (
                  <tr key={`${mobile.imsi}-${mobile.esn}-${mobile.meid}-${index}`}>
                    <td>
                      <MobileIdentityLink identity={{ ...mobile, subscriberId: mobile.subscriber?.subscriberId }} mobiles={directory}>
                        <div className="font-mono text-secondary group-hover:text-accent-green">
                          {mobile.meid
                            ? isHrpd(mobile)
                              ? formatMeid(mobile.meid)
                              : `MEID ${formatMeid(mobile.meid)}`
                            : mobile.esn != null
                              ? `ESN ${formatEsn(mobile.esn)}`
                              : "-"}
                          {mobile.meid && isHrpd(mobile) && (
                            <span
                              title="This is an MEID or IMEI (first 14 digits)"
                              aria-label="This is an MEID or IMEI (first 14 digits)"
                              className="ml-1 inline-flex h-3.5 w-3.5 items-center justify-center rounded-full border border-border-input align-middle font-sans text-[9px] text-muted cursor-help"
                            >
                              i
                            </span>
                          )}
                        </div>
                        {mobile.meid && mobile.esn != null && (
                          <div className="font-mono text-[11px] text-muted group-hover:text-accent-green">ESN {formatEsn(mobile.esn)}</div>
                        )}
                        {!isHrpd(mobile) && (
                          <div className="font-mono text-[11px] text-muted group-hover:text-accent-green">IMSI {mobile.imsi}</div>
                        )}
                        {mobile.esn != null && esnManufacturer(mobile.esn) && (
                          <div className="text-[11px] text-dimmed group-hover:text-accent-green">{esnManufacturer(mobile.esn)}</div>
                        )}
                      </MobileIdentityLink>
                    </td>
                    <td>
                      <NetworkBadge mobile={mobile} />
                    </td>
                    <td>
                      <AccessBadge value={mobile.admissionClass} />
                    </td>
                    <td className="text-secondary">
                      {reasonLabel(reason)}
                      {accessClass(mobile.admissionClass) === "provisioning" && mobile.subscriber?.configuredImsi && (
                        <div className="text-[11px] text-muted">
                          handset IMSI differs from configured {mobile.subscriber.configuredImsi}
                        </div>
                      )}
                    </td>
                    <td>
                      {mobile.subscriber ? (
                        <div>
                          <Link
                            href={`/subscribers/${encodeURIComponent(mobile.subscriber.subscriberId)}`}
                            className="text-secondary hover:text-accent-green transition-colors"
                          >
                            {mobile.subscriber.displayName || mobile.subscriber.phoneNumber}
                          </Link>
                          <div className="text-[11px] text-muted font-mono">
                            {mobile.subscriber.phoneNumber} · {mobile.subscriber.status.replace("SUBSCRIBER_STATUS_", "").toLowerCase()}
                          </div>
                        </div>
                      ) : (
                        <span className="text-dimmed">-</span>
                      )}
                    </td>
                    <td className="font-mono text-secondary">
                      <BaseStationLink baseStation={mobile.admissionBaseStation} nodes={nodes} />
                    </td>
                    <td className="font-mono text-muted">
                      {formatSeen(mobile.admissionAt)}
                    </td>
                    <td className="text-right font-mono text-secondary">{mobile.deniedCount}</td>
                    <td className="text-right">
                      <Action mobile={mobile} />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      {total > PAGE_SIZE && (
        <div className="flex items-center justify-end mt-2 text-xs text-muted">
          <span className="flex items-center gap-2">
            <button type="button" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))} className="px-2 rounded border border-border-input disabled:text-dimmed">‹</button>
            <span className="font-mono">
              {offset + 1}–{Math.min(total, offset + PAGE_SIZE)} of {total}
            </span>
            <button type="button" disabled={offset + PAGE_SIZE >= total} onClick={() => setOffset(offset + PAGE_SIZE)} className="px-2 rounded border border-border-input disabled:text-dimmed">›</button>
          </span>
        </div>
      )}
    </Card>
  );
}
