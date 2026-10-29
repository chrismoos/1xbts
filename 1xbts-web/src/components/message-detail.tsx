"use client";

import { VoicePrivacyIndicator } from "@/components/voice-privacy-indicator";
import Link from "next/link";
import { useState, type ReactNode } from "react";
import { AuthenticationMark } from "@/components/authentication-activity";
import {
  type AuthVerdict,
  type VerdictKind,
  procedureLabel,
  verdictFeedback,
  verdictKind,
  verdictStatus,
} from "@/lib/authentication-verdict";
import type {
  AccessAuthentication,
  AccessDataBurst,
  AccessEvent,
  AccessMobileReject,
  AccessOrder,
  AccessOrigination,
  AccessPageResponse,
  AccessPowerMeasurementReport,
  AccessRegistration,
  AccessServiceConnectCompletion,
  AccessServiceResponse,
  ForwardAuthenticationChallenge,
  ForwardSsdUpdate,
  PagingAddress,
  PagingChannelAssignment,
  PagingEvent,
  PagingOrder,
  PageRecord,
  TrafficAlertWithInfo,
  TrafficEvent,
  TrafficServiceConnect,
  TrafficServiceRequest,
} from "@/lib/proto/bsc/v1/service";
import { formatNumberPlan, formatNumberType } from "@/lib/number-format";
import { teleserviceKind, teleserviceName } from "@/lib/teleservice";
import { parseMNotificationInd } from "@/lib/wap-push";
import { WapBody } from "@/components/wap-body";
import {
  formatHrpdFullUati,
  hrpdTimestampNsToMs,
  uatiHex,
} from "@/lib/hrpd-correlation";
import {
  HrpdDirection,
  HrpdTrafficReason,
  type HrpdAccessEvent,
  type HrpdDecodedMessage,
  type HrpdSessionEvent,
  type HrpdTrafficEvent,
  hrpdAccessReasonToJSON,
  hrpdSessionReasonToJSON,
  hrpdTrafficReasonToJSON,
} from "@/lib/proto/events/v1/an";
import { cellLabel } from "@/lib/cell";
import {
  a1DecodedIe,
  a1DirectionLabel,
  a1DiscriminatorLabel,
  a1NodeHref,
  a1TypeName,
  addsBurstLabel,
  formatHexOctets,
  type A1LogEvent,
} from "@/lib/a1-event";
export { shouldHideAccessEvent } from "@/lib/access-event-filter";

function smsSummary(
  sms: { teleserviceId: number; text: string; userData?: Uint8Array },
  peer: string,
): string {
  if (teleserviceKind(sms.teleserviceId) === "wap-push") {
    const parsed = sms.userData && sms.userData.length > 0
      ? parseMNotificationInd(sms.userData)
      : null;
    const target = parsed?.contentLocation ? ` -> ${parsed.contentLocation}` : peer;
    return `MMS Push |${target}`;
  }
  const body = sms.text ? ` "${sms.text}"` : "";
  return `SMS |${body}${peer}`;
}

function teleserviceField(id: number): string {
  return `${teleserviceName(id)} (${formatHex(id, 4)})`;
}

function formatBool(value: boolean): string {
  return value ? "1" : "0";
}

function formatOptBool(value?: boolean): string | null {
  return value == null ? null : formatBool(value);
}

function formatHex(value: number | string, width = 2): string {
  try {
    const normalized =
      typeof value === "string" ? BigInt(value) : value > 0xffffffff ? BigInt(value) : BigInt(value >>> 0);
    return `0x${normalized.toString(16).toUpperCase().padStart(width, "0")}`;
  } catch {
    return `0x${String(value).padStart(width, "0")}`;
  }
}

// C.S0005-E Table 2.7.3-1 and Table 3.7.4-1 order codes with authentication meaning.
const ORDER_SSD_UPDATE_RESULT = 0b000011;
const ORDER_PARAMETER_UPDATE = 0b000101;
const ORDER_LONG_CODE_TRANSITION = 0b010111;
const PARAMETER_UPDATE_REQUEST_NUMBER_MASK = 0b0000_1111;

/** Meaning of an authentication or privacy order's ORDQ, or null for other orders. */
function authenticationOrderMeaning(order: number, ordq: number, reverse: boolean): string | null {
  switch (order) {
    case ORDER_SSD_UPDATE_RESULT:
      return reverse ? (ordq === 0 ? "SSD update confirmed" : ordq === 1 ? "SSD update rejected" : null) : null;
    case ORDER_PARAMETER_UPDATE:
      return `request ${ordq & PARAMETER_UPDATE_REQUEST_NUMBER_MASK}`;
    case ORDER_LONG_CODE_TRANSITION:
      switch (ordq) {
        case 0:
          return "request public long code";
        case 1:
          return "request private long code";
        case 2:
          return "accept public long code transition";
        case 3:
          return "accept private long code transition";
        default:
          return null;
      }
    default:
      return null;
  }
}

/** AUTHR, RANDC and COUNT of an r-csch PDU (C.S0004-E §2.1.1.1.1.1). */
function AccessAuthenticationDetail({ auth }: { auth: AccessAuthentication }) {
  return (
    <div className="text-xs space-y-1">
      <div className={SECTION_HEADING_CLASS}>Authentication</div>
      <FieldGrid>
        <Field label="AUTHR" value={formatHex(auth.authr, 5)} />
        <Field label="RANDC" value={formatHex(auth.randc, 2)} />
        <Field label="COUNT" value={auth.count} />
        <Field label="RAND" value={auth.rand != null ? formatHex(auth.rand, 8) : "no broadcast RAND matches RANDC"} />
      </FieldGrid>
    </div>
  );
}

/** A section title with a rule that runs to the right edge. */
const SECTION_HEADING_CLASS =
  "flex items-center gap-2 text-[10.5px] font-semibold uppercase tracking-wider text-muted after:h-px after:flex-1 after:bg-border-primary after:content-['']";

function FieldGrid({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-x-5 text-xs">
      {children}
    </div>
  );
}

function Field({ label, value }: { label: string; value?: ReactNode | null }) {
  if (value == null || value === "") {
    return null;
  }
  return (
    <span className="flex min-w-0 justify-between gap-3 border-b border-dashed border-border-subtle py-0.5">
      <span className="shrink-0 text-[11px] text-muted">{label}</span>
      <span className="min-w-0 text-right font-mono text-primary break-words">{value}</span>
    </span>
  );
}

function DetailSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="space-y-1">
      <div className={SECTION_HEADING_CLASS}>{title}</div>
      {children}
    </div>
  );
}

function StatTile({ label, value }: { label: string; value: string | null }) {
  if (value == null) return null;
  return (
    <div className="min-w-24 rounded-md bg-surface-raised px-2.5 py-1.5">
      <div className="text-[10.5px] uppercase tracking-wide text-muted">{label}</div>
      <div className="font-mono text-sm text-primary">{value}</div>
    </div>
  );
}

type A1NodeMatchList = Parameters<typeof a1NodeHref>[0];

function maybeLink(href: string | null, content: ReactNode): ReactNode {
  if (!href) {
    return content;
  }
  return (
    <Link href={href} className="text-accent-green hover:underline">
      {content}
    </Link>
  );
}

export function formatPagingAddress(addr?: PagingAddress): string {
  if (!addr) return "broadcast";
  if (addr.esn != null) return `ESN:${formatHex(addr.esn, 8)}`;
  if (addr.imsiClass0) {
    return `IMSI_CLASS0:s1=${addr.imsiClass0.imsiMS1},s2=${addr.imsiClass0.imsiMS2}`;
  }
  if (addr.imsiS) return `IMSI_S:s1=${addr.imsiS.imsiMS1},s2=${addr.imsiS.imsiMS2}`;
  return "unknown";
}

