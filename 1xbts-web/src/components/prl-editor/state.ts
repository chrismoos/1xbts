// Reducer + state for the PRL structured editor.
//
// ts-proto generates flattened oneofs: `PrlDecoded` has both
// `classic?` and `extended?` as optional fields; exactly one is set.
// Same for `PrlAcqRecord` (one of ~10 acquisition body fields),
// `PrlExtAcqRecord`, `PrlExtSysRecord`, `PrlExtSysIdMccMnc`.

import { produce } from "immer";
import { v4 as uuid } from "@/lib/uuid-lite";

import {
  PrlDecoded,
  PrlClassicBody,
  PrlExtendedBody,
  PrlAcqRecord,
  PrlExtAcqRecord,
  PrlSysRecord,
  PrlExtSysRecord,
  PrlCommonSubnetRecord,
} from "@/lib/proto/hlr/v1/service";

export type EditorMode = "classic" | "extended";

export interface EditorState {
  loaded: PrlDecoded;
  draft: PrlDecoded;
  acqIds: string[];
  sysIds: string[];
  subnetIds: string[];
}

export type EditorAction =
  | { type: "load"; payload: PrlDecoded }
  | { type: "patchClassic"; mutator: (draft: PrlClassicBody) => void }
  | { type: "patchExtended"; mutator: (draft: PrlExtendedBody) => void }
  | { type: "addAcq"; record: PrlAcqRecord | PrlExtAcqRecord }
  | { type: "removeAcq"; index: number }
  | { type: "reorderAcq"; from: number; to: number }
  | {
      type: "patchAcq";
      index: number;
      mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void;
    }
  | { type: "addSys"; record: PrlSysRecord | PrlExtSysRecord }
  | { type: "removeSys"; index: number }
  | { type: "reorderSys"; from: number; to: number }
  | {
      type: "patchSys";
      index: number;
      mutator: (draft: PrlSysRecord | PrlExtSysRecord) => void;
    }
  | { type: "addSubnet"; record: PrlCommonSubnetRecord }
  | { type: "removeSubnet"; index: number }
  | { type: "reorderSubnet"; from: number; to: number }
  | {
      type: "patchSubnet";
      index: number;
      mutator: (draft: PrlCommonSubnetRecord) => void;
    };

export function modeOf(decoded: PrlDecoded): EditorMode {
  return decoded.extended ? "extended" : "classic";
}

export function initialState(loaded: PrlDecoded): EditorState {
  const acqCount = acqRecordsOf(loaded).length;
  const sysCount = sysRecordsOf(loaded).length;
  const subnetCount = subnetRecordsOf(loaded).length;
  return {
    loaded,
    draft: structuredClone(loaded),
    acqIds: makeIds(acqCount),
    sysIds: makeIds(sysCount),
    subnetIds: makeIds(subnetCount),
  };
}

function makeIds(n: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < n; i++) out.push(uuid());
  return out;
}

export function acqRecordsOf(d: PrlDecoded): (PrlAcqRecord | PrlExtAcqRecord)[] {
  if (d.classic) return d.classic.acquisitionRecords;
  if (d.extended) return d.extended.acquisitionRecords;
  return [];
}

export function sysRecordsOf(d: PrlDecoded): (PrlSysRecord | PrlExtSysRecord)[] {
  if (d.classic) return d.classic.systemRecords;
  if (d.extended) return d.extended.systemRecords;
  return [];
}

export function subnetRecordsOf(d: PrlDecoded): PrlCommonSubnetRecord[] {
  if (d.extended) return d.extended.commonSubnetRecords;
  return [];
}

