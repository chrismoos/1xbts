// Minimal valid PRLs for "build from scratch" flow.
// The server-side encoder will compute PR_LIST_SIZE and CRC at save time;
// we just need a structurally valid template the editor can render.

import {
  PrlNidInclusion,
  PrlDecoded,
  PrlExtSysRecordType,
  PrlRoamingIndicatorKind,
} from "@/lib/proto/hlr/v1/service";
import { type BtsConfig, EvdoTxMode } from "@/lib/proto/bsc/v1/service";
import {
  emptyClassicAcq,
  emptyExtAcq,
  emptyExtSys,
} from "@/components/prl-editor/builders";

const DEFAULT_ROAM = {
  raw: 1,
  kind: PrlRoamingIndicatorKind.PRL_ROAMING_INDICATOR_KIND_INDICATOR_OFF,
};

export function emptyClassicPrl(prListId: number): PrlDecoded {
  return {
    classic: {
      prListSize: 0,
      prListId,
      prefOnly: false,
      defRoamInd: DEFAULT_ROAM,
      prListCrc: 0,
      computedCrc: 0,
      crcOk: true,
      // One CellularCdmaPreferred to satisfy "at least one acq record".
      acquisitionRecords: [emptyClassicAcq(0x04)],
      systemRecords: [],
    },
  };
}

export function emptyExtendedPrl(prListId: number): PrlDecoded {
  return {
    extended: {
      prListSize: 0,
      prListId,
      curSsprPRev: 3,
      prefOnly: false,
      defRoamInd: DEFAULT_ROAM,
      prListCrc: 0,
      computedCrc: 0,
      crcOk: true,
      acquisitionRecords: [],
      commonSubnetRecords: [],
      systemRecords: [],
    },
  };
}

export interface RunningSystemCarrierSelection {
  oneX: boolean;
  hrpd: boolean;
}

export function runningSystemCarriers(config: BtsConfig): RunningSystemCarrierSelection {
  return {
    oneX: config.evdo?.mode !== EvdoTxMode.EVDO_TX_MODE_HRPD_ONLY,
    hrpd: config.evdo !== undefined,
  };
}

export function runningSystemPrl(
  prListId: number,
  config: BtsConfig,
  selected: RunningSystemCarrierSelection,
): PrlDecoded {
  const prl = emptyExtendedPrl(prListId);
  const body = prl.extended!;
  const available = runningSystemCarriers(config);

  if (!selected.oneX && !selected.hrpd) {
    throw new Error("Select at least one running carrier.");
  }
  if (selected.oneX && !available.oneX) {
    throw new Error("The running system is not transmitting a 1x carrier.");
  }
  if (selected.hrpd && !available.hrpd) {
    throw new Error("The running system does not have an HRPD carrier.");
  }

  if (selected.oneX) {
    if (!config.overhead) {
      throw new Error("The running system did not report its 1x SID and NID.");
    }
    const bandClass = parseBandClass(config.bandClass);
    validateChannel(config.cdmaChannel, "1x");
    const acqIndex = body.acquisitionRecords.length;
    const acq = oneXAcquisition(bandClass, config.cdmaChannel);
    acq.length = 2;
    body.acquisitionRecords.push(acq);

    const anyNid = config.overhead.nid === 0xffff;
    const system = emptyExtSys(
      PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_CDMA2000,
    );
    system.sysRecordLength = anyNid ? 6 : 8;
    system.acqIndex = acqIndex;
    system.cdma2000 = {
      sid: config.overhead.sid,
      nidIncl: anyNid
        ? PrlNidInclusion.PRL_NID_INCLUSION_ANY
        : PrlNidInclusion.PRL_NID_INCLUSION_SINGLE,
      nid: anyNid ? undefined : config.overhead.nid,
    };
    body.systemRecords.push(system);
  }

  if (selected.hrpd) {
    if (!config.evdo) {
      throw new Error("The running system does not have an HRPD carrier.");
    }
    validateBandClass(config.evdo.bandClass, "HRPD");
    validateChannel(config.evdo.channel, "HRPD");
    const acqIndex = body.acquisitionRecords.length;
    const acq = emptyExtAcq(0x0b);
    acq.length = 2;
    acq.genericHrpd = {
      entries: [
        {
          bandClass: config.evdo.bandClass,
          channelNumber: config.evdo.channel,
        },
      ],
    };
    body.acquisitionRecords.push(acq);

    const subnet = hrpdSubnet(config.evdo.sectorId, config.evdo.subnetMask);
    if (subnet.commonHex) {
      body.commonSubnetRecords.push({
        subnetCommonLengthOctets: subnet.commonHex.length / 2,
        subnetCommonHex: subnet.commonHex,
      });
    }

    const system = emptyExtSys(PrlExtSysRecordType.PRL_EXT_SYS_RECORD_TYPE_HRPD);
    system.sysRecordLength = hrpdSystemRecordLength(
      subnet.lsbLengthBits,
      subnet.commonHex !== "",
    );
    system.sameGeoAsPrev = body.systemRecords.length > 0;
    system.acqIndex = acqIndex;
    system.hrpd = {
      subnetCommonIncluded: subnet.commonHex !== "",
      subnetLsbLengthBits: subnet.lsbLengthBits,
      subnetLsbHex: subnet.lsbHex,
      subnetCommonOffset: subnet.commonHex ? 0 : undefined,
    };
    body.systemRecords.push(system);
  }

  return prl;
}