export function formatAccessSummary(event: AccessEvent): string {
  if (event.isPreambleOnly && event.trafficWalshCode != null) {
    return "Traffic preamble";
  }
  if (event.origination) {
    const o = event.origination;
    const service = o.serviceOption != null ? `SO${o.serviceOption}` : "no SO";
    const rc =
      o.forRcPref != null && o.revRcPref != null
        ? `RC pref ${o.forRcPref}/${o.revRcPref}`
        : null;
    const digits = o.digits ? `digits ${o.digits}` : null;
    return [service, rc, digits].filter(Boolean).join(" | ");
  }
  if (event.authChallengeResponse) {
    return `AUTHU ${formatHex(event.authChallengeResponse.authu, 5)}`;
  }
  if (event.order) {
    if (event.order.randbs != null) {
      return `${event.order.orderName} | RANDBS ${formatHex(event.order.randbs, 8)}`;
    }
    const meaning = authenticationOrderMeaning(
      event.order.order,
      parseInt(event.order.orderSpecificHex.slice(0, 2) || "0", 16),
      true,
    );
    if (meaning) return `${event.order.orderName} | ${meaning}`;
    return event.order.detail ? `${event.order.orderName} | ${event.order.detail}` : event.order.orderName;
  }
  if (event.dataBurst) {
    const db = event.dataBurst;
    if (db.decodedSms) {
      const sms = db.decodedSms;
      const dest = sms.destinationNumber ? ` -> ${sms.destinationNumber}` : "";
      return smsSummary(sms, dest);
    }
    return `${db.burstTypeName} | ${db.payloadBytes}B`;
  }
  if (event.registration) {
    return `reg_type=${event.registration.regType} | p_rev=${event.registration.mobPRev}`;
  }
  if (event.pageResponse) {
    return `SO${event.pageResponse.serviceOption} | request_mode=${event.pageResponse.requestMode}`;
  }
  if (event.serviceConnectCompletion) {
    return `SERV_CON_SEQ=${event.serviceConnectCompletion.servConSeq}`;
  }
  if (event.serviceResponse) {
    const sr = event.serviceResponse;
    const so = sr.serviceOption != null ? ` | SO${sr.serviceOption}` : "";
    return `Service Response | ${sr.respPurposeName}${so}`;
  }
  if (event.powerMeasurementReport) {
    const m = event.powerMeasurementReport;
    const fer = m.pwrMeasFrames > 0
      ? ` | FER=${((m.errorsDetected / m.pwrMeasFrames) * 100).toFixed(1)}%`
      : "";
    return `PMRM | errors=${m.errorsDetected} frames=${m.pwrMeasFrames} pilots=${m.pilotStrengths.length}${fer}`;
  }
  if (event.rdschSummary) {
    return event.rdschSummary;
  }
  return event.l3Summary ?? event.pduSummary;
}

function simplifyAccessMsgTypeName(msgTypeName: string): string {
  const trafficChannelPrefix = /^TrafficCh\(W\d+\)\s+/;
  if (trafficChannelPrefix.test(msgTypeName)) {
    return msgTypeName.replace(trafficChannelPrefix, "");
  }
  if (/^TrafficPreamble\(W\d+\)$/.test(msgTypeName)) {
    return "Traffic Preamble";
  }
  return msgTypeName;
}

export function formatAccessTypeName(event: AccessEvent): string {
  if (event.isPreambleOnly && event.trafficWalshCode != null) {
    return "Traffic Preamble";
  }
  if (event.rdschMsgTypeName) {
    return event.rdschMsgTypeName;
  }
  return simplifyAccessMsgTypeName(event.msgTypeName);
}

function forwardAuthenticationSummary(event: PagingEvent | TrafficEvent): string | null {
  if (event.authChallenge) return `RANDU ${formatHex(event.authChallenge.randu, 6)}`;
  if (event.ssdUpdate) return `RANDSSD ${formatHex(event.ssdUpdate.randssd, 14)}`;
  if (event.order?.authbs != null) return `${event.order.orderName} | AUTHBS ${formatHex(event.order.authbs, 5)}`;
  if (event.order) {
    const meaning = authenticationOrderMeaning(event.order.order, event.order.ordq, false);
    if (meaning) return `${event.order.orderName} | ${meaning}`;
  }
  return null;
}

export function formatPagingSummary(event: PagingEvent): string {
  const authentication = forwardAuthenticationSummary(event);
  if (authentication) return authentication;
  if (event.order) {
    return event.order.orderName;
  }
  if (event.dataBurst) {
    if (event.dataBurst.decodedSms) {
      const sms = event.dataBurst.decodedSms;
      const orig = sms.originatingNumber ? ` from=${sms.originatingNumber}` : "";
      return smsSummary(sms, orig);
    }
    return `${event.dataBurst.burstType === 3 ? "SMS" : `burst=${event.dataBurst.burstType}`} | ${event.dataBurst.payloadBytes}B`;
  }
  if (event.generalPage) {
    return `${event.generalPage.pageRecords.length} record(s)`;
  }
  if (event.channelAssignment) {
    const m = event.channelAssignment;
    const rc =
      m.forRc != null && m.revRc != null
        ? `RC${m.forRc}/RC${m.revRc}`
        : m.defaultConfigName || null;
    return [m.assignModeName, `Walsh ${m.codeChan}`, rc].filter(Boolean).join(" | ");
  }
  return "";
}

export function formatTrafficSummary(event: TrafficEvent): string {
  const authentication = forwardAuthenticationSummary(event);
  if (authentication) return authentication;
  if (event.order) {
    const voice = event.voiceCallState ? ` [${event.voiceCallState}]` : "";
    return event.order.orderName + voice;
  }
  if (event.alertWithInfo) {
    const sig = event.alertWithInfo.signalInfo;
    const cpn = event.alertWithInfo.callingParty;
    const tone = sig?.signalName || "Unknown";
    const caller = cpn?.digits ? ` | CPN=${cpn.digits}` : "";
    return `Alert With Info | ${tone}${caller}`;
  }
  if (event.dataBurst) {
    if (event.dataBurst.decodedSms) {
      const sms = event.dataBurst.decodedSms;
      const orig = sms.originatingNumber ? ` from=${sms.originatingNumber}` : "";
      return smsSummary(sms, orig);
    }
    if (event.l3Summary) return event.l3Summary;
    return `${event.dataBurst.burstType === 3 ? "SMS" : `burst=${event.dataBurst.burstType}`} | ${event.dataBurst.payloadBytes}B`;
  }
  if (event.serviceRequest) {
    const sr = event.serviceRequest;
    const purpose = sr.reqPurpose === 2 ? "propose" : sr.reqPurpose === 1 ? "reject" : `0b${sr.reqPurpose.toString(2)}`;
    const so = sr.serviceOption != null ? `SO${sr.serviceOption}` : "";
    return `Service Request | ${purpose} | ${so} | RC${sr.forFchRc}/${sr.revFchRc}`;
  }
  if (event.serviceConnect) {
    const sc = event.serviceConnect;
    const so = sc.connections.length > 0 ? `SO${sc.connections[0].serviceOption}` : "";
    return `SERV_CON_SEQ=${sc.servConSeq} | ${so} | MUX${sc.forMuxOption}/${sc.revMuxOption} | RC${sc.forFchRc}/${sc.revFchRc}`;
  }
  return event.l3Summary || event.pduSummary;
}

export function formatAccessChannel(event: AccessEvent): string {
  if (event.trafficWalshCode != null) {
    return `R-TCH W${event.trafficWalshCode}`;
  }
  return "R-ACH";
}

export function formatPagingChannel(event?: PagingEvent): string {
  void event;
  return "F-PCH";
}

export function formatTrafficChannel(event: TrafficEvent): string {
  return event.channelName || `F-TCH W${event.walshCode}`;
}

function renderPageRecord(record: PageRecord, index: number) {
  if (record.class0) {
    const c = record.class0;
    const detail =
      c.pageSubclass === 0
        ? `IMSI_S=${c.imsiS != null ? `0x${BigInt(c.imsiS).toString(16).toUpperCase()}` : "?"}`
        : `IMSI_S1=${c.imsiMS1 ?? "?"} IMSI_S2=${c.imsiMS2 ?? "?"} MCC=${c.mcc ?? "?"}`;
    return (
      <div key={index} className="pl-2 border-l border-border-input">
        Class0: subclass={c.pageSubclass} | msg_seq={c.msgSeq} | {detail}
        {c.specialService ? " | SPECIAL_SVC" : ""}
        {c.serviceOption != null ? ` | SO=${c.serviceOption}` : ""}
      </div>
    );
  }
  if (record.class1) {
    const c = record.class1;
    return (
      <div key={index} className="pl-2 border-l border-border-input">
        Class1: ESN={formatHex(c.esn, 8)} | msg_seq={c.msgSeq}
        {c.specialService ? " | SPECIAL_SVC" : ""}
        {c.serviceOption != null ? ` | SO=${c.serviceOption}` : ""}
      </div>
    );
  }
  if (record.tmsi) {
    const c = record.tmsi;
    return (
      <div key={index} className="pl-2 border-l border-border-input">
        TMSI: code_addr={c.tmsiCodeAddr} | msg_seq={c.msgSeq}
        {c.specialService ? " | SPECIAL_SVC" : ""}
        {c.serviceOption != null ? ` | SO=${c.serviceOption}` : ""}
      </div>
    );
  }
  if (record.broadcast) {
    return (
      <div key={index} className="pl-2 border-l border-border-input">
        Broadcast: bc_addr={record.broadcast.bcAddr}
      </div>
    );
  }
  return (
    <div key={index} className="pl-2 border-l border-border-input">
      Record {index}
    </div>
  );
}

