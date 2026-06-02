// Factory functions returning empty (but spec-valid) rows for each
// PRL record variant. The proto types use ts-proto's flattened
// oneof encoding: only the matching optional field is populated.

import {
  PrlAbSelection,
  PrlAcqRecord,
  PrlCommonSubnetRecord,
  PrlExtAcqRecord,
  PrlExtSysIdMccMnc,
  PrlExtSysRecord,
  PrlExtSysRecordType,
  PrlNidInclusion,
  PrlPcsBlock,
  PrlPrefNeg,
  PrlPriority,
  PrlRoamingIndicator,
  PrlRoamingIndicatorKind,
  PrlStandardChannel,
  PrlSysRecord,
} from "@/lib/proto/hlr/v1/service";

// ── ACQ classic ───────────────────────────────────────────────────────

export function emptyClassicAcq(acqTypeRaw: number): PrlAcqRecord {
  const base: PrlAcqRecord = { acqTypeRaw };
  switch (acqTypeRaw) {
    case 0x01:
      return {
        ...base,
        cellularAnalog: { ab: PrlAbSelection.PRL_AB_SELECTION_SYSTEM_A },
      };
    case 0x02:
      return {
        ...base,
        cellularCdmaStandard: {
          ab: PrlAbSelection.PRL_AB_SELECTION_EITHER,
          priSec: PrlStandardChannel.PRL_STANDARD_CHANNEL_PRIMARY_OR_SECONDARY,
        },
      };
    case 0x03:
      return { ...base, cellularCdmaCustom: { channels: [] } };
    case 0x04:
      return {
        ...base,
        cellularCdmaPreferred: { ab: PrlAbSelection.PRL_AB_SELECTION_EITHER },
      };
    case 0x05:
      return {
        ...base,
        pcsCdmaUsingBlocks: { blocks: [PrlPcsBlock.PRL_PCS_BLOCK_A] },
      };
    case 0x06:
      return { ...base, pcsCdmaUsingChannels: { channels: [] } };
    case 0x07:
      return {
        ...base,
        jtacsCdmaStandard: {
          ab: PrlAbSelection.PRL_AB_SELECTION_SYSTEM_A,
          priSec: PrlStandardChannel.PRL_STANDARD_CHANNEL_PRIMARY,
        },
      };
    case 0x08:
      return { ...base, jtacsCdmaCustom: { channels: [] } };
    case 0x09:
      return { ...base, bandClass6UsingChannels: { channels: [] } };
    default:
      return { ...base, unknown: {} };
  }
}

// ── ACQ extended ──────────────────────────────────────────────────────

export function emptyExtAcq(acqTypeRaw: number): PrlExtAcqRecord {
  const length = defaultExtAcqLength(acqTypeRaw);
  const base: PrlExtAcqRecord = { acqTypeRaw, length };
  switch (acqTypeRaw) {
    case 0x01:
      return {
        ...base,
        cellularAnalog: { ab: PrlAbSelection.PRL_AB_SELECTION_SYSTEM_A },
      };
    case 0x02:
      return {
        ...base,
        cellularCdmaStandard: {
          ab: PrlAbSelection.PRL_AB_SELECTION_EITHER,
          priSec: PrlStandardChannel.PRL_STANDARD_CHANNEL_PRIMARY_OR_SECONDARY,
        },
      };
    case 0x03:
      return { ...base, cellularCdmaCustom: { channels: [] } };
    case 0x04:
      return {
        ...base,
        cellularCdmaPreferred: { ab: PrlAbSelection.PRL_AB_SELECTION_EITHER },
      };
    case 0x05:
      return {
        ...base,
        pcsCdmaUsingBlocks: { blocks: [PrlPcsBlock.PRL_PCS_BLOCK_A] },
      };
    case 0x06:
      return { ...base, pcsCdmaUsingChannels: { channels: [] } };
    case 0x07:
      return {
        ...base,
        jtacsCdmaStandard: {
          ab: PrlAbSelection.PRL_AB_SELECTION_SYSTEM_A,
          priSec: PrlStandardChannel.PRL_STANDARD_CHANNEL_PRIMARY,
        },
      };
    case 0x08:
      return { ...base, jtacsCdmaCustom: { channels: [] } };
    case 0x09:
      return { ...base, bandClass6UsingChannels: { channels: [] } };
    case 0x0a:
      return { ...base, generic1xIs95: { entries: [] } };
    case 0x0b:
      return { ...base, genericHrpd: { entries: [] } };
    case 0x0f:
      return { ...base, umbCommonTable: { entries: [] } };
    case 0x10:
      return { ...base, genericUmb: { blocks: [] } };
    default:
      return { ...base, other: { raw: new Uint8Array() } };
  }
}

