"use client";

import type { ReactNode } from "react";
import type {
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
  PagingAddress,
  PagingChannelAssignment,
  PagingEvent,
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
    const normalized = typeof value === "string" ? BigInt(value) : BigInt(value >>> 0);
    return `0x${normalized.toString(16).toUpperCase().padStart(width, "0")}`;
  } catch {
    return `0x${String(value).padStart(width, "0")}`;
  }
}

function FieldGrid({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-2 xl:grid-cols-3 gap-x-4 gap-y-0.5 text-xs text-muted">
      {children}
    </div>
  );
}

function Field({ label, value }: { label: string; value?: ReactNode | null }) {
  if (value == null || value === "") {
    return null;
  }
  return (
    <span>
      {label}: {value}
    </span>
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
  if (event.order) {
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

export function formatPagingSummary(event: PagingEvent): string {
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
        <Field label="FOR_RC" value={message.forRc} />
        <Field label="REV_RC" value={message.revRc} />
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
        <Field label="ORDER_SPECIFIC_HEX" value={message.orderSpecificHex || null} />
        <Field label="REMAINING_BITS" value={message.remainingBits} />
      </FieldGrid>
      {message.reject && <RejectDetail reject={message.reject} />}
    </div>
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
        <div className="pl-2 border-l border-border-input text-xs text-muted space-y-0.5">
          <div className="text-secondary font-medium">Active Set Pilot Strengths</div>
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
  if (event.order) {
    return (
      <FieldGrid>
        <Field label="ORDER" value={`0b${event.order.order.toString(2).padStart(6, "0")} (${event.order.orderName})`} />
        <Field label="ORDQ" value={event.order.ordq} />
      </FieldGrid>
    );
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

export function AccessDetail({ event }: { event: AccessEvent }) {
  return (
    <div className="text-xs text-muted space-y-2">
      <FieldGrid>
        <Field label="CHIP_START" value={event.chipStart} />
        <Field label="PREAMBLE" value={`${event.preambleFrames} frame(s)`} />
        <Field label="PD" value={event.pd} />
        <Field label="MSG_TYPE" value={formatHex(event.msgType)} />
        <Field label="MSG_SEQ" value={event.msgSeq} />
        <Field label="MSID_TYPE" value={event.msidType} />
        <Field label="ESN" value={event.esn != null ? formatHex(event.esn, 8) : null} />
        <Field label="MEID" value={event.meid ? event.meid.toUpperCase() : null} />
        <Field label="IMSI_M_S1" value={event.imsiMS1} />
        <Field label="IMSI_M_S2" value={event.imsiMS2} />
        <Field label="MOB_P_REV" value={event.mobPRev} />
        <Field label="SUBSCRIBER_ID" value={event.subscriberId ?? null} />
        <Field label="SNR_DB" value={event.snrDb != null ? event.snrDb.toFixed(1) : null} />
        <Field label="SIGNAL_PWR_DB" value={event.signalPowerDb != null ? event.signalPowerDb.toFixed(1) : null} />
        <Field label="DEMOD_QUALITY" value={event.demodQualityPct != null ? `${event.demodQualityPct.toFixed(1)}%` : null} />
        <Field label="RX_PWR_DBM" value={event.rxPowerDbm != null ? event.rxPowerDbm.toFixed(1) : null} />
        <Field label="TRAFFIC_WALSH" value={event.trafficWalshCode != null ? `W${event.trafficWalshCode}` : null} />
        <Field label="PREAMBLE_ONLY" value={event.isPreambleOnly ? "1" : null} />
      </FieldGrid>

      {event.resolvedAddress && <div>Resolved Address: {event.resolvedAddress}</div>}
      {event.address && <div>Address: {event.address}</div>}

      {event.registration && <AccessRegistrationDetail message={event.registration} />}
      {event.origination && <AccessOriginationDetail message={event.origination} />}
      {event.pageResponse && <AccessPageResponseDetail message={event.pageResponse} />}
      {event.order && <AccessOrderDetail message={event.order} />}
      {event.dataBurst && <AccessDataBurstDetail message={event.dataBurst} />}
      {event.serviceConnectCompletion && <AccessServiceConnectCompletionDetail message={event.serviceConnectCompletion} />}
      {event.serviceResponse && <AccessServiceResponseDetail message={event.serviceResponse} />}
      {event.powerMeasurementReport && <AccessPowerMeasurementReportDetail message={event.powerMeasurementReport} />}

      {!event.registration &&
        !event.origination &&
        !event.pageResponse &&
        !event.order &&
        !event.dataBurst &&
        !event.serviceConnectCompletion &&
        !event.serviceResponse &&
        !event.powerMeasurementReport &&
        event.l3Summary && <div>L3: {event.l3Summary}</div>}

      {event.rdschSummary && <div>R-DSCH: {event.rdschSummary}</div>}
      {event.rdschMsgTypeName && <div>R-DSCH Type: {event.rdschMsgTypeName}</div>}
      {event.pduSummary && <div>PDU: {event.pduSummary}</div>}
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
        <div className="pl-2 border-l border-border-input text-xs text-muted space-y-0.5">
          <div className="text-secondary font-medium">Signal Information Record</div>
          <FieldGrid>
            <Field label="SIGNAL_TYPE" value={`${sig.signalType} (${sig.signalTypeName})`} />
            <Field label="ALERT_PITCH" value={`${sig.alertPitch} (${sig.alertPitchName})`} />
            <Field label="SIGNAL" value={`${sig.signal} (${sig.signalName})`} />
          </FieldGrid>
        </div>
      )}
      {cpn && (
        <div className="pl-2 border-l border-border-input text-xs text-muted space-y-0.5">
          <div className="text-secondary font-medium">Calling Party Number</div>
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
      {event.order && (
        <FieldGrid>
          <Field label="ORDER" value={`0b${event.order.order.toString(2).padStart(6, "0")} (${event.order.orderName})`} />
          <Field label="ORDQ" value={event.order.ordq} />
        </FieldGrid>
      )}
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
      {!event.order && !event.dataBurst && !event.serviceRequest && !event.serviceConnect && !event.alertWithInfo && event.l3Summary && (
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
            <div className="text-secondary font-medium">
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
