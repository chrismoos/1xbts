"use client";

import { use, useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { esnManufacturer } from "@/lib/esn-manufacturer";
import { formatEsn, formatMeid } from "@/lib/format";
import { Card, Stat } from "@/components/card";
import { RecentMessagesCard } from "@/components/recent-messages-card";
import { RecentOtaspCard } from "@/components/recent-otasp-card";
import { OtaspOverridesCard } from "@/components/otasp-overrides-card";
import { ImsiGenerateButton } from "@/components/imsi-generate-button";
import { validateRingtoneFile } from "@/lib/validation";
import {
  NumberPlan,
  NumberType,
  type RegistrationBinding,
  type Subscriber,
  type SubscriberIdentity,
} from "@/lib/proto/hlr/v1/service";
import {
  DEFAULT_NUMBER_PLAN,
  DEFAULT_NUMBER_TYPE,
  NUMBER_PLAN_OPTIONS,
  NUMBER_TYPE_OPTIONS,
  normalizeNumberPlan,
  normalizeNumberType,
} from "@/lib/subscriber-options";
import {
  IMSI_DIGITS,
  PHONE_MAX_DIGITS,
  validateImsi,
  validateMeid,
  validatePhoneNumber,
} from "@/lib/validation";

// Data Session card is hidden for now; set to true to bring it back.
const SHOW_DATA_SESSION = false;

type FieldErrors = {
  phoneNumber?: string;
  imsi?: string;
  esn?: string;
  meid?: string;
  callerNumber?: string;
  form?: string;
};

type UiSubscriber = Omit<Subscriber, "status"> & { status: string };

type SubscriberDetail = {
  subscriber?: UiSubscriber;
  identities: SubscriberIdentity[];
  binding?: RegistrationBinding;
  error?: string;
};

function formatTimestamp(
  ts?: Date | string | { seconds: number; nanos: number } | undefined
): string {
  if (!ts) return "-";
  if (ts instanceof Date || typeof ts === "string") {
    const date = new Date(ts);
    return Number.isNaN(date.getTime()) ? "-" : date.toLocaleString();
  }
  const millis = ts.seconds * 1000 + Math.floor(ts.nanos / 1_000_000);
  const date = new Date(millis);
  return Number.isNaN(date.getTime()) ? "-" : date.toLocaleString();
}

function formatBindingIdentity(binding?: RegistrationBinding): string {
  if (!binding) return "-";
  return [
    binding.esn != null ? `ESN ${formatEsn(binding.esn)}` : null,
    binding.meid ? `MEID ${formatMeid(binding.meid)}` : null,
    binding.imsi ? `IMSI ${binding.imsi}` : null,
  ]
    .filter(Boolean)
    .join(" / ") || "-";
}

export default function SubscriberDetailPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  const router = useRouter();
  const [detail, setDetail] = useState<SubscriberDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [saveResult, setSaveResult] = useState<string | null>(null);
  const [calling, setCalling] = useState(false);
  const [callAudioFile, setCallAudioFile] = useState("");
  const [callerNumber, setCallerNumber] = useState("");
  const [callResult, setCallResult] = useState<string | null>(null);
  const [startingData, setStartingData] = useState(false);
  const [dataResult, setDataResult] = useState<string | null>(null);
  const [dataSo, setDataSo] = useState(33);
  const [ringtoneFile, setRingtoneFile] = useState<File | null>(null);
  const [ringtoneBusy, setRingtoneBusy] = useState(false);
  const [ringtoneResult, setRingtoneResult] = useState<string | null>(null);

  const primaryIdentity = useMemo(
    () =>
      detail?.identities.find((identity) => identity.isPrimary) ??
      detail?.identities?.[0],
    [detail]
  );

  const [phoneNumber, setPhoneNumber] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [status, setStatus] = useState("active");
  const [esnHex, setEsnHex] = useState("");
  const [imsi, setImsi] = useState("");
  const [meid, setMeid] = useState("");
  const [numberType, setNumberType] = useState<NumberType>(DEFAULT_NUMBER_TYPE);
  const [numberPlan, setNumberPlan] = useState<NumberPlan>(DEFAULT_NUMBER_PLAN);
  const [fieldErrors, setFieldErrors] = useState<FieldErrors>({});

  const clearFieldError = (field: keyof FieldErrors) =>
    setFieldErrors((prev) => {
      if (prev[field] === undefined && prev.form === undefined) return prev;
      const next = { ...prev };
      delete next[field];
      delete next.form;
      return next;
    });

  const fetchDetail = useCallback(async () => {
    try {
      const res = await fetch(`/api/subscribers/${encodeURIComponent(id)}`);
      const data: SubscriberDetail = await res.json();
      if (!res.ok || data.error) {
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      setDetail(data);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown error");
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => {
    fetchDetail();
  }, [fetchDetail]);

  useEffect(() => {
    const subscriber = detail?.subscriber;
    if (!subscriber) return;
    setPhoneNumber(subscriber.phoneNumber);
    setDisplayName(subscriber.displayName || "");
    setStatus(subscriber.status || "active");
    setNumberType(normalizeNumberType(subscriber.numberType));
    setNumberPlan(normalizeNumberPlan(subscriber.numberPlan));

    setEsnHex(
      primaryIdentity?.esn != null
        ? (primaryIdentity.esn >>> 0).toString(16).toUpperCase().padStart(8, "0")
        : ""
    );
    setImsi(primaryIdentity?.imsi ?? "");
    setMeid(primaryIdentity?.meid ?? "");
  }, [detail, primaryIdentity]);

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setSaveResult(null);
    setSaving(true);

    try {
      const normalizedPhoneNumber = phoneNumber.trim();
      const normalizedEsn = esnHex.trim();
      const normalizedImsi = imsi.trim();
      const normalizedMeid = meid.trim().toLowerCase();
      const errors: FieldErrors = {};

      const phoneCheck = validatePhoneNumber(normalizedPhoneNumber);
      if (!phoneCheck.ok) errors.phoneNumber = phoneCheck.error;

      let esnValue: number | undefined;
      if (normalizedEsn) {
        esnValue = parseInt(normalizedEsn, 16);
        if (isNaN(esnValue)) errors.esn = "ESN must be valid hexadecimal";
      }

      if (normalizedImsi) {
        const imsiCheck = validateImsi(normalizedImsi);
        if (!imsiCheck.ok) errors.imsi = imsiCheck.error;
      }

      if (normalizedMeid) {
        const meidCheck = validateMeid(normalizedMeid);
        if (!meidCheck.ok) errors.meid = meidCheck.error;
      }

      if (!errors.esn && !errors.imsi && !errors.meid) {
        if (!normalizedImsi || (!normalizedEsn && !normalizedMeid)) {
          errors.form = "Enter IMSI plus ESN or MEID";
        }
      }

      if (Object.keys(errors).length > 0) {
        setFieldErrors(errors);
        return;
      }
      setFieldErrors({});

      const body: Record<string, unknown> = {
        phoneNumber: normalizedPhoneNumber,
        displayName,
        status,
        numberType,
        numberPlan,
      };
      if (esnValue !== undefined) body.esn = esnValue;
      if (normalizedImsi) body.imsi = normalizedImsi;
      if (normalizedMeid) body.meid = normalizedMeid;

      const res = await fetch(`/api/subscribers/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      const data = await res.json();
      if (!res.ok || data.error) {
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      setSaveResult("Saved");
      await fetchDetail();
    } catch (err) {
      setFieldErrors({
        form: err instanceof Error ? err.message : "unknown error",
      });
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!detail?.subscriber) return;
    setDeleting(true);
    setError(null);
    try {
      const res = await fetch(
        `/api/subscribers?id=${encodeURIComponent(detail.subscriber.subscriberId)}`,
        { method: "DELETE" }
      );
      const data = await res.json();
      if (!res.ok || data.error) {
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      router.push("/subscribers");
    } catch (err) {
      setError(err instanceof Error ? err.message : "unknown error");
      setDeleting(false);
    }
  };

  const handleCall = async () => {
    if (!detail?.subscriber) return;
    setCallResult(null);
    const trimmedCaller = callerNumber.trim();
    if (trimmedCaller) {
      const check = validatePhoneNumber(trimmedCaller);
      if (!check.ok) {
        setFieldErrors((prev) => ({ ...prev, callerNumber: check.error }));
        return;
      }
    }
    setFieldErrors((prev) => {
      if (prev.callerNumber === undefined) return prev;
      const next = { ...prev };
      delete next.callerNumber;
      return next;
    });
    setCalling(true);
    try {
      const res = await fetch("/api/calls", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          subscriberId: detail.subscriber.subscriberId,
          audioFile: callAudioFile.trim() || undefined,
          callerNumber: trimmedCaller || undefined,
        }),
      });
      const data = await res.json();
      if (!res.ok || !data.accepted) {
        throw new Error(data.message || `HTTP ${res.status}`);
      }
      setCallResult(data.message || "Call requested");
    } catch (err) {
      setCallResult(err instanceof Error ? err.message : "unknown error");
    } finally {
      setCalling(false);
    }
  };

  const handleRingtoneUpload = async () => {
    if (!ringtoneFile) return;
    const validation = validateRingtoneFile(ringtoneFile);
    if (!validation.ok) {
      setRingtoneResult(validation.error);
      return;
    }
    setRingtoneBusy(true);
    setRingtoneResult(null);
    try {
      const form = new FormData();
      form.append("file", ringtoneFile);
      const res = await fetch(
        `/api/subscribers/${encodeURIComponent(id)}/ringtone`,
        { method: "POST", body: form }
      );
      const data = await res.json();
      if (!res.ok || data.error) {
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      setRingtoneResult(
        `Uploaded (${data.codecs?.length ?? 0} codecs, ${(data.durationMs ?? 0) / 1000}s)`
      );
      setRingtoneFile(null);
      await fetchDetail();
    } catch (err) {
      setRingtoneResult(err instanceof Error ? err.message : "unknown error");
    } finally {
      setRingtoneBusy(false);
    }
  };

  const handleRingtoneClear = async () => {
    setRingtoneBusy(true);
    setRingtoneResult(null);
    try {
      const res = await fetch(
        `/api/subscribers/${encodeURIComponent(id)}/ringtone`,
        { method: "DELETE" }
      );
      if (!res.ok && res.status !== 204) {
        const data = await res.json().catch(() => ({}));
        throw new Error(data.error || `HTTP ${res.status}`);
      }
      setRingtoneResult("Removed");
      await fetchDetail();
    } catch (err) {
      setRingtoneResult(err instanceof Error ? err.message : "unknown error");
    } finally {
      setRingtoneBusy(false);
    }
  };

  const handleDataCall = async () => {
    if (!detail?.subscriber) return;
    setStartingData(true);
    setDataResult(null);
    try {
      const res = await fetch("/api/data-call", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          subscriberId: detail.subscriber.subscriberId,
          serviceOption: dataSo,
        }),
      });
      const data = await res.json();
      if (!res.ok || !data.accepted) {
        throw new Error(data.message || `HTTP ${res.status}`);
      }
      setDataResult(data.message || "Data session requested");
    } catch (err) {
      setDataResult(err instanceof Error ? err.message : "unknown error");
    } finally {
      setStartingData(false);
    }
  };

  if (loading) {
    return <div className="max-w-7xl mx-auto text-sm text-muted">Loading...</div>;
  }

  if (error || !detail?.subscriber) {
    return (
      <div className="max-w-7xl mx-auto space-y-4">
        <Link
          href="/subscribers"
          className="text-sm text-muted hover:text-primary"
        >
          &larr; Subscribers
        </Link>
        <div className="rounded-lg border border-accent-red/20 bg-accent-red-bg p-4 text-sm text-accent-red">
          {error || "Subscriber not found"}
        </div>
      </div>
    );
  }

  const subscriber = detail.subscriber;
  const binding = detail.binding;

  return (
    <div className="max-w-7xl mx-auto space-y-6">
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <Link
            href="/subscribers"
            className="text-sm text-muted hover:text-primary"
          >
            &larr; Subscribers
          </Link>
          <h1 className="text-lg font-bold font-mono mt-2">{subscriber.phoneNumber}</h1>
          <div className="text-xs text-muted">
            {subscriber.displayName || "Unnamed subscriber"}
          </div>
        </div>
        <button
          onClick={handleDelete}
          disabled={deleting}
          className="text-xs px-3 py-1.5 rounded bg-accent-red-bg text-accent-red border border-accent-red/20 hover:bg-accent-red/15 disabled:opacity-50 transition-colors"
        >
          {deleting ? "Deleting..." : "Delete Subscriber"}
        </button>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <Card title="Subscriber">
          <Stat label="Phone Number" value={subscriber.phoneNumber} mono />
          <Stat label="Display Name" value={subscriber.displayName || "-"} />
          <Stat label="Status" value={subscriber.status} />
          <Stat label="Subscriber ID" value={subscriber.subscriberId} mono />
          <Stat label="Created" value={formatTimestamp(subscriber.createdAt)} mono />
          <Stat label="Updated" value={formatTimestamp(subscriber.updatedAt)} mono />
        </Card>

        <Card title="Registration Binding">
          {binding ? (
            <>
              <Stat label="State" value={binding.state} />
              <Stat label="Serving Node" value={binding.servingNodeId} mono />
              <Stat label="Identity" value={formatBindingIdentity(binding)} mono />
              <Stat label="PGSLOT" value={binding.pgslot != null ? String(binding.pgslot) : "-"} mono />
              <Stat
                label="Slot Cycle Index"
                value={
                  binding.slotCycleIndex != null
                    ? String(binding.slotCycleIndex)
                    : "-"
                }
                mono
              />
              <Stat
                label="Last MSG_SEQ"
                value={binding.lastMsgSeq != null ? String(binding.lastMsgSeq) : "-"}
                mono
              />
              <Stat label="Last Registered" value={formatTimestamp(binding.lastRegisteredAt)} mono />
              <Stat label="Last Seen" value={formatTimestamp(binding.lastSeenAt)} mono />
            </>
          ) : (
            <p className="text-sm text-muted">No active registration binding.</p>
          )}
        </Card>

        <Card title={`Identities (${detail.identities.length})`}>
          {detail.identities.length === 0 ? (
            <p className="text-sm text-muted">No identities provisioned.</p>
          ) : (
            <div className="space-y-3">
              {detail.identities.map((identity) => (
                <div
                  key={identity.subscriberIdentityId}
                  className="rounded border border-border p-3 text-xs space-y-1"
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-mono text-muted">
                      {identity.subscriberIdentityId.slice(0, 8)}...
                    </span>
                    {identity.isPrimary && (
                      <span className="rounded bg-badge-green-bg px-2 py-0.5 text-badge-green-text">
                        Primary
                      </span>
                    )}
                  </div>
                  {identity.esn != null && (
                    <div className="font-mono text-secondary">
                      ESN {formatEsn(identity.esn)}
                      {esnManufacturer(identity.esn) && (
                        <span className="ml-2 text-muted font-sans">{esnManufacturer(identity.esn)}</span>
                      )}
                    </div>
                  )}
                  <div className="font-mono text-secondary">
                    IMSI {identity.imsi || "Not Available"}
                  </div>
                  {identity.meid && (
                    <div className="font-mono text-secondary">
                      MEID {formatMeid(identity.meid)}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </Card>
      </div>

      <RecentMessagesCard phone={subscriber.phoneNumber} />

      <RecentOtaspCard subscriberId={subscriber.subscriberId} />

      <OtaspOverridesCard
        subscriberId={subscriber.subscriberId}
        prlOverride={subscriber.prlOverrideId ?? undefined}
        spc={subscriber.serviceProgrammingCode ?? undefined}
        analogControlChannel={subscriber.firstchpOverride ?? undefined}
        onChanged={fetchDetail}
      />

      <Card title="Voice Call">
        <div className="space-y-4">
          <div className="grid grid-cols-1 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] gap-4 items-end">
            <div>
              <label className="block text-xs text-muted mb-1" htmlFor="caller-number">
                Caller ID Number
              </label>
              <input
                id="caller-number"
                type="text"
                value={callerNumber}
                onChange={(e) => {
                  setCallerNumber(e.target.value);
                  setFieldErrors((prev) => {
                    if (prev.callerNumber === undefined) return prev;
                    const next = { ...prev };
                    delete next.callerNumber;
                    return next;
                  });
                }}
                inputMode="numeric"
                maxLength={PHONE_MAX_DIGITS}
                placeholder="0000000000"
                aria-invalid={!!fieldErrors.callerNumber}
                aria-describedby={
                  fieldErrors.callerNumber ? "caller-number-error" : undefined
                }
                className="w-full glass-input font-mono"
              />
              {fieldErrors.callerNumber && (
                <p id="caller-number-error" className="text-accent-red text-xs mt-1">
                  {fieldErrors.callerNumber}
                </p>
              )}
            </div>
            <div>
              <label className="block text-xs text-muted mb-1">
                Optional WAV Override
              </label>
              <input
                type="text"
                value={callAudioFile}
                onChange={(e) => setCallAudioFile(e.target.value)}
                placeholder="Use server default when blank"
                className="w-full glass-input font-mono"
              />
            </div>
            <button
              type="button"
              onClick={handleCall}
              disabled={calling || !binding}
              className="text-xs px-4 py-2 rounded bg-accent-blue hover:bg-accent-blue/80 text-primary disabled:opacity-50 transition-colors"
            >
              {calling ? "Calling..." : "Start Call"}
            </button>
          </div>
          <p className="text-xs text-muted">
            Places a BS-originated voice call to this subscriber. The subscriber must
            have an active registration binding.
          </p>
          {callResult && (
            <p
              className={`text-xs ${
                callResult.toLowerCase().includes("accepted")
                  ? "text-accent-green"
                  : "text-accent-red"
              }`}
            >
              {callResult}
            </p>
          )}
        </div>
      </Card>

      {/* Data Session card hidden for now. */}
      {SHOW_DATA_SESSION && (
        <Card title="Data Session">
          <div className="space-y-4">
            <div className="grid grid-cols-1 md:grid-cols-[minmax(0,1fr)_auto] gap-4 items-end">
            <div>
              <label className="block text-xs text-muted mb-1">
                Service Option
              </label>
              <select
                value={dataSo}
                onChange={(e) => setDataSo(Number(e.target.value))}
                className="w-full glass-input font-mono"
              >
                <option value={33}>SO 33 — High-Rate Packet (IS-707)</option>
                <option value={12}>SO 12 — Asynchronous Data</option>
                <option value={7}>SO 7 — Packet Data</option>
              </select>
            </div>
            <button
              type="button"
              onClick={handleDataCall}
              disabled={startingData || !binding}
              className="text-xs px-4 py-2 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:opacity-50 transition-colors"
            >
              {startingData ? "Starting..." : "Start Data Session"}
            </button>
          </div>
          <p className="text-xs text-muted">
            Pages the mobile and assigns a traffic channel with the selected packet data
            service option. RLP/PPP negotiation starts automatically on Service Connect.
          </p>
          {dataResult && (
            <p
              className={`text-xs ${
                dataResult.toLowerCase().includes("accepted")
                  ? "text-accent-green"
                  : "text-accent-red"
              }`}
            >
              {dataResult}
            </p>
          )}
        </div>
        </Card>
      )}

      <Card title="Custom Ringtone">
        <div className="space-y-3">
          {subscriber.hasRingtone ? (
            <div className="text-xs space-y-1">
              <div className="text-secondary">
                A custom ringtone is stored
                {subscriber.ringtoneDurationMs != null && (
                  <span className="text-muted">
                    {" "}
                    ({(subscriber.ringtoneDurationMs / 1000).toFixed(1)}s)
                  </span>
                )}
                .
              </div>
              <div className="text-muted">
                Encoded for all supported codecs (EVRC-A, EVRC-B, EVRC-WB).
              </div>
            </div>
          ) : (
            <div className="text-xs text-muted">
              No custom ringtone uploaded. Default ringback tone will be played.
            </div>
          )}

          <div className="flex flex-wrap items-end gap-3">
            <div>
              <label className="block text-xs text-muted mb-1">
                WAV File (max 4 MB)
              </label>
              <input
                type="file"
                accept=".wav,audio/wav,audio/x-wav"
                onChange={(e) => setRingtoneFile(e.target.files?.[0] ?? null)}
                className="text-xs"
              />
            </div>
            <button
              type="button"
              onClick={handleRingtoneUpload}
              disabled={ringtoneBusy || !ringtoneFile}
              className="text-xs px-4 py-2 rounded bg-accent-blue hover:bg-accent-blue/80 text-primary disabled:opacity-50 transition-colors"
            >
              {ringtoneBusy ? "Working..." : subscriber.hasRingtone ? "Replace" : "Upload"}
            </button>
            {subscriber.hasRingtone && (
              <button
                type="button"
                onClick={handleRingtoneClear}
                disabled={ringtoneBusy}
                className="text-xs px-3 py-2 rounded bg-accent-red-bg text-accent-red border border-accent-red/20 hover:bg-accent-red/15 disabled:opacity-50 transition-colors"
              >
                Remove
              </button>
            )}
          </div>
          {ringtoneResult && (
            <p
              className={`text-xs ${
                ringtoneResult.toLowerCase().includes("error") ||
                ringtoneResult.toLowerCase().includes("fail") ||
                ringtoneResult.toLowerCase().includes("must") ||
                ringtoneResult.toLowerCase().includes("not")
                  ? "text-accent-red"
                  : "text-accent-green"
              }`}
            >
              {ringtoneResult}
            </p>
          )}
        </div>
      </Card>

      <Card title="Edit Subscriber">
        <form onSubmit={handleSave} className="space-y-4" noValidate>
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            <div>
              <label className="block text-xs text-muted mb-1" htmlFor="edit-phone-number">
                Phone Number
              </label>
              <input
                id="edit-phone-number"
                type="text"
                value={phoneNumber}
                onChange={(e) => {
                  setPhoneNumber(e.target.value);
                  clearFieldError("phoneNumber");
                }}
                inputMode="numeric"
                maxLength={PHONE_MAX_DIGITS}
                aria-invalid={!!fieldErrors.phoneNumber}
                aria-describedby={
                  fieldErrors.phoneNumber ? "edit-phone-number-error" : undefined
                }
                className="w-full glass-input font-mono"
              />
              {fieldErrors.phoneNumber && (
                <p id="edit-phone-number-error" className="text-accent-red text-xs mt-1">
                  {fieldErrors.phoneNumber}
                </p>
              )}
            </div>
            <div>
              <label className="block text-xs text-muted mb-1">Display Name</label>
              <input
                type="text"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                className="w-full glass-input"
              />
            </div>
            <div>
              <label className="block text-xs text-muted mb-1">Status</label>
              <select
                value={status}
                onChange={(e) => setStatus(e.target.value)}
                className="w-full glass-input"
              >
                <option value="active">active</option>
                <option value="suspended">suspended</option>
                <option value="disabled">disabled</option>
              </select>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            <div>
              <label className="block text-xs text-muted mb-1" htmlFor="edit-esn">
                ESN (hex)
              </label>
              <input
                id="edit-esn"
                type="text"
                value={esnHex}
                onChange={(e) => {
                  setEsnHex(e.target.value);
                  clearFieldError("esn");
                }}
                maxLength={8}
                aria-invalid={!!fieldErrors.esn}
                aria-describedby={fieldErrors.esn ? "edit-esn-error" : undefined}
                className="w-full glass-input font-mono"
              />
              {fieldErrors.esn && (
                <p id="edit-esn-error" className="text-accent-red text-xs mt-1">
                  {fieldErrors.esn}
                </p>
              )}
            </div>
            <div>
              <div className="flex items-end justify-between mb-1">
                <label className="block text-xs text-muted" htmlFor="edit-imsi">
                  IMSI
                </label>
                <ImsiGenerateButton
                  phoneNumber={phoneNumber}
                  onGenerated={(v) => {
                    setImsi(v);
                    clearFieldError("imsi");
                  }}
                />
              </div>
              <input
                id="edit-imsi"
                type="text"
                value={imsi}
                onChange={(e) => {
                  setImsi(e.target.value);
                  clearFieldError("imsi");
                }}
                inputMode="numeric"
                maxLength={IMSI_DIGITS}
                aria-invalid={!!fieldErrors.imsi}
                aria-describedby={fieldErrors.imsi ? "edit-imsi-error" : undefined}
                className="w-full glass-input font-mono"
              />
              {fieldErrors.imsi && (
                <p id="edit-imsi-error" className="text-accent-red text-xs mt-1">
                  {fieldErrors.imsi}
                </p>
              )}
            </div>
            <div>
              <label className="block text-xs text-muted mb-1" htmlFor="edit-meid">
                MEID (hex)
              </label>
              <input
                id="edit-meid"
                type="text"
                value={meid}
                onChange={(e) => {
                  setMeid(e.target.value);
                  clearFieldError("meid");
                }}
                maxLength={14}
                aria-invalid={!!fieldErrors.meid}
                aria-describedby={fieldErrors.meid ? "edit-meid-error" : undefined}
                className="w-full glass-input font-mono"
              />
              {fieldErrors.meid && (
                <p id="edit-meid-error" className="text-accent-red text-xs mt-1">
                  {fieldErrors.meid}
                </p>
              )}
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <label className="block text-xs text-muted mb-1" htmlFor="edit-number-type">
                Number Type
              </label>
              <select
                id="edit-number-type"
                value={numberType}
                onChange={(e) => setNumberType(Number(e.target.value) as NumberType)}
                className="w-full glass-input"
              >
                {NUMBER_TYPE_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="block text-xs text-muted mb-1" htmlFor="edit-number-plan">
                Numbering Plan
              </label>
              <select
                id="edit-number-plan"
                value={numberPlan}
                onChange={(e) => setNumberPlan(Number(e.target.value) as NumberPlan)}
                className="w-full glass-input"
              >
                {NUMBER_PLAN_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </div>
          </div>

          {fieldErrors.form && <p className="text-xs text-accent-red">{fieldErrors.form}</p>}
          {saveResult && <p className="text-xs text-accent-green">{saveResult}</p>}

          <button
            type="submit"
            disabled={saving}
            className="text-xs px-4 py-1.5 rounded bg-accent-green-bg text-accent-green border border-accent-green/20 hover:bg-accent-green/15 disabled:opacity-50 transition-colors"
          >
            {saving ? "Saving..." : "Save Changes"}
          </button>
        </form>
      </Card>
    </div>
  );
}