function ChannelAssignmentDetail({ message }: { message: PagingChannelAssignment }) {
  return (
    <div className="space-y-1">
      <FieldGrid>
        <Field
          label="ASSIGN_MODE"
          value={`0b${message.assignMode.toString(2).padStart(3, "0")} (${message.assignModeName})`}
        />
        <Field label="CODE_CHAN" value={message.codeChan} />
        <Field label="FRAME_OFFSET" value={message.frameOffset} />
        <Field label="ENCRYPT_MODE" value={`0b${message.encryptMode.toString(2).padStart(2, "0")}`} />
        <Field label="DIRECT_CH_ASSIGN" value={formatOptBool(message.directChAssignInd)} />
        <Field label="FREQ_INCL" value={formatBool(message.freqIncl)} />
        <Field label="BAND_CLASS" value={message.bandClass} />
        <Field label="CDMA_FREQ" value={message.cdmaFreq} />
        <Field label="BYPASS_ALERT" value={formatOptBool(message.bypassAlertAnswer)} />
        <Field
          label="DEFAULT_CONFIG"
          value={
            message.defaultConfig != null
              ? `0b${message.defaultConfig.toString(2).padStart(3, "0")} (${message.defaultConfigName || "raw"})`
              : null
          }
        />
        <Field
          label="GRANTED_MODE"
          value={
            message.grantedMode != null
              ? `0b${message.grantedMode.toString(2).padStart(2, "0")}`
              : null
          }
        />
        <Field label="EFFECTIVE_FOR_RC" value={message.forRc} />
        <Field label="EFFECTIVE_REV_RC" value={message.revRc} />
        <Field label="FPC_SUBCHAN_GAIN" value={message.fpcSubchanGain} />
        <Field label="RLGAIN_ADJ" value={message.rlgainAdj} />
        <Field
          label="CH_IND"
          value={message.chInd != null ? `0b${message.chInd.toString(2).padStart(2, "0")}` : null}
        />
        <Field label="CH_RECORD_LEN" value={message.chRecordLenOctets != null ? `${message.chRecordLenOctets} octets` : null} />
        <Field label="FPC_INIT_SETPT" value={message.fpcFchInitSetpt != null ? formatHex(message.fpcFchInitSetpt) : null} />
        <Field label="FPC_FER" value={message.fpcFchFer != null ? `0b${message.fpcFchFer.toString(2).padStart(5, "0")}` : null} />
        <Field label="FPC_MIN_SETPT" value={message.fpcFchMinSetpt != null ? formatHex(message.fpcFchMinSetpt) : null} />
        <Field label="FPC_MAX_SETPT" value={message.fpcFchMaxSetpt != null ? formatHex(message.fpcFchMaxSetpt) : null} />
        <Field label="REV_FCH_GATING" value={formatOptBool(message.revFchGatingMode)} />
        <Field label="PLCM_TYPE" value={message.plcmType} />
        <Field label="EARLY_RL_TX" value={formatOptBool(message.earlyRlTransmitInd)} />
        <Field label="TX_PWR_LIMIT" value={message.txPwrLimit} />
      </FieldGrid>
      {message.pilots.length > 0 && (
        <div className="text-xs text-muted space-y-1">
          {message.pilots.map((pilot, index) => (
            <div key={index} className="pl-2 border-l border-border-input">
              Pilot {index + 1}: PN={pilot.pilotPn} | pwr_comb={formatBool(pilot.pwrCombInd)} | code_chan_fch={pilot.codeChanFch} | qof_mask_id_fch={pilot.qofMaskIdFch}
            </div>
          ))}
        </div>
      )}
      {message.sduHex && (
        <div className="text-xs text-muted break-all font-mono">SDU_HEX: {message.sduHex}</div>
      )}
    </div>
  );
}

function AccessRegistrationDetail({ message }: { message: AccessRegistration }) {
  return (
    <FieldGrid>
      <Field label="REG_TYPE" value={message.regType} />
      <Field label="MOB_TERM" value={formatBool(message.mobTerm)} />
      <Field label="SLOT_CYCLE_INDEX" value={message.slotCycleIndex} />
      <Field label="MOB_P_REV" value={message.mobPRev} />
      <Field label="SCM" value={formatHex(message.scm)} />
      <Field label="RETURN_CAUSE" value={message.returnCause} />
      <Field label="REMAINING_BITS" value={message.remainingBits} />
    </FieldGrid>
  );
}

function AccessOriginationDetail({ message }: { message: AccessOrigination }) {
  return (
    <div className="space-y-2">
      <FieldGrid>
        <Field label="MOB_TERM" value={formatBool(message.mobTerm)} />
        <Field label="SLOT_CYCLE_INDEX" value={message.slotCycleIndex} />
        <Field label="MOB_P_REV" value={message.mobPRev} />
        <Field label="SCM" value={formatHex(message.scm)} />
        <Field label="REQUEST_MODE" value={message.requestMode} />
        <Field label="SPECIAL_SERVICE" value={formatBool(message.specialService)} />
        <Field label="SERVICE_OPTION" value={message.serviceOption} />
        <Field label="PM" value={formatBool(message.pm)} />
        <Field label="DIGIT_MODE" value={formatBool(message.digitMode)} />
        <Field label="NUMBER_TYPE" value={formatNumberType(message.numberType)} />
        <Field label="NUMBER_PLAN" value={formatNumberPlan(message.numberPlan)} />
        <Field label="MORE_FIELDS" value={formatBool(message.moreFields)} />
        <Field label="NUM_FIELDS" value={message.numFields} />
        <Field label="DIGITS" value={message.digits} />
        <Field label="NAR_AN_CAP" value={formatBool(message.narAnCap)} />
        <Field label="PACA_REORIG" value={formatBool(message.pacaReorig)} />
        <Field label="RETURN_CAUSE" value={message.returnCause} />
        <Field label="MORE_RECORDS" value={formatBool(message.moreRecords)} />
        <Field label="ENCRYPTION_SUPPORTED" value={message.encryptionSupported} />
        <Field label="PACA_SUPPORTED" value={formatBool(message.pacaSupported)} />
        <Field label="ALT_SO" value={message.altServiceOptions.length > 0 ? message.altServiceOptions.join(", ") : null} />
        <Field label="DRS" value={formatOptBool(message.drs)} />
        <Field label="UZID_INCL" value={formatOptBool(message.uzidIncl)} />
        <Field label="UZID" value={message.uzid} />
        <Field label="CH_IND" value={message.chInd} />
        <Field label="SR_ID" value={message.srId} />
        <Field label="OTD_SUPPORTED" value={formatOptBool(message.otdSupported)} />
        <Field label="QPCH_SUPPORTED" value={formatOptBool(message.qpchSupported)} />
        <Field label="ENHANCED_RC" value={formatOptBool(message.enhancedRc)} />
        <Field label="FOR_RC_PREF" value={message.forRcPref} />
        <Field label="REV_RC_PREF" value={message.revRcPref} />
        <Field label="FCH_SUPPORTED" value={formatOptBool(message.fchSupported)} />
        <Field label="DCCH_SUPPORTED" value={formatOptBool(message.dcchSupported)} />
        <Field label="GEO_LOC_INCL" value={formatOptBool(message.geoLocIncl)} />
        <Field label="GEO_LOC_TYPE" value={message.geoLocType} />
        <Field label="REV_FCH_GATING_REQ" value={formatOptBool(message.revFchGatingReq)} />
        <Field label="ORIG_REASON" value={formatOptBool(message.origReason)} />
        <Field label="ORIG_COUNT" value={message.origCount} />
        <Field label="REMAINING_BITS" value={message.remainingBits} />
      </FieldGrid>
      {message.fch && (
        <div className="text-xs text-muted pl-2 border-l border-border-input">
          FCH: frame5ms={formatBool(message.fch.frameSize5msSupported)} | for_rcs=[{message.fch.forSupportedRcs.join(", ")}] | rev_rcs=[{message.fch.revSupportedRcs.join(", ")}]
        </div>
      )}
      {message.dcch && (
        <div className="text-xs text-muted pl-2 border-l border-border-input">
          DCCH: frame_mode=0b{message.dcch.frameSizeMode.toString(2).padStart(2, "0")} | for_rcs=[{message.dcch.forSupportedRcs.join(", ")}] | rev_rcs=[{message.dcch.revSupportedRcs.join(", ")}]
        </div>
      )}
    </div>
  );
}

