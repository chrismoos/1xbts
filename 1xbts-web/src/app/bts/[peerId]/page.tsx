"use client";

import { use, useCallback, useEffect, useMemo, useState } from "react";
import { RxPower } from "@/components/rx-power";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Card, Stat } from "@/components/card";
import { BtsConfigCards } from "@/components/bts-config-cards";
import { IqCaptureCard } from "@/components/iq-capture-card";
import {
  RadioMetricsCards,
  type RadioMetrics,
} from "@/components/radio-metrics-cards";
import {
  attachStateBadgeClass,
  attachStateLabel,
  cellHref,
  cellTitle,
  cellToken,
  peerHref,
  sameCell,
} from "@/lib/cell";
import { esnManufacturer } from "@/lib/esn-manufacturer";
import { formatEsn, formatMeid, formatTimeMs } from "@/lib/format";
import { radioConfigPairName } from "@/lib/radio-config";
import { serviceOptionName } from "@/lib/service-option";
import { useBtsList } from "@/lib/use-bts-list";
import type { BtsConfig, TrafficChannelPower } from "@/lib/proto/bsc/v1/service";
import { CellKind, type ReversePowerControlEntry } from "@/lib/proto/bts_management/v1/service";
import type { FemtoCellStatus } from "@/lib/proto/femto_management/v1/service";

interface ServedMobile {
  address: string;
  state: string;
  esn?: number;
  imsi?: string;
  meid?: string;
  snrDb?: number;
  rxPowerDbm?: number;
  rxLevelDbfs?: number;
  demodQualityPct?: number;
  lastHeardMs?: number;
  phoneNumber?: string;
  subscriberId?: string;
  subscriberDisplayName?: string;
  trafficWalshCode?: number;
  trafficServiceOption?: number;
  servingCell?: { cell: number; sector: number };
}

function formatDb(value?: number): string {
  return value != null ? `${value.toFixed(1)} dB` : "-";
}

function mobileLabel(mobile: ServedMobile): string {
  if (mobile.esn != null) return `ESN ${formatEsn(mobile.esn)}`;
  if (mobile.meid) return `MEID ${formatMeid(mobile.meid)}`;
  return mobile.address;
}

function FemtoLocationMap({ latitude, longitude }: { latitude: number; longitude: number }) {
  const latitudeDelta = 0.008;
  const longitudeDelta = 0.012;
  const bounds = [
    longitude - longitudeDelta,
    latitude - latitudeDelta,
    longitude + longitudeDelta,
    latitude + latitudeDelta,
  ].join(",");
  const marker = `${latitude},${longitude}`;
  const mapUrl = `https://www.openstreetmap.org/export/embed.html?bbox=${encodeURIComponent(bounds)}&layer=mapnik&marker=${encodeURIComponent(marker)}`;
  const detailUrl = `https://www.openstreetmap.org/?mlat=${encodeURIComponent(latitude)}&mlon=${encodeURIComponent(longitude)}#map=15/${encodeURIComponent(latitude)}/${encodeURIComponent(longitude)}`;

  return (
    <div className="mt-4 overflow-hidden rounded-lg border border-border">
      <iframe
        title="Femtocell GPS location"
        src={mapUrl}
        className="h-64 w-full bg-surface-solid"
        loading="lazy"
      />
      <div className="border-t border-border px-3 py-2 text-right">
        <a
          href={detailUrl}
          target="_blank"
          rel="noreferrer"
          className="text-xs text-accent-green hover:text-accent-green"
        >
          Open larger map &rarr;
        </a>
      </div>
    </div>
  );
}