export function reducer(state: EditorState, action: EditorAction): EditorState {
  switch (action.type) {
    case "load":
      return initialState(action.payload);

    case "patchClassic":
      return produce(state, (s) => {
        if (s.draft.classic) action.mutator(s.draft.classic);
      });

    case "patchExtended":
      return produce(state, (s) => {
        if (s.draft.extended) action.mutator(s.draft.extended);
      });

    case "addAcq":
      return produce(state, (s) => {
        acqArrayMut(s.draft).push(action.record);
        s.acqIds.push(uuid());
      });

    case "removeAcq":
      return produce(state, (s) => {
        acqArrayMut(s.draft).splice(action.index, 1);
        s.acqIds.splice(action.index, 1);
        // Auto-fixup: every sys record whose acqIndex pointed at this
        // row should now point one earlier; rows that pointed AT this
        // index get clamped to 0 (their reference is gone — the
        // validator will flag this as an explicit error so the
        // operator knows it lost meaning).
        remapAcqIndex(s.draft, (i) => {
          if (i === action.index) return 0;
          return i > action.index ? i - 1 : i;
        });
      });

    case "reorderAcq":
      return produce(state, (s) => {
        moveItem(acqArrayMut(s.draft), action.from, action.to);
        moveItem(s.acqIds, action.from, action.to);
        remapAcqIndex(s.draft, (i) => remapAfterMove(i, action.from, action.to));
      });

    case "patchAcq":
      return produce(state, (s) => {
        const arr = acqArrayMut(s.draft);
        if (action.index >= 0 && action.index < arr.length) {
          action.mutator(arr[action.index]);
        }
      });

    case "addSys":
      return produce(state, (s) => {
        sysArrayMut(s.draft).push(action.record);
        s.sysIds.push(uuid());
      });

    case "removeSys":
      return produce(state, (s) => {
        sysArrayMut(s.draft).splice(action.index, 1);
        s.sysIds.splice(action.index, 1);
      });

    case "reorderSys":
      return produce(state, (s) => {
        moveItem(sysArrayMut(s.draft), action.from, action.to);
        moveItem(s.sysIds, action.from, action.to);
      });

    case "patchSys":
      return produce(state, (s) => {
        const arr = sysArrayMut(s.draft);
        if (action.index >= 0 && action.index < arr.length) {
          action.mutator(arr[action.index]);
        }
      });

    case "addSubnet":
      return produce(state, (s) => {
        const arr = subnetArrayMut(s.draft);
        if (arr) {
          arr.push(action.record);
          s.subnetIds.push(uuid());
        }
      });

    case "removeSubnet":
      return produce(state, (s) => {
        const arr = subnetArrayMut(s.draft);
        if (arr) {
          arr.splice(action.index, 1);
          s.subnetIds.splice(action.index, 1);
          // HRPD sys records reference subnet rows by offset.
          remapSubnetOffset(s.draft, (i) => {
            if (i === action.index) return undefined; // referenced row gone
            return i > action.index ? i - 1 : i;
          });
        }
      });

    case "reorderSubnet":
      return produce(state, (s) => {
        const arr = subnetArrayMut(s.draft);
        if (arr) {
          moveItem(arr, action.from, action.to);
          moveItem(s.subnetIds, action.from, action.to);
          remapSubnetOffset(s.draft, (i) =>
            remapAfterMove(i, action.from, action.to)
          );
        }
      });

    case "patchSubnet":
      return produce(state, (s) => {
        const arr = subnetArrayMut(s.draft);
        if (arr && action.index >= 0 && action.index < arr.length) {
          action.mutator(arr[action.index]);
        }
      });
  }
}

function moveItem<T>(arr: T[], from: number, to: number): void {
  if (from < 0 || from >= arr.length || to < 0 || to >= arr.length) return;
  const [item] = arr.splice(from, 1);
  arr.splice(to, 0, item);
}

/** Apply a renumbering function to every sys record's acqIndex. */
function remapAcqIndex(d: PrlDecoded, fn: (i: number) => number): void {
  const syss = sysRecordsOf(d);
  for (const s of syss) {
    s.acqIndex = fn(s.acqIndex);
  }
}

/** Apply a renumbering function to every HRPD sys record's subnetCommonOffset.
 *  Returning undefined drops the reference (and the include flag). */
function remapSubnetOffset(
  d: PrlDecoded,
  fn: (i: number) => number | undefined
): void {
  if (!d.extended) return;
  for (const s of d.extended.systemRecords) {
    if (s.hrpd && s.hrpd.subnetCommonIncluded && s.hrpd.subnetCommonOffset != null) {
      const next = fn(s.hrpd.subnetCommonOffset);
      if (next === undefined) {
        s.hrpd.subnetCommonIncluded = false;
        s.hrpd.subnetCommonOffset = undefined;
      } else {
        s.hrpd.subnetCommonOffset = next;
      }
    }
  }
}

/** Translate an old index to its new position after an array move. */
function remapAfterMove(i: number, from: number, to: number): number {
  if (i === from) return to;
  if (from < to) {
    if (i > from && i <= to) return i - 1;
  } else if (from > to) {
    if (i >= to && i < from) return i + 1;
  }
  return i;
}

function acqArrayMut(
  d: PrlDecoded
): (PrlAcqRecord | PrlExtAcqRecord)[] {
  if (d.classic) return d.classic.acquisitionRecords;
  if (d.extended) return d.extended.acquisitionRecords;
  return [];
}

function sysArrayMut(
  d: PrlDecoded
): (PrlSysRecord | PrlExtSysRecord)[] {
  if (d.classic) return d.classic.systemRecords;
  if (d.extended) return d.extended.systemRecords;
  return [];
}

function subnetArrayMut(d: PrlDecoded): PrlCommonSubnetRecord[] | null {
  if (d.extended) return d.extended.commonSubnetRecords;
  return null;
}

export function isDirty(state: EditorState): boolean {
  return JSON.stringify(state.draft) !== JSON.stringify(state.loaded);
}