function AccessPageResponseDetail({ message }: { message: AccessPageResponse }) {
  return (
    <FieldGrid>
      <Field label="MOB_TERM" value={formatBool(message.mobTerm)} />
      <Field label="SLOT_CYCLE_INDEX" value={message.slotCycleIndex} />
      <Field label="MOB_P_REV" value={message.mobPRev} />
      <Field label="SCM" value={formatHex(message.scm)} />
      <Field label="REQUEST_MODE" value={message.requestMode} />
      <Field label="SERVICE_OPTION" value={message.serviceOption} />
      <Field label="PM" value={formatBool(message.pm)} />
      <Field label="NAR_AN_CAP" value={formatBool(message.narAnCap)} />
      <Field label="ALT_SO" value={message.altServiceOptions.length > 0 ? message.altServiceOptions.join(", ") : null} />
      <Field label="REMAINING_BITS" value={message.remainingBits} />
    </FieldGrid>
  );
}

function RejectDetail({ reject }: { reject: AccessMobileReject }) {
  return (
    <FieldGrid>
      <Field label="ORDQ" value={`0x${reject.ordq.toString(16).toUpperCase().padStart(2, "0")} (${reject.ordqName})`} />
      <Field label="REJECTED_TYPE" value={`0x${reject.rejectedType.toString(16).toUpperCase().padStart(2, "0")} (${reject.rejectedTypeName})`} />
      <Field label="REJECTED_ORDER" value={reject.rejectedOrder != null ? `0b${reject.rejectedOrder.toString(2).padStart(6, "0")} (${reject.rejectedOrderName ?? "Unknown"})` : null} />
      <Field label="REJECTED_ORDQ" value={reject.rejectedOrdq} />
      <Field label="REJECTED_RECORD" value={reject.rejectedRecord} />
      <Field label="CON_REF" value={reject.conRef} />
      <Field label="TAG" value={reject.tag} />
      <Field label="REJECTED_PDU_TYPE" value={reject.rejectedPduType != null ? `0b${reject.rejectedPduType.toString(2).padStart(2, "0")} (${reject.rejectedPduTypeName ?? "Unknown"})` : null} />
      <Field label="TRAILING" value={reject.trailingHex || null} />
    </FieldGrid>
  );
}

function AccessOrderDetail({ message }: { message: AccessOrder }) {
  return (
    <div className="space-y-2">
      <FieldGrid>
        <Field label="ORDER" value={`0b${message.order.toString(2).padStart(6, "0")} (${message.orderName})`} />
        <Field label="ADD_RECORD_LEN" value={message.addRecordLen} />
        <Field label="DETAIL" value={message.detail} />
        <Field label="RANDBS" value={message.randbs != null ? formatHex(message.randbs, 8) : null} />
        <Field
          label="MEANING"
          value={authenticationOrderMeaning(
            message.order,
            parseInt(message.orderSpecificHex.slice(0, 2) || "0", 16),
            true,
          )}
        />
        <Field label="ORDER_SPECIFIC_HEX" value={message.orderSpecificHex || null} />
        <Field label="REMAINING_BITS" value={message.remainingBits} />
      </FieldGrid>
      {message.reject && <RejectDetail reject={message.reject} />}
    </div>
  );
}

function ForwardOrderDetail({ order }: { order: PagingOrder }) {
  return (
    <FieldGrid>
      <Field label="ORDER" value={`0b${order.order.toString(2).padStart(6, "0")} (${order.orderName})`} />
      <Field label="ORDQ" value={order.ordq} />
      <Field label="AUTHBS" value={order.authbs != null ? formatHex(order.authbs, 5) : null} />
      <Field label="MEANING" value={authenticationOrderMeaning(order.order, order.ordq, false)} />
    </FieldGrid>
  );
}

/** Authentication Challenge Message (C.S0005-E §3.7.2.3.2.10, §3.7.3.3.2.2). */
function ForwardAuthChallengeDetail({ message }: { message: ForwardAuthenticationChallenge }) {
  return (
    <FieldGrid>
      <Field label="RANDU" value={formatHex(message.randu, 6)} />
      <Field label="GEN_CMEA_KEY" value={formatBool(message.genCmeaKey)} />
    </FieldGrid>
  );
}

/** SSD Update Message (C.S0005-E §3.7.2.3.2.11, §3.7.3.3.2.5). */
function ForwardSsdUpdateDetail({ message }: { message: ForwardSsdUpdate }) {
  return (
    <FieldGrid>
      <Field label="RANDSSD" value={formatHex(message.randssd, 14)} />
    </FieldGrid>
  );
}

function AccessServiceConnectCompletionDetail({ message }: { message: AccessServiceConnectCompletion }) {
  return (
    <FieldGrid>
      <Field label="SERV_CON_SEQ" value={message.servConSeq} />
    </FieldGrid>
  );
}

