import {
  type PrlAcqRecord,
  type PrlCommonSubnetRecord,
  type PrlExtAcqRecord,
  type PrlExtSysRecord,
  PrlPrefNeg,
  type PrlSysRecord,
} from "@/lib/proto/hlr/v1/service";
import { acqDetailSummary, acqShortLabel } from "./acq-label";
import { formatRoamingIndicator } from "@/lib/prl-options";
import { type EditorState, modeOf } from "./state";
import { systemReferenceLabel } from "./system-label";

export interface ReviewFieldChange {
  label: string;
  before: string;
  after: string;
}

export interface ReviewRecordChange {
  added: boolean;
  deleted: boolean;
  changed: boolean;
  moved: boolean;
  fromIndex?: number;
  toIndex?: number;
  beforeSummary?: string;
  afterSummary?: string;
  fields: ReviewFieldChange[];
}

export interface ReviewSection {
  title: string;
  changes: ReviewRecordChange[];
}

export interface SaveReview {
  fields: ReviewFieldChange[];
  sections: ReviewSection[];
  counts: {
    added: number;
    deleted: number;
    changed: number;
    moved: number;
  };
}

export function buildSaveReview(
  state: EditorState,
  metadata: {
    savedName: string;
    name: string;
    savedNotes: string;
    notes: string;
  },
): SaveReview {
  const mode = modeOf(state.draft);
  const beforeHeader = state.loaded.extended ?? state.loaded.classic!;
  const afterHeader = state.draft.extended ?? state.draft.classic!;
  const fields: ReviewFieldChange[] = [];

  addField(fields, "Name", metadata.savedName, metadata.name);
  addField(fields, "Notes", metadata.savedNotes, metadata.notes);
  addField(fields, "PRL ID", beforeHeader.prListId, afterHeader.prListId);
  addField(
    fields,
    "Allow unlisted systems",
    !beforeHeader.prefOnly,
    !afterHeader.prefOnly,
  );
  addField(
    fields,
    "Status for unlisted systems",
    formatRoamingIndicator(beforeHeader.defRoamInd?.raw ?? 0),
    formatRoamingIndicator(afterHeader.defRoamInd?.raw ?? 0),
  );

  const sections: ReviewSection[] = [
    {
      title: "Acquisitions",
      changes: buildRecordChanges(
        state.loadedAcqIds,
        state.acqIds,
        (state.loaded.classic ?? state.loaded.extended)!.acquisitionRecords,
        (state.draft.classic ?? state.draft.extended)!.acquisitionRecords,
        (record, index) => acqSummary(record, index, mode),
      ),
    },
    {
      title: "Systems",
      changes: buildRecordChanges(
        state.loadedSysIds,
        state.sysIds,
        (state.loaded.classic ?? state.loaded.extended)!.systemRecords,
        (state.draft.classic ?? state.draft.extended)!.systemRecords,
        systemSummary,
      ),
    },
    ...(mode === "extended"
      ? [
          {
            title: "Common subnets",
            changes: buildRecordChanges(
              state.loadedSubnetIds,
              state.subnetIds,
              state.loaded.extended!.commonSubnetRecords,
              state.draft.extended!.commonSubnetRecords,
              subnetSummary,
            ),
          },
        ]
      : []),
  ].filter((section) => section.changes.length > 0);

  const allRecords = sections.flatMap((section) => section.changes);
  return {
    fields,
    sections,
    counts: {
      added: allRecords.filter((change) => change.added).length,
      deleted: allRecords.filter((change) => change.deleted).length,
      changed:
        fields.length + allRecords.filter((change) => change.changed).length,
      moved: allRecords.filter((change) => change.moved).length,
    },
  };
}

function addField(
  changes: ReviewFieldChange[],
  label: string,
  before: unknown,
  after: unknown,
) {
  if (JSON.stringify(before) === JSON.stringify(after)) return;
  changes.push({ label, before: formatValue(before), after: formatValue(after) });
}

function buildRecordChanges<T>(
  loadedIds: string[],
  currentIds: string[],
  loadedRecords: T[],
  currentRecords: T[],
  summary: (record: T, index: number) => string,
): ReviewRecordChange[] {
  const loadedIndex = new Map(loadedIds.map((id, index) => [id, index]));
  const currentIndex = new Map(currentIds.map((id, index) => [id, index]));
  const movedIds = movedRecordIds(loadedIds, currentIds);
  const changes: ReviewRecordChange[] = [];

  loadedIds.forEach((id, index) => {
    if (currentIndex.has(id)) return;
    changes.push({
      added: false,
      deleted: true,
      changed: false,
      moved: false,
      fromIndex: index,
      beforeSummary: summary(loadedRecords[index], index),
      fields: [],
    });
  });

  currentIds.forEach((id, index) => {
    const oldIndex = loadedIndex.get(id);
    if (oldIndex == null) {
      changes.push({
        added: true,
        deleted: false,
        changed: false,
        moved: false,
        toIndex: index,
        afterSummary: summary(currentRecords[index], index),
        fields: [],
      });
      return;
    }

    const before = loadedRecords[oldIndex];
    const after = currentRecords[index];
    const changed = JSON.stringify(before) !== JSON.stringify(after);
    const moved = movedIds.has(id);
    if (!changed && !moved) return;
    changes.push({
      added: false,
      deleted: false,
      changed,
      moved,
      fromIndex: oldIndex,
      toIndex: index,
      beforeSummary: summary(before, oldIndex),
      afterSummary: summary(after, index),
      fields: changed ? diffFields(before, after) : [],
    });
  });

  return changes;
}

