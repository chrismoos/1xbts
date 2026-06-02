// PRL editor validation. Returns a map of field-path → error message.
// The Save button is disabled when this map is non-empty.

import {
  PrlAcqRecord,
  PrlCommonSubnetRecord,
  PrlDecoded,
  PrlExtAcqRecord,
  PrlExtSysRecord,
  PrlNidInclusion,
  PrlPrefNeg,
  PrlSysRecord,
} from "@/lib/proto/hlr/v1/service";
import { acqRecordsOf, sysRecordsOf, subnetRecordsOf } from "./state";

export type ErrorMap = Map<string, string>;

export interface ValidationResult {
  errors: ErrorMap;
  /** True iff `errors.size === 0`. */
  isValid: boolean;
}

export function validate(d: PrlDecoded): ValidationResult {
  const errors: ErrorMap = new Map();

  if (d.classic) {
    validateHeader(d.classic, "header", errors, {
      prListIdMax: 0xffff,
    });
  } else if (d.extended) {
    validateHeader(d.extended, "header", errors, {
      prListIdMax: 0xffff,
    });
  } else {
    errors.set("body", "Missing classic or extended body");
  }

  const acqs = acqRecordsOf(d);
  acqs.forEach((r, i) => validateAcqRecord(r, `acq[${i}]`, errors));

  const sysIsClassic = !!d.classic;
  const syss = sysRecordsOf(d);
  syss.forEach((r, i) => {
    if (sysIsClassic) {
      validateClassicSys(r as PrlSysRecord, `sys[${i}]`, acqs.length, i, errors);
    } else {
      const subnetCount = subnetRecordsOf(d).length;
      validateExtSys(
        r as PrlExtSysRecord,
        `sys[${i}]`,
        acqs.length,
        subnetCount,
        errors
      );
    }
  });

  const subnets = subnetRecordsOf(d);
  subnets.forEach((r, i) =>
    validateCommonSubnet(r, `subnet[${i}]`, errors)
  );

  return { errors, isValid: errors.size === 0 };
}

function validateHeader(
  hdr: { prListId: number; defRoamInd?: { raw: number } },
  prefix: string,
  errors: ErrorMap,
  opts: { prListIdMax: number }
) {
  if (hdr.prListId < 0 || hdr.prListId > opts.prListIdMax) {
    errors.set(`${prefix}.prListId`, `Must be 0–${opts.prListIdMax}`);
  }
  if (hdr.defRoamInd && (hdr.defRoamInd.raw < 0 || hdr.defRoamInd.raw > 255)) {
    errors.set(`${prefix}.defRoamInd`, "Roaming indicator must be 0–255");
  }
}