export function parseBandClass(value: string): number {
  const match = /^bc(\d+)$/i.exec(value.trim());
  const bandClass = match ? Number(match[1]) : Number.NaN;
  if (!Number.isInteger(bandClass) || bandClass < 0 || bandClass > 31) {
    throw new Error(`Unsupported running 1x band class: ${value || "unknown"}.`);
  }
  return bandClass;
}

function oneXAcquisition(bandClass: number, channel: number) {
  switch (bandClass) {
    case 0: {
      const record = emptyExtAcq(0x03);
      record.cellularCdmaCustom = { channels: [channel] };
      return record;
    }
    case 1: {
      const record = emptyExtAcq(0x06);
      record.pcsCdmaUsingChannels = { channels: [channel] };
      return record;
    }
    case 3: {
      const record = emptyExtAcq(0x08);
      record.jtacsCdmaCustom = { channels: [channel] };
      return record;
    }
    case 6: {
      const record = emptyExtAcq(0x09);
      record.bandClass6UsingChannels = { channels: [channel] };
      return record;
    }
    default: {
      const record = emptyExtAcq(0x0a);
      record.generic1xIs95 = {
        entries: [{ bandClass, channelNumber: channel }],
      };
      return record;
    }
  }
}

function validateBandClass(bandClass: number, carrier: string): void {
  if (!Number.isInteger(bandClass) || bandClass < 0 || bandClass > 31) {
    throw new Error(`The running system reported an invalid ${carrier} band class.`);
  }
}

function validateChannel(channel: number, carrier: string): void {
  if (!Number.isInteger(channel) || channel < 0 || channel > 0x7ff) {
    throw new Error(`The running system reported an invalid ${carrier} channel.`);
  }
}

function hrpdSubnet(
  sectorId: string,
  subnetMask: number,
): { commonHex: string; lsbLengthBits: number; lsbHex: string } {
  const normalized = sectorId.replace(/^0x/i, "").replace(/:/g, "");
  if (!/^[0-9a-f]{32}$/i.test(normalized)) {
    throw new Error("The running system reported an invalid HRPD Sector ID.");
  }
  if (!Number.isInteger(subnetMask) || subnetMask < 0 || subnetMask > 128) {
    throw new Error("The running system reported an invalid HRPD subnet mask.");
  }

  // SUBNET_LSB_LENGTH is seven bits. A /128 therefore needs one common
  // subnet octet and a 120-bit LSB segment; all shorter prefixes fit directly.
  const commonOctets = subnetMask === 128 ? 1 : 0;
  const lsbLengthBits = subnetMask - commonOctets * 8;
  const firstLsbOctet = commonOctets;
  const lsbOctets = Math.ceil(lsbLengthBits / 8);
  const bytes = normalized.match(/.{2}/g)!.map((value) => Number.parseInt(value, 16));
  const lsbBytes = bytes.slice(firstLsbOctet, firstLsbOctet + lsbOctets);

  const partialBits = lsbLengthBits % 8;
  if (partialBits !== 0 && lsbBytes.length > 0) {
    lsbBytes[lsbBytes.length - 1] &= 0xff << (8 - partialBits);
  }

  return {
    commonHex: bytesToHex(bytes.slice(0, commonOctets)),
    lsbLengthBits,
    lsbHex: bytesToHex(lsbBytes),
  };
}

function hrpdSystemRecordLength(lsbLengthBits: number, commonIncluded: boolean): number {
  // Five framing bits, sixteen common fields, eleven HRPD identity fields,
  // optional 12-bit common offset, the subnet, roaming indicator, and flag.
  const bits = 41 + lsbLengthBits + (commonIncluded ? 12 : 0);
  return Math.ceil(bits / 8);
}

function bytesToHex(bytes: number[]): string {
  return bytes.map((value) => value.toString(16).padStart(2, "0")).join("");
}
