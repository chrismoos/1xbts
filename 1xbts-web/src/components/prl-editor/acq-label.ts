import {
  PrlAbSelection,
  PrlAcqRecord,
  PrlExtAcqRecord,
  PrlPcsBlock,
  PrlStandardChannel,
} from "@/lib/proto/hlr/v1/service";
import {
  AB_OPTIONS,
  CLASSIC_ACQ_TYPE_OPTIONS,
  EXTENDED_ACQ_TYPE_OPTIONS,
  PCS_BLOCK_OPTIONS,
  STD_CHAN_OPTIONS,
} from "@/lib/prl-options";

function typeLabelForRaw(raw: number, mode: "classic" | "extended"): string {
  const options =
    mode === "extended" ? EXTENDED_ACQ_TYPE_OPTIONS : CLASSIC_ACQ_TYPE_OPTIONS;
  const hit = options.find((o) => o.value === raw);
  return hit ? hit.label.replace(/\s*\([01]+\)$/, "") : `Type 0x${raw.toString(16)}`;
}

export function acqShortLabel(
  record: PrlAcqRecord | PrlExtAcqRecord,
  mode: "classic" | "extended",
): string {
  return typeLabelForRaw(record.acqTypeRaw, mode);
}

export function acqRowSummary(
  index: number,
  record: PrlAcqRecord | PrlExtAcqRecord,
  mode: "classic" | "extended",
): string {
  return `ACQ #${index} · ${acqShortLabel(record, mode)}`;
}

function optionLabel<T>(
  value: T,
  options: Array<{ value: T; label: string }>,
): string {
  return options.find((option) => option.value === value)?.label ?? String(value);
}

function abLabel(value: PrlAbSelection): string {
  return optionLabel(value, AB_OPTIONS).replace(/\s*\([^)]*\)$/, "");
}

function standardChannelLabel(value: PrlStandardChannel): string {
  return optionLabel(value, STD_CHAN_OPTIONS).replace(/\s*\([^)]*\)$/, "");
}

function channelList(label: string, channels: number[]): string {
  return channels.length > 0
    ? `${label} ${channels.join(", ")}`
    : `No ${label.toLowerCase()}`;
}

export function acqDetailSummary(
  record: PrlAcqRecord | PrlExtAcqRecord,
): string {
  if (record.cellularAnalog) return abLabel(record.cellularAnalog.ab);
  if (record.cellularCdmaStandard) {
    return `${abLabel(record.cellularCdmaStandard.ab)} · ${standardChannelLabel(record.cellularCdmaStandard.priSec)}`;
  }
  if (record.cellularCdmaCustom) {
    return channelList("Channels", record.cellularCdmaCustom.channels);
  }
  if (record.cellularCdmaPreferred) {
    return abLabel(record.cellularCdmaPreferred.ab);
  }
  if (record.pcsCdmaUsingBlocks) {
    const blocks = record.pcsCdmaUsingBlocks.blocks.map((block) =>
      optionLabel(block as PrlPcsBlock, PCS_BLOCK_OPTIONS).replace(/^Block /, ""),
    );
    return blocks.length > 0 ? `Blocks ${blocks.join(", ")}` : "No blocks";
  }
  if (record.pcsCdmaUsingChannels) {
    return channelList("Channels", record.pcsCdmaUsingChannels.channels);
  }
  if (record.jtacsCdmaStandard) {
    return `${abLabel(record.jtacsCdmaStandard.ab)} · ${standardChannelLabel(record.jtacsCdmaStandard.priSec)}`;
  }
  if (record.jtacsCdmaCustom) {
    return channelList("Channels", record.jtacsCdmaCustom.channels);
  }
  if (record.bandClass6UsingChannels) {
    return channelList("Channels", record.bandClass6UsingChannels.channels);
  }
  if ("generic1xIs95" in record && record.generic1xIs95) {
    const entries = record.generic1xIs95.entries.map(
      (entry) => `Band ${entry.bandClass} / Channel ${entry.channelNumber}`,
    );
    return entries.length > 0 ? entries.join(", ") : "No band/channel entries";
  }
  if ("genericHrpd" in record && record.genericHrpd) {
    const entries = record.genericHrpd.entries.map(
      (entry) => `Band ${entry.bandClass} / Channel ${entry.channelNumber}`,
    );
    return entries.length > 0 ? entries.join(", ") : "No band/channel entries";
  }
  if ("umbCommonTable" in record && record.umbCommonTable) {
    const profiles = record.umbCommonTable.entries.map(
      (entry) => `Profile ${entry.umbAcqProfile}`,
    );
    return profiles.length > 0 ? profiles.join(", ") : "No UMB profiles";
  }
  if ("genericUmb" in record && record.genericUmb) {
    const blocks = record.genericUmb.blocks.map(
      (block) =>
        `Band ${block.bandClass} / Channel ${block.channelNumber} / Profile ${block.umbAcqTableProfile}`,
    );
    return blocks.length > 0 ? blocks.join(", ") : "No UMB blocks";
  }
  if ("other" in record && record.other) {
    return `${record.other.raw.length} raw bytes`;
  }
  return `Raw type 0x${record.acqTypeRaw.toString(16).padStart(2, "0")}`;
}

export function acqBandClasses(
  record: PrlAcqRecord | PrlExtAcqRecord,
): number[] {
  if (
    record.cellularAnalog ||
    record.cellularCdmaStandard ||
    record.cellularCdmaCustom ||
    record.cellularCdmaPreferred
  ) {
    return [0];
  }
  if (record.pcsCdmaUsingBlocks || record.pcsCdmaUsingChannels) return [1];
  if (record.jtacsCdmaStandard || record.jtacsCdmaCustom) return [3];
  if (record.bandClass6UsingChannels) return [6];
  if ("generic1xIs95" in record && record.generic1xIs95) {
    return uniqueNumbers(record.generic1xIs95.entries.map((entry) => entry.bandClass));
  }
  if ("genericHrpd" in record && record.genericHrpd) {
    return uniqueNumbers(record.genericHrpd.entries.map((entry) => entry.bandClass));
  }
  if ("genericUmb" in record && record.genericUmb) {
    return uniqueNumbers(record.genericUmb.blocks.map((block) => block.bandClass));
  }
  return [];
}

export function acqChannels(
  record: PrlAcqRecord | PrlExtAcqRecord,
): number[] {
  if (record.cellularCdmaCustom) return record.cellularCdmaCustom.channels;
  if (record.pcsCdmaUsingChannels) return record.pcsCdmaUsingChannels.channels;
  if (record.jtacsCdmaCustom) return record.jtacsCdmaCustom.channels;
  if (record.bandClass6UsingChannels) {
    return record.bandClass6UsingChannels.channels;
  }
  if ("generic1xIs95" in record && record.generic1xIs95) {
    return record.generic1xIs95.entries.map((entry) => entry.channelNumber);
  }
  if ("genericHrpd" in record && record.genericHrpd) {
    return record.genericHrpd.entries.map((entry) => entry.channelNumber);
  }
  if ("genericUmb" in record && record.genericUmb) {
    return record.genericUmb.blocks.map((block) => block.channelNumber);
  }
  return [];
}

export function acqPcsBlocks(
  record: PrlAcqRecord | PrlExtAcqRecord,
): number[] {
  return record.pcsCdmaUsingBlocks?.blocks.map(Number) ?? [];
}

function uniqueNumbers(values: number[]): number[] {
  return [...new Set(values)].sort((a, b) => a - b);
}