function validateAcqRecord(
  r: PrlAcqRecord | PrlExtAcqRecord,
  prefix: string,
  errors: ErrorMap
) {
  const channelMax = (1 << 11) - 1;
  const blockMax = 7;

  if (r.cellularCdmaCustom) {
    const c = r.cellularCdmaCustom.channels;
    if (c.length === 0)
      errors.set(`${prefix}.channels`, "At least one channel required");
    if (c.length > 31) errors.set(`${prefix}.channels`, "Max 31 channels");
    if (c.some((v) => v < 0 || v > channelMax))
      errors.set(`${prefix}.channels`, `Each channel 0–${channelMax}`);
  }
  if (r.pcsCdmaUsingChannels) {
    const c = r.pcsCdmaUsingChannels.channels;
    if (c.length === 0)
      errors.set(`${prefix}.channels`, "At least one channel required");
    if (c.length > 31) errors.set(`${prefix}.channels`, "Max 31 channels");
    if (c.some((v) => v < 0 || v > channelMax))
      errors.set(`${prefix}.channels`, `Each channel 0–${channelMax}`);
  }
  if (r.jtacsCdmaCustom) {
    const c = r.jtacsCdmaCustom.channels;
    if (c.length === 0)
      errors.set(`${prefix}.channels`, "At least one channel required");
    if (c.some((v) => v < 0 || v > channelMax))
      errors.set(`${prefix}.channels`, `Each channel 0–${channelMax}`);
  }
  if (r.bandClass6UsingChannels) {
    const c = r.bandClass6UsingChannels.channels;
    if (c.length === 0)
      errors.set(`${prefix}.channels`, "At least one channel required");
    if (c.some((v) => v < 0 || v > channelMax))
      errors.set(`${prefix}.channels`, `Each channel 0–${channelMax}`);
  }
  if (r.pcsCdmaUsingBlocks) {
    const b = r.pcsCdmaUsingBlocks.blocks;
    if (b.length === 0)
      errors.set(`${prefix}.blocks`, "At least one block required");
    if (b.length > blockMax)
      errors.set(`${prefix}.blocks`, `Max ${blockMax} blocks`);
  }

  // Extended-only paths
  if ("generic1xIs95" in r && r.generic1xIs95) {
    r.generic1xIs95.entries.forEach((e, i) => {
      if (e.bandClass < 0 || e.bandClass > 31)
        errors.set(`${prefix}.entry[${i}].bandClass`, "0–31 (5 bits)");
      if (e.channelNumber < 0 || e.channelNumber > channelMax)
        errors.set(`${prefix}.entry[${i}].channelNumber`, `0–${channelMax}`);
    });
  }
  if ("genericHrpd" in r && r.genericHrpd) {
    r.genericHrpd.entries.forEach((e, i) => {
      if (e.bandClass < 0 || e.bandClass > 31)
        errors.set(`${prefix}.entry[${i}].bandClass`, "0–31 (5 bits)");
      if (e.channelNumber < 0 || e.channelNumber > channelMax)
        errors.set(`${prefix}.entry[${i}].channelNumber`, `0–${channelMax}`);
    });
  }
  if ("umbCommonTable" in r && r.umbCommonTable) {
    r.umbCommonTable.entries.forEach((e, i) => {
      if (e.umbAcqProfile > 63)
        errors.set(`${prefix}.entry[${i}].umbAcqProfile`, "0–63 (6 bits)");
      if (e.fftSize > 15)
        errors.set(`${prefix}.entry[${i}].fftSize`, "0–15 (4 bits)");
      if (e.cyclicPrefixLength > 7)
        errors.set(`${prefix}.entry[${i}].cyclicPrefixLength`, "0–7 (3 bits)");
      if (e.numGuardSubcarriers > 127)
        errors.set(
          `${prefix}.entry[${i}].numGuardSubcarriers`,
          "0–127 (7 bits)"
        );
    });
  }
  if ("genericUmb" in r && r.genericUmb) {
    const blocks = r.genericUmb.blocks;
    if (blocks.length > 63)
      errors.set(`${prefix}.blocks`, "Max 63 blocks (6-bit count)");
    blocks.forEach((b, i) => {
      if (b.bandClass > 255)
        errors.set(`${prefix}.block[${i}].bandClass`, "0–255 (8 bits)");
      if (b.channelNumber > 0xffff)
        errors.set(`${prefix}.block[${i}].channelNumber`, "0–65535 (16 bits)");
      if (b.umbAcqTableProfile > 63)
        errors.set(
          `${prefix}.block[${i}].umbAcqTableProfile`,
          "0–63 (6 bits)"
        );
    });
  }
}

function validateClassicSys(
  r: PrlSysRecord,
  prefix: string,
  acqCount: number,
  index: number,
  errors: ErrorMap
) {
  if (r.sid < 0 || r.sid > 0x7fff)
    errors.set(`${prefix}.sid`, "0–32767 (15 bits)");

  if (r.nidIncl === PrlNidInclusion.PRL_NID_INCLUSION_SINGLE) {
    if (r.nid == null)
      errors.set(`${prefix}.nid`, "NID required when NID_INCL = SingleNid");
    else if (r.nid < 0 || r.nid > 0xffff)
      errors.set(`${prefix}.nid`, "0–65535 (16 bits)");
  } else if (r.nid != null) {
    errors.set(
      `${prefix}.nid`,
      "NID must be omitted unless NID_INCL = SingleNid"
    );
  }

  if (r.acqIndex < 0 || r.acqIndex >= acqCount)
    errors.set(
      `${prefix}.acqIndex`,
      `Must reference an existing ACQ row (0–${Math.max(0, acqCount - 1)})`
    );

  if (index === 0 && r.sameGeoAsPrev)
    errors.set(`${prefix}.sameGeoAsPrev`, "First record must start a new GEO");

  if (r.prefNeg === PrlPrefNeg.PRL_PREF_NEG_PREFERRED) {
    if (!r.roamingIndicator)
      errors.set(
        `${prefix}.roamingIndicator`,
        "Required when PREF_NEG = Preferred"
      );
    if (r.priority == null)
      errors.set(`${prefix}.priority`, "Required when PREF_NEG = Preferred");
  } else if (r.prefNeg === PrlPrefNeg.PRL_PREF_NEG_NEGATIVE) {
    if (r.roamingIndicator)
      errors.set(
        `${prefix}.roamingIndicator`,
        "Must be cleared when PREF_NEG = Negative"
      );
  }
}