function defaultExtAcqLength(acqTypeRaw: number): number {
  switch (acqTypeRaw) {
    case 0x01: case 0x02: case 0x03: case 0x04: case 0x05:
    case 0x06: case 0x07: case 0x08: case 0x09:
      return 1;
    case 0x0a: case 0x0b: case 0x0f:
      return 0;
    case 0x10:
      return 1;
    default:
      return 1;
  }
}

// ── SYS classic ───────────────────────────────────────────────────────

export function emptyClassicSys(): PrlSysRecord {
  return {
    sid: 0,
    nidIncl: PrlNidInclusion.PRL_NID_INCLUSION_ANY,
    nid: undefined,
    sameGeoAsPrev: false,
    prefNeg: PrlPrefNeg.PRL_PREF_NEG_PREFERRED,
    acqIndex: 0,
    roamingIndicator: defaultRoamingIndicator(),
    priority: PrlPriority.PRL_PRIORITY_EQUALLY_DESIRABLE,
  };
}

// ── SYS extended ──────────────────────────────────────────────────────

export function emptyExtSys(sysRecordType: PrlExtSysRecordType): PrlExtSysRecord {
  const base: PrlExtSysRecord = {
    sysRecordLength: 6,
    sysRecordType,
    sysRecordTypeRaw: extSysRecordTypeToRaw(sysRecordType),
    prefNeg: PrlPrefNeg.PRL_PREF_NEG_PREFERRED,
    sameGeoAsPrev: false,
    priority: PrlPriority.PRL_PRIORITY_EQUALLY_DESIRABLE,
    acqIndex: 0,
    roamingIndicator: defaultRoamingIndicator(),
    association: undefined,
  };
  switch (sysRecordType) {
    case PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_CDMA2000:
      return {
        ...base,
        cdma2000: {
          nidIncl: PrlNidInclusion.PRL_NID_INCLUSION_ANY,
          sid: 0,
          nid: undefined,
        },
      };
    case PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_HRPD:
      return {
        ...base,
        hrpd: {
          subnetCommonIncluded: false,
          subnetLsbLengthBits: 0,
          subnetLsbHex: "",
          subnetCommonOffset: undefined,
        },
      };
    case PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_MCC_MNC:
      return {
        ...base,
        mccMnc: emptyMccMnc("subtype000"),
      };
    default:
      return {
        ...base,
        raw: { sysRecordType: 0, rawBits: new Uint8Array(), rawBitLen: 0 },
      };
  }
}

function extSysRecordTypeToRaw(t: PrlExtSysRecordType): number {
  switch (t) {
    case PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_CDMA2000: return 0;
    case PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_HRPD: return 1;
    case PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_RESERVED_OBSOLETE: return 2;
    case PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_MCC_MNC: return 3;
    default: return 0;
  }
}

export type MccMncSubtypeKey =
  | "subtype000"
  | "subtype001"
  | "subtype010"
  | "subtype011";

export function emptyMccMnc(subtype: MccMncSubtypeKey): PrlExtSysIdMccMnc {
  switch (subtype) {
    case "subtype000":
      return { subtype000: { mcc: "310", mnc: "23" } };
    case "subtype001":
      return { subtype001: { mcc: "310", mnc: "23", sids: [] } };
    case "subtype010":
      return { subtype010: { mcc: "310", mnc: "23", pairs: [] } };
    case "subtype011":
      return { subtype011: { mcc: "310", mnc: "23", subnets: [] } };
  }
}

// ── Common Subnet ─────────────────────────────────────────────────────

export function emptyCommonSubnet(): PrlCommonSubnetRecord {
  return { subnetCommonLengthOctets: 0, subnetCommonHex: "" };
}

// ── Shared ────────────────────────────────────────────────────────────

export function defaultRoamingIndicator(): PrlRoamingIndicator {
  return {
    raw: 0,
    kind: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_ON_HOME,
  };
}
