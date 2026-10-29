import {
  A1Direction,
  A1MessageType,
  A1MessageDiscriminator,
  A1MobileIdentityForm,
  AddsBurstType,
  a1CmServiceTypeToJSON,
  a1NumberTypeToJSON,
  a1NumberingPlanToJSON,
  a1RegistrationTypeToJSON,
  type A1CmServiceType,
  type A1NumberType,
  type A1NumberingPlan,
  type A1RegistrationType,
} from "@/lib/proto/events/v1/a1";
import type { CellId } from "@/lib/proto/events/v1/cell";
import { serviceOptionName } from "@/lib/service-option";

// SSE-compacted A1 message from `/api/events`: `sdu` is dropped for `sduHex` and IEs arrive pre-hexed, so the browser sees no base64 or Uint8Array.
export interface A1LogEvent {
  voicePrivacy?: string;
  // FEID for a femtocell, base-station id for a BSC. Empty when unknown.
  peerId: string;
  direction: A1Direction;
  discriminator: A1MessageDiscriminator;
  messageType: A1MessageType;
  messageTypeValue: number;
  messageTypeName: string;
  sduHex: string;
  sduLengthBytes: number;
  informationElements: A1LogInformationElement[];
  summary: string;
  cell?: CellId;
  mobileIdentity?: { imsi: string; esn: number; meid: string };
  adds?: {
    burstType: AddsBurstType;
    burstTypeValue: number;
    payloadLengthBytes: number;
  };
  timestampUs: string;
  eventId: string;
  sipCallId: string;
  contentType: string;
  peer: string;
  a1CallId: string;
  source?: number;
  identity?: { imsi?: string; esn?: number; meid?: string };
  subscriber?: { subscriberId?: string };
  sequence?: string;
}

export interface A1LogInformationElement {
  iei: number;
  name: string;
  contentHex: string;
  // Parsed convenience fields. `contentHex` above is always authoritative.
  mobileIdentity?: { form: A1MobileIdentityForm; imsi: string; esn: number; meid: string };
  serviceOption?: { serviceOption: number };
  registrationType?: { type: A1RegistrationType };
  cmServiceType?: { type: A1CmServiceType };
  slotCycleIndex?: { value: number };
  tag?: { value: number };
  channelType?: {
    speechOrDataIndicator: number;
    channelRateAndType: number;
    coding: number;
  };
  channelNumber?: { value: number };
  calledParty?: {
    numberType: A1NumberType;
    numberingPlan: A1NumberingPlan;
    digits: string;
  };
  authResponse?: { valueHex: string };
  authConfirmation?: { randc: number };
  authCount?: { count: number };
  authChallenge?: { valueHex: string };
}

function enumLabel(name: string, value: number, prefix: string): string {
  if (name === "UNRECOGNIZED") {
    return `${value}`;
  }
  return name.replace(prefix, "").toLowerCase().replace(/_/g, " ");
}

function hex(value: number, width = 2): string {
  return `0x${value.toString(16).toUpperCase().padStart(width, "0")}`;
}

export function a1TimestampUs(event: A1LogEvent): number {
  const parsed = Number(event.timestampUs);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : Date.now() * 1000;
}

// A1 is a network-side interface, so UL/DL read relative to the MSC rather than
// the TX/RX the air-interface lanes use.
export function a1DirectionLabel(direction: A1Direction): string {
  switch (direction) {
    case A1Direction.A1_DIRECTION_UPLINK:
      return "A1 UL";
    case A1Direction.A1_DIRECTION_DOWNLINK:
      return "A1 DL";
    default:
      return "A1";
  }
}

export function a1DirectionClass(direction: A1Direction): string {
  switch (direction) {
    case A1Direction.A1_DIRECTION_UPLINK:
      return "text-accent-green";
    case A1Direction.A1_DIRECTION_DOWNLINK:
      return "text-accent-blue";
    default:
      return "text-dimmed";
  }
}

export function a1DiscriminatorLabel(
  discriminator: A1MessageDiscriminator
): string {
  switch (discriminator) {
    case A1MessageDiscriminator.A1_MESSAGE_DISCRIMINATOR_DTAP:
      return "DTAP";
    case A1MessageDiscriminator.A1_MESSAGE_DISCRIMINATOR_BSMAP:
      return "BSMAP";
    default:
      return "";
  }
}

export function addsBurstLabel(burstType: AddsBurstType): string {
  switch (burstType) {
    case AddsBurstType.ADDS_BURST_TYPE_SMS:
      return "SMS";
    case AddsBurstType.ADDS_BURST_TYPE_OTA:
      return "OTA";
    case AddsBurstType.ADDS_BURST_TYPE_PLD:
      return "PLD";
    default:
      return "ADDS";
  }
}