function validateExtSys(
  r: PrlExtSysRecord,
  prefix: string,
  acqCount: number,
  subnetCount: number,
  errors: ErrorMap
) {
  if (r.acqIndex < 0 || r.acqIndex >= acqCount)
    errors.set(
      `${prefix}.acqIndex`,
      `Must reference an existing ACQ row (0–${Math.max(0, acqCount - 1)})`
    );

  if (r.cdma2000) {
    if (r.cdma2000.sid < 0 || r.cdma2000.sid > 0x7fff)
      errors.set(`${prefix}.cdma2000.sid`, "0–32767 (15 bits)");
    if (
      r.cdma2000.nidIncl === PrlNidInclusion.PRL_NID_INCLUSION_SINGLE &&
      r.cdma2000.nid == null
    )
      errors.set(`${prefix}.cdma2000.nid`, "NID required for SingleNid");
  }

  if (r.hrpd) {
    if (r.hrpd.subnetLsbLengthBits > 127)
      errors.set(
        `${prefix}.hrpd.subnetLsbLengthBits`,
        "0–127 (7 bits)"
      );
    const wantBytes = Math.ceil(r.hrpd.subnetLsbLengthBits / 8) * 2;
    if (r.hrpd.subnetLsbHex.length !== wantBytes)
      errors.set(
        `${prefix}.hrpd.subnetLsbHex`,
        `Must be ${wantBytes} hex chars for ${r.hrpd.subnetLsbLengthBits} bits`
      );
    if (r.hrpd.subnetCommonIncluded) {
      if (r.hrpd.subnetCommonOffset == null)
        errors.set(
          `${prefix}.hrpd.subnetCommonOffset`,
          "Required when SUBNET_COMMON_INCLUDED"
        );
      else if (
        r.hrpd.subnetCommonOffset < 0 ||
        r.hrpd.subnetCommonOffset >= subnetCount
      )
        errors.set(
          `${prefix}.hrpd.subnetCommonOffset`,
          `Must reference an existing Common Subnet row (0–${Math.max(0, subnetCount - 1)})`
        );
    }
  }

  if (r.mccMnc) {
    const validateMccMnc = (mcc: string, mnc: string, sub: string) => {
      if (!/^\d{3}$/.test(mcc))
        errors.set(`${prefix}.mccMnc.${sub}.mcc`, "MCC must be 3 digits");
      if (!/^\d{2,3}$/.test(mnc))
        errors.set(
          `${prefix}.mccMnc.${sub}.mnc`,
          "MNC must be 2 or 3 digits"
        );
    };
    if (r.mccMnc.subtype000)
      validateMccMnc(
        r.mccMnc.subtype000.mcc,
        r.mccMnc.subtype000.mnc,
        "subtype000"
      );
    if (r.mccMnc.subtype001) {
      validateMccMnc(
        r.mccMnc.subtype001.mcc,
        r.mccMnc.subtype001.mnc,
        "subtype001"
      );
      if (r.mccMnc.subtype001.sids.length > 15)
        errors.set(
          `${prefix}.mccMnc.subtype001.sids`,
          "Max 15 SIDs (4-bit count)"
        );
    }
    if (r.mccMnc.subtype010) {
      validateMccMnc(
        r.mccMnc.subtype010.mcc,
        r.mccMnc.subtype010.mnc,
        "subtype010"
      );
      if (r.mccMnc.subtype010.pairs.length > 15)
        errors.set(
          `${prefix}.mccMnc.subtype010.pairs`,
          "Max 15 (SID,NID) pairs"
        );
    }
    if (r.mccMnc.subtype011) {
      validateMccMnc(
        r.mccMnc.subtype011.mcc,
        r.mccMnc.subtype011.mnc,
        "subtype011"
      );
      if (r.mccMnc.subtype011.subnets.length > 15)
        errors.set(
          `${prefix}.mccMnc.subtype011.subnets`,
          "Max 15 subnets"
        );
      r.mccMnc.subtype011.subnets.forEach((s, i) => {
        const wantBytes = Math.ceil(s.subnetLengthBits / 8) * 2;
        if (s.subnetIdHex.length !== wantBytes)
          errors.set(
            `${prefix}.mccMnc.subtype011.subnets[${i}].subnetIdHex`,
            `Must be ${wantBytes} hex chars for ${s.subnetLengthBits} bits`
          );
      });
    }
  }

  if (r.prefNeg === PrlPrefNeg.PRL_PREF_NEG_PREFERRED && !r.roamingIndicator)
    errors.set(
      `${prefix}.roamingIndicator`,
      "Required when PREF_NEG = Preferred"
    );
}

function validateCommonSubnet(
  r: PrlCommonSubnetRecord,
  prefix: string,
  errors: ErrorMap
) {
  if (r.subnetCommonLengthOctets > 15)
    errors.set(
      `${prefix}.subnetCommonLengthOctets`,
      "0–15 (4 bits)"
    );
  const wantChars = r.subnetCommonLengthOctets * 2;
  if (r.subnetCommonHex.length !== wantChars)
    errors.set(
      `${prefix}.subnetCommonHex`,
      `Must be ${wantChars} hex chars for ${r.subnetCommonLengthOctets} octets`
    );
}