function AccessPowerMeasurementReportDetail({ message }: { message: AccessPowerMeasurementReport }) {
  const fer = message.pwrMeasFrames > 0
    ? `${((message.errorsDetected / message.pwrMeasFrames) * 100).toFixed(1)}%`
    : "N/A";
  return (
    <div className="space-y-1">
      <FieldGrid>
        <Field label="ERRORS_DETECTED" value={message.errorsDetected} />
        <Field label="PWR_MEAS_FRAMES" value={message.pwrMeasFrames} />
        <Field label="FER" value={fer} />
        <Field label="LAST_HDM_SEQ" value={message.lastHdmSeq === 3 ? "3 (none received)" : message.lastHdmSeq} />
        <Field label="NUM_PILOTS" value={message.pilotStrengths.length} />
        <Field label="DCCH_PWR_MEAS_INCL" value={formatBool(message.dcchPwrMeasIncl)} />
        {message.dcchPwrMeasIncl && (
          <>
            <Field label="DCCH_PWR_MEAS_FRAMES" value={message.dcchPwrMeasFrames} />
            <Field label="DCCH_ERRORS_DETECTED" value={message.dcchErrorsDetected} />
          </>
        )}
        <Field label="SCH_PWR_MEAS_INCL" value={formatBool(message.schPwrMeasIncl)} />
        {message.schPwrMeasIncl && (
          <>
            <Field label="SCH_ID" value={message.schId} />
            <Field label="SCH_PWR_MEAS_FRAMES" value={message.schPwrMeasFrames} />
            <Field label="SCH_ERRORS_DETECTED" value={message.schErrorsDetected} />
          </>
        )}
      </FieldGrid>
      {message.pilotStrengths.length > 0 && (
        <div className="text-xs space-y-1">
          <div className={SECTION_HEADING_CLASS}>Active Set Pilot Strengths</div>
          {message.pilotStrengths.map((strength, i) => (
            <div key={i}>
              Pilot {i}: PILOT_STRENGTH={strength}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function AccessServiceResponseDetail({ message }: { message: AccessServiceResponse }) {
  return (
    <FieldGrid>
      <Field label="SERV_REQ_SEQ" value={message.servReqSeq} />
      <Field label="RESP_PURPOSE" value={`0b${message.respPurpose.toString(2).padStart(4, "0")} (${message.respPurposeName})`} />
      <Field label="SERVICE_OPTION" value={message.serviceOption != null ? `SO${message.serviceOption}` : null} />
    </FieldGrid>
  );
}

function AccessDataBurstDetail({ message }: { message: AccessDataBurst }) {
  const sms = message.decodedSms;
  const isWap = sms ? teleserviceKind(sms.teleserviceId) === "wap-push" : false;
  return (
    <div className="flex flex-col gap-2">
      <FieldGrid>
        <Field label="MSG_NUMBER" value={message.msgNumber} />
        <Field label="BURST_TYPE" value={`0b${message.burstType.toString(2).padStart(6, "0")} (${message.burstTypeName})`} />
        <Field label="NUM_MSGS" value={message.numMsgs} />
        <Field label="NUM_FIELDS" value={message.numFields} />
        <Field label="PAYLOAD_BYTES" value={message.payloadBytes} />
        <Field label="PAYLOAD_HEX" value={message.payloadHex || null} />
        <Field label="REMAINING_BITS" value={message.remainingBits} />
        {sms && (
          <>
            <Field label="SMS_TELESERVICE" value={teleserviceField(sms.teleserviceId)} />
            <Field label="SMS_DEST" value={sms.destinationNumber || null} />
            <Field label="SMS_ORIG" value={sms.originatingNumber || null} />
            <Field label="SMS_MSG_TYPE" value={sms.messageType} />
            <Field label="SMS_MSG_ID" value={sms.messageId} />
            {!isWap && <Field label="SMS_TEXT" value={sms.text || null} />}
            {isWap && <Field label="SMS_USER_DATA" value={`${sms.userData?.length ?? 0} bytes`} />}
          </>
        )}
      </FieldGrid>
      {isWap && sms?.userData && sms.userData.length > 0 && <WapBody bytes={sms.userData} />}
    </div>
  );
}

export function PagingDetail({ event }: { event: PagingEvent }) {
  if (event.systemParameters) {
    const m = event.systemParameters;
    return (
      <FieldGrid>
        <Field label="SID" value={m.sid} />
        <Field label="NID" value={m.nid} />
        <Field label="BASE_ID" value={m.baseId} />
        <Field label="PILOT_PN" value={m.pilotPn} />
        <Field label="REG_ZONE" value={m.regZone} />
        <Field label="TOTAL_ZONES" value={m.totalZones} />
        <Field label="PAGE_CHAN" value={m.pageChan} />
        <Field label="MAX_SCI" value={m.maxSlotCycleIndex} />
        <Field label="PWR_UP_REG" value={formatBool(m.powerUpReg)} />
        <Field label="PARAM_REG" value={formatBool(m.parameterReg)} />
      </FieldGrid>
    );
  }
  if (event.accessParameters) {
    const m = event.accessParameters;
    return (
      <FieldGrid>
        <Field label="PILOT_PN" value={m.pilotPn} />
        <Field label="ACC_CHAN" value={m.accChan} />
        <Field label="NOM_PWR" value={`${m.nomPwr} dB`} />
        <Field label="INIT_PWR" value={`${m.initPwr} dB`} />
        <Field label="PWR_STEP" value={`${m.pwrStep} dB`} />
        <Field label="NUM_STEP" value={m.numStep} />
        <Field label="MAX_CAP_SZ" value={m.maxCapSz} />
        <Field label="AUTH" value={m.auth} />
        <Field label="RAND" value={m.auth !== 0 ? formatHex(m.rand, 8) : null} />
      </FieldGrid>
    );
  }
  if (event.neighborList) {
    const m = event.neighborList;
    return (
      <FieldGrid>
        <Field label="PILOT_PN" value={m.pilotPn} />
        <Field label="PILOT_INC" value={m.pilotInc} />
        <Field label="NEIGHBORS" value={m.neighbors.join(", ")} />
      </FieldGrid>
    );
  }
  if (event.cdmaChannelList) {
    return (
      <FieldGrid>
        <Field label="CHANNELS" value={event.cdmaChannelList.channels.join(", ")} />
      </FieldGrid>
    );
  }
  if (event.extendedSystemParameters) {
    const m = event.extendedSystemParameters;
    return (
      <FieldGrid>
        <Field label="P_REV" value={m.pRev} />
        <Field label="MIN_P_REV" value={m.minPRev} />
        <Field label="MCC" value={m.mcc} />
        <Field label="IMSI_11_12" value={m.imsi1112} />
        <Field label="USE_TMSI" value={formatBool(m.useTmsi)} />
        <Field label="PREF_MSID_TYPE" value={m.prefMsidType} />
        <Field label="MAX_NUM_ALT_SO" value={m.maxNumAltSo} />
      </FieldGrid>
    );
  }
  if (event.generalPage) {
    const m = event.generalPage;
    return (
      <div className="text-xs text-muted space-y-1">
        <FieldGrid>
          <Field label="CONFIG_MSG_SEQ" value={m.configMsgSeq} />
          <Field label="ACC_MSG_SEQ" value={m.accMsgSeq} />
          <Field label="CLASS_0_DONE" value={formatBool(m.class0Done)} />
          <Field label="CLASS_1_DONE" value={formatBool(m.class1Done)} />
          <Field label="TMSI_DONE" value={formatBool(m.tmsiDone)} />
        </FieldGrid>
        {m.pageRecords.length === 0 ? (
          <div className="text-dimmed">No page records (idle)</div>
        ) : (
          m.pageRecords.map(renderPageRecord)
        )}
      </div>
    );
  }
  if (event.authChallenge) {
    return <ForwardAuthChallengeDetail message={event.authChallenge} />;
  }
  if (event.ssdUpdate) {
    return <ForwardSsdUpdateDetail message={event.ssdUpdate} />;
  }
  if (event.order) {
    return <ForwardOrderDetail order={event.order} />;
  }
  if (event.dataBurst) {
    const sms = event.dataBurst.decodedSms;
    const isWap = sms ? teleserviceKind(sms.teleserviceId) === "wap-push" : false;
    return (
      <div className="flex flex-col gap-2">
        <FieldGrid>
          <Field label="BURST" value={event.dataBurst.burstType === 3 ? "SMS" : `type=${event.dataBurst.burstType}`} />
          <Field label="MSG_NUMBER" value={event.dataBurst.msgNumber} />
          <Field label="NUM_MSGS" value={event.dataBurst.numMsgs} />
          <Field label="PAYLOAD" value={`${event.dataBurst.payloadBytes} bytes`} />
          {sms && (
            <>
              <Field label="SMS_TELESERVICE" value={teleserviceField(sms.teleserviceId)} />
              <Field label="SMS_ORIG" value={sms.originatingNumber || null} />
              <Field label="SMS_MSG_TYPE" value={sms.messageType} />
              <Field label="SMS_MSG_ID" value={sms.messageId} />
              {!isWap && <Field label="SMS_TEXT" value={sms.text || null} />}
              {isWap && <Field label="SMS_USER_DATA" value={`${sms.userData?.length ?? 0} bytes`} />}
            </>
          )}
        </FieldGrid>
        {isWap && sms?.userData && sms.userData.length > 0 && <WapBody bytes={sms.userData} />}
      </div>
    );
  }
  if (event.channelAssignment) {
    return <ChannelAssignmentDetail message={event.channelAssignment} />;
  }
  return null;
}

const VERDICT_BANNER_CLASS: Record<VerdictKind, string> = {
  ok: "bg-accent-green-bg",
  fail: "bg-accent-red-bg",
  pending: "bg-accent-amber-bg",
};

export function AccessDetail({ event, verdict }: { event: AccessEvent; verdict?: AuthVerdict | null }) {
  const message =
    event.registration ||
    event.origination ||
    event.pageResponse ||
    event.order ||
    event.dataBurst ||
    event.serviceConnectCompletion ||
    event.serviceResponse ||
    event.powerMeasurementReport ||
    event.authChallengeResponse;
  const kind = verdict ? verdictKind(verdict) : null;
  const raw = [
    event.address && `Address: ${event.address}`,
    event.resolvedAddress && `Resolved address: ${event.resolvedAddress}`,
    event.l3Summary && `L3: ${event.l3Summary}`,
    event.rdschMsgTypeName && `R-DSCH type: ${event.rdschMsgTypeName}`,
    event.rdschSummary && `R-DSCH: ${event.rdschSummary}`,
    event.pduSummary && `PDU: ${event.pduSummary}`,
  ].filter(Boolean);
  return (
    <div className="space-y-3 text-xs">
      {verdict && kind && (
        <div className={`flex gap-2 rounded-md px-3 py-2 text-secondary ${VERDICT_BANNER_CLASS[kind]}`}>
          <AuthenticationMark verdict={verdict} />
          <span>
            <span className="font-semibold text-primary">
              {procedureLabel(verdict)}: {verdictStatus(verdict)}.
            </span>{" "}
            {verdictFeedback(verdict)}
          </span>
        </div>
      )}

      <div className="flex flex-wrap gap-2">
        <StatTile label="SNR" value={event.snrDb != null ? `${event.snrDb.toFixed(1)} dB` : null} />
        <StatTile label="Signal" value={event.signalPowerDb != null ? `${event.signalPowerDb.toFixed(1)} dB` : null} />
        <StatTile label="RX power" value={event.rxPowerDbm != null ? `${event.rxPowerDbm.toFixed(1)} dBm` : null} />
        <StatTile label="Demod" value={event.demodQualityPct != null ? `${event.demodQualityPct.toFixed(1)}%` : null} />
        <StatTile label="Preamble" value={event.preambleFrames ? `${event.preambleFrames} frames` : null} />
        <StatTile label="Walsh" value={event.trafficWalshCode != null ? `W${event.trafficWalshCode}` : null} />
      </div>

      <DetailSection title="Identity">
        <FieldGrid>
          <Field label="ESN" value={event.esn != null ? formatHex(event.esn, 8) : null} />
          <Field label="MEID" value={event.meid ? event.meid.toUpperCase() : null} />
          <Field label="IMSI_M_S1" value={event.imsiMS1} />
          <Field label="IMSI_M_S2" value={event.imsiMS2} />
          <Field label="MSID_TYPE" value={event.msidType} />
          <Field label="MOB_P_REV" value={event.mobPRev} />
          <Field label="Subscriber" value={event.subscriberId ?? null} />
        </FieldGrid>
      </DetailSection>

      <DetailSection title="Header">
        <FieldGrid>
          <Field label="PD" value={event.pd} />
          <Field label="MSG_TYPE" value={formatHex(event.msgType)} />
          <Field label="MSG_SEQ" value={event.msgSeq} />
          <Field label="CHIP_START" value={event.chipStart} />
          <Field label="PREAMBLE_ONLY" value={event.isPreambleOnly ? "1" : null} />
        </FieldGrid>
      </DetailSection>

      {event.authentication && <AccessAuthenticationDetail auth={event.authentication} />}

      {message && (
        <DetailSection title={event.msgTypeName || "Message"}>
          {event.registration && <AccessRegistrationDetail message={event.registration} />}
          {event.origination && <AccessOriginationDetail message={event.origination} />}
          {event.pageResponse && <AccessPageResponseDetail message={event.pageResponse} />}
          {event.order && <AccessOrderDetail message={event.order} />}
          {event.dataBurst && <AccessDataBurstDetail message={event.dataBurst} />}
          {event.serviceConnectCompletion && (
            <AccessServiceConnectCompletionDetail message={event.serviceConnectCompletion} />
          )}
          {event.serviceResponse && <AccessServiceResponseDetail message={event.serviceResponse} />}
          {event.powerMeasurementReport && (
            <AccessPowerMeasurementReportDetail message={event.powerMeasurementReport} />
          )}
          {event.authChallengeResponse && (
            <FieldGrid>
              <Field label="AUTHU" value={formatHex(event.authChallengeResponse.authu, 5)} />
            </FieldGrid>
          )}
        </DetailSection>
      )}

      {raw.length > 0 && (
        <details>
          <summary className="cursor-pointer text-muted">Raw decoder output</summary>
          <pre className="mt-1.5 whitespace-pre-wrap break-all rounded-md bg-surface-raised px-2.5 py-2 font-mono text-[11.5px] text-secondary">
            {raw.join("\n")}
          </pre>
        </details>
      )}
    </div>
  );
}

function AlertWithInfoDetail({ message }: { message: TrafficAlertWithInfo }) {
  const sig = message.signalInfo;
  const cpn = message.callingParty;
  return (
    <div className="space-y-1">
      <FieldGrid>
        <Field label="NUM_INFO_RECS" value={message.numInfoRecords} />
      </FieldGrid>
      {sig && (
        <div className="text-xs space-y-1">
          <div className={SECTION_HEADING_CLASS}>Signal Information Record</div>
          <FieldGrid>
            <Field label="SIGNAL_TYPE" value={`${sig.signalType} (${sig.signalTypeName})`} />
            <Field label="ALERT_PITCH" value={`${sig.alertPitch} (${sig.alertPitchName})`} />
            <Field label="SIGNAL" value={`${sig.signal} (${sig.signalName})`} />
          </FieldGrid>
        </div>
      )}
      {cpn && (
        <div className="text-xs space-y-1">
          <div className={SECTION_HEADING_CLASS}>Calling Party Number</div>
          <FieldGrid>
            <Field label="DIGITS" value={cpn.digits} />
            <Field label="NUMBER_TYPE" value={formatNumberType(cpn.numberType)} />
            <Field label="NUMBER_PLAN" value={formatNumberPlan(cpn.numberPlan)} />
            <Field label="PI" value={cpn.presentationIndicator} />
            <Field label="SI" value={cpn.screeningIndicator} />
          </FieldGrid>
        </div>
      )}
    </div>
  );
}

function TrafficServiceRequestDetail({ message }: { message: TrafficServiceRequest }) {
  const purposeName = message.reqPurpose === 1 ? "reject" : message.reqPurpose === 2 ? "propose" : `unknown(${message.reqPurpose})`;
  return (
    <div className="space-y-2">
      <FieldGrid>
        <Field label="SERV_REQ_SEQ" value={message.servReqSeq} />
        <Field label="REQ_PURPOSE" value={`0b${message.reqPurpose.toString(2).padStart(4, "0")} (${purposeName})`} />
        <Field label="SERVICE_OPTION" value={message.serviceOption != null ? `SO${message.serviceOption}` : null} />
        <Field label="FOR_MUX_OPTION" value={message.forMuxOption != null ? formatHex(message.forMuxOption, 4) : null} />
        <Field label="REV_MUX_OPTION" value={message.revMuxOption != null ? formatHex(message.revMuxOption, 4) : null} />
        <Field label="FOR_FCH_RC" value={message.forFchRc} />
        <Field label="REV_FCH_RC" value={message.revFchRc} />
      </FieldGrid>
    </div>
  );
}

function TrafficServiceConnectDetail({ message }: { message: TrafficServiceConnect }) {
  return (
    <div className="space-y-2">
      <FieldGrid>
        <Field label="SERV_CON_SEQ" value={message.servConSeq} />
        <Field label="FOR_MUX_OPTION" value={formatHex(message.forMuxOption, 4)} />
        <Field label="REV_MUX_OPTION" value={formatHex(message.revMuxOption, 4)} />
        <Field label="FOR_RATES" value={formatHex(message.forRates)} />
        <Field label="REV_RATES" value={formatHex(message.revRates)} />
        <Field label="FCH_FRAME_SIZE" value={message.fchFrameSize} />
        <Field label="FOR_FCH_RC" value={message.forFchRc} />
        <Field label="REV_FCH_RC" value={message.revFchRc} />
        <Field label="NON_NEG_HEX" value={message.nonNegHex || null} />
      </FieldGrid>
      {message.connections.length > 0 && (
        <div className="text-xs text-muted space-y-1">
          {message.connections.map((conn, index) => (
            <div key={index} className="pl-2 border-l border-border-input">
              Connection {index}: CON_REF={conn.conRef} | SO={conn.serviceOption} | FOR_TRAFFIC={conn.forTraffic} | REV_TRAFFIC={conn.revTraffic} | UI_ENCRYPT={conn.uiEncryptMode} | SR_ID={conn.srId} | RLP_INFO={conn.rlpInfoIncl ? "1" : "0"}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export function TrafficDetail({ event }: { event: TrafficEvent }) {
  const h = event.header;
  return (
    <div className="text-xs text-muted space-y-2">
      <FieldGrid>
        <Field label="CHANNEL" value={event.channelName} />
        <Field label="WALSH" value={`W${event.walshCode}`} />
        <Field label="SERVICE_OPTION" value={event.serviceOption != null ? `SO${event.serviceOption}` : null} />
        <Field label="RC" value={event.rcName || null} />
        <Field label="MSG_TAG" value={h != null ? formatHex(h.msgTag) : null} />
        <Field label="MSG_TYPE" value={h?.msgTypeName} />
        <Field label="MSG_SEQ" value={h?.msgSeq} />
        <Field label="ACK_SEQ" value={h?.ackSeq} />
        <Field label="ACK_REQ" value={h != null ? formatBool(h.ackReq) : null} />
        <Field label="VALID_ACK" value={h != null ? formatBool(h.validAck) : null} />
        <Field label="SDU_BITS" value={h?.sduLengthBits} />
      </FieldGrid>

      {event.address && <div>Address: {event.address}</div>}
      {event.order && <ForwardOrderDetail order={event.order} />}
      {event.authChallenge && <ForwardAuthChallengeDetail message={event.authChallenge} />}
      {event.ssdUpdate && <ForwardSsdUpdateDetail message={event.ssdUpdate} />}
      {event.dataBurst && (() => {
        const db = event.dataBurst;
        const sms = db.decodedSms;
        const isWap = sms ? teleserviceKind(sms.teleserviceId) === "wap-push" : false;
        return (
          <>
            <FieldGrid>
              <Field label="BURST" value={db.burstType === 3 ? "SMS" : `type=${db.burstType}`} />
              <Field label="MSG_NUMBER" value={db.msgNumber} />
              <Field label="NUM_MSGS" value={db.numMsgs} />
              <Field label="PAYLOAD" value={`${db.payloadBytes} bytes`} />
              {sms && (
                <>
                  <Field label="SMS_TELESERVICE" value={teleserviceField(sms.teleserviceId)} />
                  <Field label="SMS_ORIG" value={sms.originatingNumber || null} />
                  <Field label="SMS_MSG_TYPE" value={sms.messageType} />
                  <Field label="SMS_MSG_ID" value={sms.messageId} />
                  {!isWap && <Field label="SMS_TEXT" value={sms.text || null} />}
                  {isWap && <Field label="SMS_USER_DATA" value={`${sms.userData?.length ?? 0} bytes`} />}
                </>
              )}
            </FieldGrid>
            {isWap && sms?.userData && sms.userData.length > 0 && (
              <WapBody bytes={sms.userData} />
            )}
          </>
        );
      })()}
      {event.serviceRequest && <TrafficServiceRequestDetail message={event.serviceRequest} />}
      {event.serviceConnect && <TrafficServiceConnectDetail message={event.serviceConnect} />}
      {event.alertWithInfo && <AlertWithInfoDetail message={event.alertWithInfo} />}
      {event.voiceCallState && (
        <div className="text-xs">
          Voice State: <span className={`px-1.5 py-0.5 rounded ${
            event.voiceCallState === "Connected" ? "bg-badge-green-bg text-badge-green-text" :
            event.voiceCallState === "Alerting" ? "bg-badge-yellow-bg text-badge-yellow-text" :
            event.voiceCallState === "Releasing" ? "bg-badge-orange-bg text-badge-orange-text" :
            "bg-badge-blue-bg text-badge-blue-text"
          }`}>{event.voiceCallState}</span>
        </div>
      )}
      {!event.order && !event.dataBurst && !event.serviceRequest && !event.serviceConnect && !event.alertWithInfo && !event.authChallenge && !event.ssdUpdate && event.l3Summary && (
        <div>L3: {event.l3Summary}</div>
      )}
      {event.pduSummary && <div>PDU: {event.pduSummary}</div>}
      {event.sduHex && <div className="font-mono break-all text-muted">SDU_HEX: {event.sduHex}</div>}
      {event.pduHex && <div className="font-mono break-all text-muted">PDU_HEX: {event.pduHex}</div>}
    </div>
  );
}

// ----- HRPD detail components ---------------------------------------------

function formatUati(uati: number): string {
  return uatiHex(uati);
}

function formatTimestamp(ns: number | string): string {
  const ms = hrpdTimestampNsToMs(ns);
  return ms == null ? "-" : new Date(ms).toISOString();
}

function bytesHex(b: Uint8Array | undefined): string {
  const bytes = normalizeBytes(b);
  if (bytes.length === 0) return "-";
  return bytes
    .map((x) => x.toString(16).padStart(2, "0"))
    .join("");
}

function normalizeBytes(value: Uint8Array | number[] | Record<string, number> | string | undefined): number[] {
  if (!value) return [];
  if (typeof value === "string") {
    try {
      return Array.from(atob(value), (char) => char.charCodeAt(0));
    } catch {
      return [];
    }
  }
  if (value instanceof Uint8Array || Array.isArray(value)) {
    return Array.from(value);
  }
  return Object.keys(value)
    .sort((a, b) => Number(a) - Number(b))
    .map((key) => value[key])
    .filter((byte) => Number.isFinite(byte));
}

function payloadLength(payload: Uint8Array | undefined, explicitLength?: number): number {
  if (explicitLength != null && explicitLength > 0) return explicitLength;
  return normalizeBytes(payload).length;
}

export function hrpdDirectionLabel(direction: HrpdDirection): "EVDO TX" | "EVDO RX" | "EVDO" {
  if (direction === HrpdDirection.HRPD_DIRECTION_TX) return "EVDO TX";
  if (direction === HrpdDirection.HRPD_DIRECTION_RX) return "EVDO RX";
  return "EVDO";
}

export function hrpdDirectionClass(direction?: HrpdDirection): string {
  if (direction === HrpdDirection.HRPD_DIRECTION_TX) return "text-accent-cyan";
  if (direction === HrpdDirection.HRPD_DIRECTION_RX) return "text-accent-green";
  return "text-accent-purple";
}

function primaryDecoded(messages: HrpdDecodedMessage[]): HrpdDecodedMessage | undefined {
  return messages.find((m) => m.typeName || m.summary);
}

export function formatHrpdSessionSummary(event: HrpdSessionEvent): string {
  const canonical = formatHrpdFullUati(event.fullUati);
  return `HRPD Session uati=${canonical ?? formatUati(event.uati)} ${hrpdSessionReasonToJSON(event.reason)} color=${event.colorCode}`;
}

export function formatHrpdAccessSummary(event: HrpdAccessEvent): string {
  const decoded = primaryDecoded(event.decodedMessages);
  if (decoded?.summary) return decoded.summary;
  return `HRPD Access sig=${event.accessSignature} ${hrpdAccessReasonToJSON(event.reason)} color=${event.colorCode} (${payloadLength(event.payload, event.payloadLengthBytes)}B)`;
}

export function formatHrpdTrafficSummary(event: HrpdTrafficEvent): string {
  const decoded = primaryDecoded(event.decodedMessages);
  if (decoded?.summary) return decoded.summary;
  const canonical = formatHrpdFullUati(event.fullUati);
  const identity = canonical
    ? `uati=${canonical} receive_ati=${formatUati(event.receiveAti || event.uati)}`
    : `receive_ati=${formatUati(event.receiveAti || event.uati)}`;
  if (
    event.reason ===
    HrpdTrafficReason.HRPD_TRAFFIC_REASON_REVERSE_PILOT_SNR_UPDATED
  ) {
    return `ReversePilotSNR ${identity} mac=${event.macIndex} snr=${(
      event.reversePilotSnrDbTenths / 10
    ).toFixed(1)}dB drc=${event.drcValue}`;
  }
  return `HRPD Traffic ${identity} mac=${event.macIndex} drc=${event.drcValue} ${hrpdTrafficReasonToJSON(event.reason)}`;
}

export function HrpdSessionDetail({ event }: { event: HrpdSessionEvent }) {
  const canonical = formatHrpdFullUati(event.fullUati);
  return (
    <FieldGrid>
      <Field label="Timestamp" value={formatTimestamp(event.timestampNs)} />
      <Field label="UATI" value={canonical} />
      <Field label="Session Key" value={formatUati(event.uati)} />
      <Field label="Reason" value={hrpdSessionReasonToJSON(event.reason)} />
      <Field label="Color Code" value={event.colorCode} />
      <Field label="Air-Link Mgmt Subtype" value={event.airLinkManagementSubtype} />
      <Field label="Session Mgmt Subtype" value={event.sessionManagementSubtype} />
      <Field label="Address Mgmt Subtype" value={event.addressManagementSubtype} />
      <Field label="Connection Layer Subtype" value={event.connectionLayerSubtype} />
      <Field label="Security Subtype" value={event.securitySubtype} />
      <Field label="MAC Subtype" value={event.macSubtype} />
      <Field label="PHY Subtype" value={event.physicalLayerSubtype} />
    </FieldGrid>
  );
}

export function HrpdAccessDetail({ event }: { event: HrpdAccessEvent }) {
  const length = payloadLength(event.payload, event.payloadLengthBytes);
  const canonical = formatHrpdFullUati(event.fullUati);
  return (
    <div className="space-y-1">
      <FieldGrid>
        <Field label="Timestamp" value={formatTimestamp(event.timestampNs)} />
        <Field label="Direction" value={hrpdDirectionLabel(event.direction)} />
        <Field label="UATI" value={canonical} />
        <Field label="Associated Key" value={event.uati ? formatUati(event.uati) : null} />
        <Field label="Receive ATI" value={event.receiveAti ? formatUati(event.receiveAti) : null} />
        <Field label="Access Signature" value={formatHex(event.accessSignature, 8)} />
        <Field label="Reason" value={hrpdAccessReasonToJSON(event.reason)} />
        <Field label="Color Code" value={event.colorCode} />
        <Field label="Payload Length" value={`${length} bytes`} />
      </FieldGrid>
      {event.decodedMessages.length > 0 && <HrpdDecodedMessages messages={event.decodedMessages} />}
      {length > 0 && (
        <div className="font-mono break-all text-muted text-xs">
          PAYLOAD: {bytesHex(event.payload)}
        </div>
      )}
    </div>
  );
}

export function HrpdTrafficDetail({ event }: { event: HrpdTrafficEvent }) {
  const length = payloadLength(event.payload, event.payloadLengthBytes);
  const canonical = formatHrpdFullUati(event.fullUati);
  return (
    <div className="space-y-1">
      <FieldGrid>
        <Field label="Timestamp" value={formatTimestamp(event.timestampNs)} />
        <Field label="Direction" value={hrpdDirectionLabel(event.direction)} />
        <Field label="UATI" value={canonical} />
        <Field label="Receive ATI" value={formatUati(event.receiveAti || event.uati)} />
        <Field label="Reason" value={hrpdTrafficReasonToJSON(event.reason)} />
        <Field label="MAC Index" value={event.macIndex} />
        <Field label="DRC Value" value={event.drcValue} />
        <Field label="Payload Length" value={`${length} bytes`} />
      </FieldGrid>
      {event.decodedMessages.length > 0 && <HrpdDecodedMessages messages={event.decodedMessages} />}
      {length > 0 && (
        <div className="font-mono break-all text-muted text-xs">
          PAYLOAD: {bytesHex(event.payload)}
        </div>
      )}
    </div>
  );
}

function HrpdDecodedMessages({ messages }: { messages: HrpdDecodedMessage[] }) {
  return (
    <div className="text-xs text-muted space-y-1">
      {messages.map((message, index) => {
        const len = payloadLength(message.payload);
        return (
          <div key={index} className="pl-2 border-l border-border-input">
            <div className={SECTION_HEADING_CLASS}>
              {message.typeName || `Message ${index + 1}`}
            </div>
            <FieldGrid>
              <Field label="Summary" value={message.summary || null} />
              <Field label="Protocol" value={message.protocolType ? formatHex(message.protocolType) : null} />
              <Field label="Message ID" value={message.messageId ? formatHex(message.messageId) : null} />
              <Field label="Payload" value={`${len} bytes`} />
            </FieldGrid>
            {len > 0 && (
              <div className="font-mono break-all text-muted">
                DECODED_PAYLOAD: {bytesHex(message.payload)}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

export function LaneBadge({ lane }: { lane: "L3" | "A1" }) {
  return (
    <span
      className={`font-mono text-[10px] leading-none px-1 py-0.5 rounded shrink-0 w-7 text-center ${
        lane === "A1"
          ? "bg-badge-purple-bg text-badge-purple-text"
          : "bg-surface-raised text-muted"
      }`}
      title={
        lane === "A1"
          ? "A1 — IOS DTAP/BSMAP signaling between the access network and the MSC (A.S0014)"
          : "L3 — air-interface Layer 3, as transmitted by the mobile"
      }
    >
      {lane}
    </span>
  );
}

export function A1Detail({
  event,
  cells = [],
}: {
  event: A1LogEvent;
  cells?: A1NodeMatchList;
}) {
  const discriminator = a1DiscriminatorLabel(event.discriminator);
  const identity = event.mobileIdentity;
  const [showBytes, setShowBytes] = useState(false);
  const decodedCount = event.informationElements.filter((ie) =>
    a1DecodedIe(ie),
  ).length;
  const peerHref = event.peerId ? a1NodeHref(cells, { peerId: event.peerId }) : null;
  const cellHref = event.cell ? a1NodeHref(cells, { cell: event.cell }) : null;
  return (
    <div className="space-y-1">
      <FieldGrid>
        <Field label="Direction" value={a1DirectionLabel(event.direction)} />
        <Field label="Discriminator" value={discriminator || null} />
        <Field label="Message Type" value={a1TypeName(event)} />
        {event.voicePrivacy && <Field label="Voice privacy" value={<span className="inline-flex items-center gap-2"><VoicePrivacyIndicator status={event.voicePrivacy} />{event.voicePrivacy}</span>} />}
        <Field
          label="Peer"
          value={
            event.peerId
              ? maybeLink(peerHref, <span className="font-mono">{event.peerId}</span>)
              : null
          }
        />
        <Field
          label="Cell"
          value={event.cell ? maybeLink(cellHref, cellLabel(event.cell)) : null}
        />
        <Field label="IMSI" value={identity?.imsi || null} />
        <Field
          label="ESN"
          value={identity?.esn ? formatHex(identity.esn, 8) : null}
        />
        <Field label="MEID" value={identity?.meid || null} />
        <Field label="SDU Length" value={`${event.sduLengthBytes} bytes`} />
        {event.adds && (
          <Field
            label="ADDS"
            value={`${addsBurstLabel(event.adds.burstType)} (0x${event.adds.burstTypeValue
              .toString(16)
              .toUpperCase()
              .padStart(2, "0")}), ${event.adds.payloadLengthBytes} bytes`}
          />
        )}
        <Field label="A1 Call" value={event.a1CallId !== "0" ? event.a1CallId : null} />
        <Field label="SIP Call-ID" value={event.sipCallId || null} />
        <Field label="Content-Type" value={event.contentType || null} />
        <Field label="Peer Address" value={event.peer || null} />
      </FieldGrid>

      {event.sduHex && (
        <div className="text-xs text-muted break-all font-mono">
          SDU_HEX: {formatHexOctets(event.sduHex)}
        </div>
      )}

      {event.informationElements.length > 0 && (
        <div className="space-y-0.5 pt-1">
          <div className="text-secondary font-medium text-xs">
            Information Elements ({event.informationElements.length})
            {decodedCount > 0 && (
              <button
                type="button"
                onClick={() => setShowBytes((shown) => !shown)}
                className="ml-2 font-normal text-accent-green hover:underline"
              >
                {showBytes ? "hide bytes" : "show bytes"}
              </button>
            )}
          </div>
          {event.informationElements.map((ie, index) => {
            const decoded = a1DecodedIe(ie);
            const bytes = !decoded || showBytes;
            return (
              <div
                key={`${ie.iei}-${index}`}
                className="text-xs font-mono flex gap-2 items-baseline"
              >
                <span className="text-accent-amber shrink-0 w-10">
                  {formatHex(ie.iei)}
                </span>
                <span className="text-secondary shrink-0 w-56 truncate">
                  {ie.name || "unmodelled"}
                </span>
                <span className="break-all">
                  {decoded && <span className="text-primary">{decoded}</span>}
                  {bytes && (
                    <span className={decoded ? "text-muted ml-2" : "text-muted"}>
                      {formatHexOctets(ie.contentHex)}
                    </span>
                  )}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