function movedRecordIds(loadedIds: string[], currentIds: string[]): Set<string> {
  const originalPosition = new Map(loadedIds.map((id, index) => [id, index]));
  const retained = currentIds.filter((id) => originalPosition.has(id));
  const sequence = retained.map((id) => originalPosition.get(id)!);
  const stableIndexes = longestIncreasingSubsequenceIndexes(sequence);
  return new Set(retained.filter((_id, index) => !stableIndexes.has(index)));
}

function longestIncreasingSubsequenceIndexes(values: number[]): Set<number> {
  const tails: number[] = [];
  const tailsIndexes: number[] = [];
  const previous = new Array<number>(values.length).fill(-1);

  values.forEach((value, index) => {
    let low = 0;
    let high = tails.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (tails[middle] < value) low = middle + 1;
      else high = middle;
    }
    if (low > 0) previous[index] = tailsIndexes[low - 1];
    tails[low] = value;
    tailsIndexes[low] = index;
  });

  const result = new Set<number>();
  let cursor = tailsIndexes[tails.length - 1] ?? -1;
  while (cursor >= 0) {
    result.add(cursor);
    cursor = previous[cursor];
  }
  return result;
}

function diffFields(before: unknown, after: unknown, path = ""): ReviewFieldChange[] {
  if (JSON.stringify(before) === JSON.stringify(after)) return [];
  if (
    !isPlainObject(before) ||
    !isPlainObject(after) ||
    Array.isArray(before) ||
    Array.isArray(after) ||
    before instanceof Uint8Array ||
    after instanceof Uint8Array
  ) {
    return [
      {
        label: fieldLabel(path),
        before: formatValue(before),
        after: formatValue(after),
      },
    ];
  }

  const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
  return [...keys].flatMap((key) =>
    diffFields(
      (before as Record<string, unknown>)[key],
      (after as Record<string, unknown>)[key],
      path ? `${path}.${key}` : key,
    ),
  );
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function fieldLabel(path: string): string {
  const key = path.split(".").at(-1) ?? "Value";
  const labels: Record<string, string> = {
    acqIndex: "Acquisition",
    acqTypeRaw: "Acquisition type",
    prefNeg: "Preference",
    sameGeoAsPrev: "Same geography",
    roamingIndicator: "Roaming status",
    raw: path.endsWith("roamingIndicator.raw") ? "Roaming status" : "Raw value",
    bandClass: "Band class",
    channelNumber: "Channel",
    nidIncl: "NID matching",
    subnetLsbHex: "Subnet",
    subnetLsbLengthBits: "Subnet length",
    subnetCommonOffset: "Common subnet",
  };
  return (
    labels[key] ??
    key.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/^./, (char) => char.toUpperCase())
  );
}

function formatValue(value: unknown): string {
  if (value === undefined || value === null || value === "") return "—";
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (value instanceof Uint8Array) {
    return [...value].map((byte) => byte.toString(16).padStart(2, "0")).join("") || "—";
  }
  if (typeof value === "object") {
    return compact(JSON.stringify(value));
  }
  return compact(String(value));
}

function compact(value: string): string {
  const singleLine = value.replace(/\s+/g, " ").trim();
  return singleLine.length > 120 ? `${singleLine.slice(0, 117)}…` : singleLine || "—";
}

function acqSummary(
  record: PrlAcqRecord | PrlExtAcqRecord,
  index: number,
  mode: "classic" | "extended",
): string {
  return `ACQ #${index} · ${acqShortLabel(record, mode)} · ${acqDetailSummary(record)}`;
}

function systemSummary(
  record: PrlSysRecord | PrlExtSysRecord,
  index: number,
): string {
  const preference =
    record.prefNeg === PrlPrefNeg.PRL_PREF_NEG_PREFERRED ? "Preferred" : "Negative";
  const roaming = record.roamingIndicator
    ? formatRoamingIndicator(record.roamingIndicator.raw)
    : "No roaming status";
  return `${systemReferenceLabel(record, index)} · ACQ #${record.acqIndex} · ${preference} · ${roaming}`;
}

function subnetSummary(record: PrlCommonSubnetRecord, index: number): string {
  return `Subnet #${index} · ${record.subnetCommonLengthOctets} octets · ${record.subnetCommonHex || "empty"}`;
}
