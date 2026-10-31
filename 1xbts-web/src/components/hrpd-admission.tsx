import Link from "next/link";
import { Card, Stat } from "@/components/card";
import { reasonLabel } from "@/lib/admission";
import { HardwareVerdict, type SessionAdmission } from "@/lib/proto/an/v1/service";

const BADGE = {
  admitted: "text-accent-green bg-accent-green-bg",
  refused: "text-accent-red bg-accent-red-bg",
  unknown: "text-dimmed bg-surface-raised",
};

const ACCESS_AUTH_RESULT_LABELS: Record<string, string> = {
  accepted: "Accepted",
  rejected: "Rejected",
  aaa_error: "AN-AAA error",
  aaa_timeout: "AN-AAA timeout",
  chap_refused: "CHAP refused",
  no_response: "No response",
  peer_terminated: "Closed by handset",
};

const ACCESS_AUTH_STATE_LABELS: Record<string, string> = {
  idle: "Not started",
  lcp: "Negotiating LCP",
  challenge_sent: "Challenge sent",
  awaiting_aaa: "Waiting for AN-AAA",
  terminating: "Closing",
  settled: "Done",
};

function verdictKind(admission?: SessionAdmission): keyof typeof BADGE {
  switch (admission?.hardwareVerdict) {
    case HardwareVerdict.HARDWARE_VERDICT_ADMITTED:
      return "admitted";
    case HardwareVerdict.HARDWARE_VERDICT_REFUSED:
      return "refused";
    default:
      return "unknown";
  }
}

function verdictLabel(admission?: SessionAdmission): string {
  switch (verdictKind(admission)) {
    case "admitted":
      return "Admitted";
    case "refused":
      return "Refused";
    default:
      return "No Hardware ID";
  }
}

export function HrpdAdmissionBadge({ admission }: { admission?: SessionAdmission }) {
  if (!admission) return <span className="text-dimmed text-xs">-</span>;
  const kind = verdictKind(admission);
  return (
    <div className="space-y-0.5">
      <span className={`text-xs px-2 py-0.5 rounded whitespace-nowrap ${BADGE[kind]}`}>
        {verdictLabel(admission)}
      </span>
      {kind === "refused" && admission.reason && (
        <div className="text-[11px] text-muted">{reasonLabel(admission.reason)}</div>
      )}
      {!admission.accessClosed && (
        <div className="text-[11px] text-muted">access open</div>
      )}
    </div>
  );
}

export function HrpdAdmissionCard({ admission }: { admission?: SessionAdmission }) {
  if (!admission) return null;
  const kind = verdictKind(admission);
  const result = admission.accessAuthResult;
  return (
    <Card title="Access">
      <div className="flex justify-between items-center py-0.5">
        <span className="text-muted text-sm">Hardware ID</span>
        <HrpdAdmissionBadge admission={admission} />
      </div>
      <Stat label="Access mode" value={admission.accessClosed ? "Closed" : "Open"} />
      {kind === "refused" && (
        <Stat label="Reason" value={reasonLabel(admission.reason)} />
      )}
      <div className="flex justify-between py-0.5">
        <span className="text-muted text-sm">Subscriber</span>
        {admission.subscriberId ? (
          <Link
            href={`/subscribers/${encodeURIComponent(admission.subscriberId)}`}
            className="text-sm font-mono text-accent-cyan hover:underline"
          >
            {admission.subscriberId.slice(0, 8)}
          </Link>
        ) : (
          <span className="text-sm text-dimmed">None</span>
        )}
      </div>
      <Stat
        label="Access stream"
        value={admission.accessStreamId ? `Stream ${admission.accessStreamId}` : "None"}
      />
      {admission.accessStreamId > 0 && (
        <>
          <Stat
            label="Access authentication"
            value={
              result
                ? (ACCESS_AUTH_RESULT_LABELS[result] ?? result)
                : (ACCESS_AUTH_STATE_LABELS[admission.accessAuthState] ??
                  admission.accessAuthState)
            }
          />
          <Stat label="NAI" value={admission.accessAuthNai || "-"} mono />
          <Stat label="A12 IMSI" value={admission.accessAuthImsi || "-"} mono />
        </>
      )}
    </Card>
  );
}