export function a1TypeName(event: A1LogEvent): string {
  return `${event.messageTypeName} (0x${event.messageTypeValue
    .toString(16)
    .toUpperCase()
    .padStart(2, "0")})`;
}

export function formatA1Summary(event: A1LogEvent): string {
  if (event.summary) return event.summary;
  return `${event.informationElements.length} IE(s), ${event.sduLengthBytes} bytes`;
}

export function formatHexOctets(hex: string): string {
  return hex.replace(/(.{2})/g, "$1 ").trim();
}

// `peerId` is not assumed to be a femtocell FEID — A1 is spoken to a BSC too —
// so lookups resolve against the live access-node list rather than building a
// route from the identifier.
export function a1NodeHref(
  cells: { peerId?: string; baseStation?: string; cell?: { cell?: number; sector?: number } }[],
  match: { peerId?: string; cell?: { cell?: number; sector?: number } },
): string | null {
  const entry = cells.find((candidate) => {
    if (match.peerId) {
      return candidate.peerId === match.peerId;
    }
    if (match.cell && candidate.cell) {
      return (
        candidate.cell.cell === match.cell.cell &&
        (candidate.cell.sector ?? 0) === (match.cell.sector ?? 0)
      );
    }
    return false;
  });
  if (!entry?.peerId) {
    return null;
  }
  const base = entry.baseStation
    ? `?baseStation=${encodeURIComponent(entry.baseStation)}`
    : "";
  return `/bts/${encodeURIComponent(entry.peerId)}${base}`;
}

export function a1DecodedIe(ie: A1LogInformationElement): string | null {
  if (ie.mobileIdentity) {
    const { form, imsi, esn, meid } = ie.mobileIdentity;
    if (form === A1MobileIdentityForm.A1_MOBILE_IDENTITY_FORM_IMSI) return `IMSI ${imsi}`;
    if (form === A1MobileIdentityForm.A1_MOBILE_IDENTITY_FORM_ESN) {
      return `ESN ${hex(esn, 8)}`;
    }
    if (form === A1MobileIdentityForm.A1_MOBILE_IDENTITY_FORM_MEID) return `MEID ${meid}`;
    return null;
  }
  if (ie.serviceOption) {
    const so = ie.serviceOption.serviceOption;
    const name = serviceOptionName(so);
    return name ? `SO ${so} (${name})` : `SO ${so}`;
  }
  // The generated `*ToJSON` helpers give the variant name. An unlisted wire
  // value comes back as "UNRECOGNIZED", shown as the raw number.
  if (ie.registrationType) {
    return enumLabel(
      a1RegistrationTypeToJSON(ie.registrationType.type),
      ie.registrationType.type,
      "A1_REGISTRATION_TYPE_",
    );
  }
  if (ie.cmServiceType) {
    return enumLabel(
      a1CmServiceTypeToJSON(ie.cmServiceType.type),
      ie.cmServiceType.type,
      "A1_CM_SERVICE_TYPE_",
    );
  }
  if (ie.slotCycleIndex) return `SCI ${ie.slotCycleIndex.value}`;
  if (ie.tag) return `tag ${ie.tag.value}`;
  if (ie.channelType) {
    const { speechOrDataIndicator, channelRateAndType, coding } = ie.channelType;
    return `speech/data ${hex(speechOrDataIndicator)} rate/type ${hex(
      channelRateAndType,
    )} coding ${hex(coding)}`;
  }
  if (ie.channelNumber) return `channel ${ie.channelNumber.value}`;
  if (ie.calledParty) {
    const { digits, numberType, numberingPlan } = ie.calledParty;
    const type = enumLabel(
      a1NumberTypeToJSON(numberType),
      numberType,
      "A1_NUMBER_TYPE_",
    );
    const plan = enumLabel(
      a1NumberingPlanToJSON(numberingPlan),
      numberingPlan,
      "A1_NUMBERING_PLAN_",
    );
    return `${digits || "(no digits)"} [${type}/${plan}]`;
  }
  if (ie.authResponse) return `AUTHR ${formatHexOctets(ie.authResponse.valueHex)}`;
  if (ie.authConfirmation) return `RANDC ${ie.authConfirmation.randc}`;
  if (ie.authCount) return `COUNT ${ie.authCount.count}`;
  if (ie.authChallenge) return `RAND ${formatHexOctets(ie.authChallenge.valueHex)}`;
  return null;
}