export default function BtsDetailPage({
  params,
}: {
  params: Promise<{ peerId: string }>;
}) {
  const { peerId: rawPeerId } = use(params);
  const peerId = decodeURIComponent(rawPeerId);
  const baseStation = useSearchParams().get("baseStation");

  const { cells } = useBtsList();
  const summary = useMemo(
    () =>
      cells.find(
        (entry) =>
          entry.peerId === peerId &&
          (!baseStation || entry.baseStation === baseStation),
      ),
    [cells, peerId, baseStation],
  );
  // The broadcast identity is present only once the peer is in service.
  const cell = summary?.cell;
  const cellTok = useMemo(() => cellToken(cell), [cell]);

  const [config, setConfig] = useState<BtsConfig | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [metrics, setMetrics] = useState<RadioMetrics | null>(null);
  const [powerControls, setPowerControls] = useState<ReversePowerControlEntry[]>([]);
  const [mobiles, setMobiles] = useState<ServedMobile[]>([]);
  const [powerDrafts, setPowerDrafts] = useState<Record<number, string>>({});
  const [mutatingWalsh, setMutatingWalsh] = useState<number | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const [femtoStatus, setFemtoStatus] = useState<FemtoCellStatus | null>(null);
  const [femtoCommandStatus, setFemtoCommandStatus] = useState<string | null>(null);
  const [femtoCommandPending, setFemtoCommandPending] = useState<string | null>(null);
  const isFemto = summary?.kind === CellKind.CELL_KIND_FEMTOCELL;

  useEffect(() => {
    if (!isFemto || !baseStation) return;
    const query = new URLSearchParams({ baseStation, peerId });
    const load = () => void fetch(`/api/femtocells/status?${query}`, { cache: "no-store" })
      .then((response) => response.json())
      .then((status) => { if (!status.error) setFemtoStatus(status); });
    load();
    const interval = setInterval(load, 5000);
    return () => clearInterval(interval);
  }, [isFemto, baseStation, peerId]);

  const femtoAction = useCallback(async (action: "enable" | "disable" | "sync" | "reboot") => {
    if (!baseStation) return;
    setMutationError(null);
    setFemtoCommandStatus(null);
    setFemtoCommandPending(action);
    try {
      const response = await fetch("/api/femtocells/action", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ baseStation, peerId, action }),
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || "femtocell operation failed");
      setFemtoCommandStatus(result.message || "Command sent");
    } catch (error) {
      setMutationError(error instanceof Error ? error.message : "femtocell operation failed");
    } finally {
      setFemtoCommandPending(null);
    }
  }, [baseStation, peerId]);

  useEffect(() => {
    if (!summary || isFemto) return;
    let cancelled = false;
    fetch(peerHref("/api/bts-config", peerId, baseStation), { cache: "no-store" })
      .then((r) => r.json())
      .then((data: BtsConfig & { error?: string }) => {
        if (cancelled) return;
        if (data.error) {
          setConfigError(data.error);
          setConfig(null);
        } else {
          setConfig(data);
          setConfigError(null);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setConfigError(err instanceof Error ? err.message : "unknown error");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [peerId, baseStation, summary, isFemto]);

  useEffect(() => {
    if (!summary || isFemto) return;
    const es = new EventSource(peerHref("/api/radio-metrics", peerId, baseStation));
    es.onmessage = (event) => {
      const data = JSON.parse(event.data);
      if (!data.error) setMetrics(data);
    };
    return () => es.close();
  }, [peerId, baseStation, summary, isFemto]);

  const loadPowerControls = useCallback(async () => {
    if (!summary || isFemto) return;
    try {
      const res = await fetch(peerHref("/api/bts/reverse-power-controls", peerId, baseStation), {
        cache: "no-store",
      });
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setPowerControls(data.entries ?? []);
    } catch {
      setPowerControls([]);
    }
  }, [peerId, baseStation, summary, isFemto]);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      void loadPowerControls();
      fetch("/api/mobiles", { cache: "no-store" })
        .then((r) => r.json())
        .then((data: ServedMobile[] | { error?: string }) => {
          if (cancelled || !Array.isArray(data)) return;
          setMobiles(data);
        })
        .catch(() => {});
    };
    tick();
    const interval = setInterval(tick, 3000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [loadPowerControls]);

  const servedMobiles = useMemo(
    () => mobiles.filter((mobile) => sameCell(mobile.servingCell, cell)),
    [mobiles, cell],
  );

  const mobileByWalsh = useMemo(() => {
    const map = new Map<number, ServedMobile>();
    for (const mobile of servedMobiles) {
      if (mobile.trafficWalshCode != null) map.set(mobile.trafficWalshCode, mobile);
    }
    return map;
  }, [servedMobiles]);

  const applyOverride = useCallback(
    async (walshCode: number, payload: { targetDb?: number; clear?: boolean }) => {
      setMutatingWalsh(walshCode);
      setMutationError(null);
      try {
        const res = await fetch(cellHref("/api/bts/power-override", cellTok, baseStation), {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ walshCode, ...payload }),
        });
        const json = await res.json();
        if (!res.ok || !json.accepted) {
          throw new Error(json.message || `HTTP ${res.status}`);
        }
        setPowerDrafts((prev) => {
          const next = { ...prev };
          if (json.trafficPower?.effectiveTargetEbNtDb != null) {
            next[walshCode] = json.trafficPower.effectiveTargetEbNtDb.toFixed(1);
          } else if (payload.clear) {
            delete next[walshCode];
          }
          return next;
        });
        await loadPowerControls();
      } catch (err) {
        setMutationError(err instanceof Error ? err.message : "unknown error");
      } finally {
        setMutatingWalsh((current) => (current === walshCode ? null : current));
      }
    },
    [loadPowerControls, cellTok, baseStation],
  );

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <div className="flex flex-wrap items-center gap-3">
        <Link href="/bts" className="text-sm text-muted hover:text-secondary">
          &larr; Cells
        </Link>
        <h1 className="text-lg font-bold">
          {summary?.name || (cell ? cellTitle(cell) : peerId)}
        </h1>
        {summary && (
          <span
            className={`text-xs px-2 py-0.5 rounded ${attachStateBadgeClass(summary.state)}`}
          >
            {attachStateLabel(summary.state)}
          </span>
        )}
        {summary?.statusDetail && (
          <span className="text-xs text-muted">{summary.statusDetail}</span>
        )}
      </div>

      {summary && !cell && summary.managementEndpoint && (
        <Card title="Peer">
          <div className="grid grid-cols-1 md:grid-cols-3 gap-x-6">
            <Stat label="Peer ID" value={peerId} mono />
            <Stat label="Management Endpoint" value={summary.managementEndpoint} mono />
          </div>
        </Card>
      )}

      {cell && summary && (
        <Card title="Cell">
          <div className="grid grid-cols-1 md:grid-cols-3 gap-x-6">
            <Stat label="Pilot PN" value={String(summary.pilotPn)} mono />
            <Stat label="Band Class" value={summary.bandClass || "-"} />
            <Stat label="CDMA Channel" value={String(summary.cdmaChannel)} mono />
            <Stat label="SID" value={String(summary.sid)} mono />
            <Stat label="NID" value={String(summary.nid)} mono />
            <Stat label="Software Version" value={summary.softwareVersion || "-"} mono />
            <Stat label="EV-DO" value={summary.evdoEnabled ? "Enabled" : "Off"} />
            <Stat
              label="Served Mobiles"
              value={summary.servedMobiles != null ? String(summary.servedMobiles) : "-"}
              mono
            />
          </div>
        </Card>
      )}

      {isFemto && femtoStatus && (
        <Card title="Femtocell">
          <div className="grid grid-cols-1 md:grid-cols-3 gap-x-6">
            <Stat label="FEID" value={femtoStatus.feid} mono />
            <Stat label="Profile" value={femtoStatus.profile} />
            <Stat label="Version" value={summary?.softwareVersion || "-"} mono />
            <Stat label="Inner Address" value={femtoStatus.innerPeerAddress || "-"} mono />
            <Stat label="SIP" value={femtoStatus.sipRegistered ? "Registered" : "Not registered"} />
            <Stat label="ACS" value={femtoStatus.acsProvisioned ? "Provisioned" : "Pending"} />
            <Stat label="GPS lock" value={femtoStatus.gpsStatus || "Unknown"} />
            <Stat
              label="Radio timing synchronization"
              value={femtoStatus.timingStatus || "Unknown"}
            />
            <Stat
              label="GPS coordinates"
              value={
                femtoStatus.gpsLatitude != null && femtoStatus.gpsLongitude != null
                  ? `${femtoStatus.gpsLatitude.toFixed(6)}, ${femtoStatus.gpsLongitude.toFixed(6)}`
                  : "Unavailable"
              }
              mono
            />
            <Stat label="Radio" value={femtoStatus.radioOperationalState || "-"} />
          </div>
          {femtoStatus.gpsLatitude != null && femtoStatus.gpsLongitude != null && (
            <FemtoLocationMap
              latitude={femtoStatus.gpsLatitude}
              longitude={femtoStatus.gpsLongitude}
            />
          )}
          {mutationError && <p className="text-accent-red text-sm mt-3">{mutationError}</p>}
          {femtoCommandStatus && (
            <p className="mt-3 rounded border border-accent-green/30 bg-accent-green/10 px-3 py-2 text-sm text-accent-green">
              {femtoCommandStatus}
            </p>
          )}
          <div className="flex flex-wrap gap-2 mt-4">
            {(["sync", "enable", "disable", "reboot"] as const).map((action) => (
              <button
                key={action}
                className="rounded border border-border px-3 py-1.5 text-xs text-secondary hover:border-accent-green hover:text-accent-green"
                disabled={femtoCommandPending !== null}
                onClick={() => void femtoAction(action)}
              >
                {femtoCommandPending === action
                  ? "Sending..."
                  : action === "sync"
                    ? "Force provisioning sync"
                    : `${action[0].toUpperCase()}${action.slice(1)} femtocell`}
              </button>
            ))}
          </div>
        </Card>
      )}

      {configError && (
        <div className="rounded-lg border border-accent-amber/20 bg-accent-amber-bg p-4 text-accent-amber text-sm">
          {configError}
        </div>
      )}

      {!isFemto && config && <BtsConfigCards config={config} />}

      {!isFemto && <IqCaptureCard peerId={peerId} />}

      {!isFemto && <RadioMetricsCards metrics={metrics} />}

      {!isFemto && <Card title={`Reverse Power Control (${powerControls.length})`}>
        {mutationError && (
          <p className="text-accent-red text-sm mb-2">{mutationError}</p>
        )}
        {powerControls.length === 0 ? (
          <p className="text-dimmed text-sm">No active traffic channels.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-muted text-xs">
                  <th className="text-left py-1">Walsh</th>
                  <th className="text-left py-1">Mobile</th>
                  <th className="text-left py-1">Target</th>
                  <th className="text-left py-1">Override</th>
                  <th className="text-right py-1">FER</th>
                  <th className="text-right py-1">Frames</th>
                </tr>
              </thead>
              <tbody>
                {powerControls.map((entry) => (
                  <PowerControlRow
                    key={entry.walshCode}
                    walshCode={entry.walshCode}
                    power={entry.power}
                    mobile={mobileByWalsh.get(entry.walshCode)}
                    draft={powerDrafts[entry.walshCode]}
                    mutating={mutatingWalsh === entry.walshCode}
                    onDraftChange={(value) =>
                      setPowerDrafts((prev) => ({ ...prev, [entry.walshCode]: value }))
                    }
                    onPin={(targetDb) => void applyOverride(entry.walshCode, { targetDb })}
                    onClear={() => void applyOverride(entry.walshCode, { clear: true })}
                    onInvalid={() =>
                      setMutationError(`invalid target for W${entry.walshCode}`)
                    }
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>}

      <Card title={`Mobiles on this Cell (${servedMobiles.length})`}>
        {servedMobiles.length === 0 ? (
          <p className="text-dimmed text-sm">No mobiles on this cell.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="text-muted text-xs">
                  <th className="text-left py-1">Address</th>
                  <th className="text-left py-1">Subscriber</th>
                  <th className="text-left py-1">State</th>
                  <th className="text-left py-1">Traffic</th>
                  <th className="text-right py-1">SNR (dB)</th>
                  <th className="text-right py-1">Rx Level</th>
                  <th className="text-left py-1 pl-4">Last Heard</th>
                  <th className="text-left py-1"></th>
                </tr>
              </thead>
              <tbody>
                {servedMobiles.map((mobile) => (
                  <tr
                    key={mobile.address}
                    className="border-t border-border hover:bg-hover"
                  >
                    <td className="py-2 text-secondary font-mono text-xs">
                      <div>{mobileLabel(mobile)}</div>
                      <div className="text-[11px] text-muted">
                        IMSI {mobile.imsi || "Not Available"}
                      </div>
                      {mobile.esn != null && esnManufacturer(mobile.esn) && (
                        <div className="text-[11px] text-dimmed">
                          {esnManufacturer(mobile.esn)}
                        </div>
                      )}
                    </td>
                    <td className="py-2 text-xs">
                      {mobile.subscriberId ? (
                        <Link
                          href={`/subscribers/${encodeURIComponent(mobile.subscriberId)}`}
                          className="text-secondary hover:text-accent-green transition-colors"
                        >
                          {mobile.subscriberDisplayName || mobile.phoneNumber || "Subscriber"}
                        </Link>
                      ) : (
                        <span className="text-dimmed">-</span>
                      )}
                    </td>
                    <td className="py-2 text-xs text-secondary">{mobile.state}</td>
                    <td className="py-2 font-mono text-xs text-secondary">
                      {mobile.trafficWalshCode != null ? (
                        <span>
                          W{mobile.trafficWalshCode}
                          {mobile.trafficServiceOption != null && (
                            <span className="text-muted ml-1">
                              {serviceOptionName(mobile.trafficServiceOption)}
                            </span>
                          )}
                        </span>
                      ) : (
                        "-"
                      )}
                    </td>
                    <td className="py-2 text-right font-mono text-xs text-secondary">
                      {mobile.snrDb != null ? mobile.snrDb.toFixed(1) : "-"}
                    </td>
                    <td className="py-2 text-right font-mono text-xs text-secondary">
                      <RxPower dbm={mobile.rxPowerDbm} dbfs={mobile.rxLevelDbfs} />
                    </td>
                    <td className="py-2 pl-4 text-muted font-mono text-xs">
                      {mobile.lastHeardMs ? formatTimeMs(mobile.lastHeardMs) : "-"}
                    </td>
                    <td className="py-2 text-right">
                      <Link
                        href={`/mobiles/${encodeURIComponent(mobile.address)}`}
                        className="text-xs text-accent-green hover:text-accent-green transition-colors"
                      >
                        View &rarr;
                      </Link>
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

function PowerControlRow({
  walshCode,
  power,
  mobile,
  draft,
  mutating,
  onDraftChange,
  onPin,
  onClear,
  onInvalid,
}: {
  walshCode: number;
  power?: TrafficChannelPower;
  mobile?: ServedMobile;
  draft?: string;
  mutating: boolean;
  onDraftChange: (value: string) => void;
  onPin: (targetDb: number) => void;
  onClear: () => void;
  onInvalid: () => void;
}) {
  const isRc3 = power?.reverseRadioConfig === 3;
  const metricLabel = isRc3 ? "Pilot SINR" : "Eb/Nt";
  const draftValue = draft ?? power?.effectiveTargetEbNtDb.toFixed(1) ?? "";
  const radioConfigs = radioConfigPairName(
    power?.forwardRadioConfig,
    power?.reverseRadioConfig,
  );

  return (
    <tr className="border-t border-border hover:bg-hover">
      <td className="py-2 font-mono text-xs text-primary">
        <div>W{walshCode}</div>
        {radioConfigs && <div className="text-[11px] text-muted">{radioConfigs}</div>}
      </td>
      <td className="py-2 text-xs">
        {mobile ? (
          <Link
            href={`/mobiles/${encodeURIComponent(mobile.address)}`}
            className="text-accent-green hover:text-accent-green transition-colors"
          >
            {mobile.phoneNumber || mobile.subscriberDisplayName || mobile.address}
          </Link>
        ) : (
          <span className="text-dimmed">-</span>
        )}
      </td>
      <td className="py-2 text-xs text-secondary">
        {power ? (
          <div className="leading-5">
            <div className="text-[10px] uppercase tracking-wide text-muted">
              {metricLabel}
            </div>
            <div className="font-mono text-primary">
              eff {formatDb(power.effectiveTargetEbNtDb)}
            </div>
            <div className="font-mono text-muted">
              auto {formatDb(power.targetEbNtDb)}
            </div>
            <span
              className={`inline-flex px-1.5 py-0.5 rounded text-[10px] ${
                power.manualTargetOverrideDb != null
                  ? "bg-badge-orange-bg text-badge-orange-text"
                  : "bg-surface-raised text-muted"
              }`}
            >
              {power.manualTargetOverrideDb != null ? "Pinned" : "Auto"}
            </span>
          </div>
        ) : (
          <span className="text-dimmed">-</span>
        )}
      </td>
      <td className="py-2 text-xs">
        {power ? (
          <div className="flex items-center gap-2">
            <input
              type="number"
              step="0.1"
              min={isRc3 ? "-20" : "0"}
              max={isRc3 ? "40" : "20"}
              inputMode="decimal"
              value={draftValue}
              onChange={(event) => onDraftChange(event.target.value)}
              disabled={mutating}
              className="w-20 rounded border border-border-input bg-surface-solid px-2 py-1 text-right font-mono text-xs text-primary disabled:opacity-50"
            />
            <button
              type="button"
              onClick={() => {
                const targetDb = Number(draftValue.trim());
                if (!Number.isFinite(targetDb)) {
                  onInvalid();
                  return;
                }
                onPin(targetDb);
              }}
              disabled={mutating}
              className="rounded border border-accent-green/30 px-2 py-1 text-[11px] text-accent-green hover:bg-accent-green-bg disabled:opacity-50"
            >
              Pin
            </button>
            <button
              type="button"
              onClick={onClear}
              disabled={mutating || power.manualTargetOverrideDb == null}
              className="rounded border border-border-input px-2 py-1 text-[11px] text-secondary hover:bg-surface-raised disabled:opacity-50"
            >
              Clear
            </button>
          </div>
        ) : (
          <span className="text-dimmed">-</span>
        )}
      </td>
      <td className="py-2 text-right font-mono text-xs text-secondary">
        {power ? `${power.ferPct.toFixed(1)}%` : "-"}
      </td>
      <td className="py-2 text-right font-mono text-xs text-muted">
        {power ? `${power.framesCrcError}/${power.framesTotal}` : "-"}
      </td>
    </tr>
  );
}
