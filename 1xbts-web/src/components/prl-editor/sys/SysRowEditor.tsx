import {
  PrlAcqRecord,
  PrlExtAcqRecord,
  PrlExtSysRecord,
  PrlSysRecord,
} from "@/lib/proto/hlr/v1/service";
import { EditorMode } from "../state";
import { ErrorMap } from "../validation";
import { ClassicSysEditor } from "./ClassicSys";
import { ExtCdma2000Editor } from "./ExtCdma2000";
import { ExtHrpdEditor } from "./ExtHrpd";
import { ExtMccMncEditor } from "./ExtMccMnc";

export interface AcqPickerContext {
  acqRecords: (PrlAcqRecord | PrlExtAcqRecord)[];
  patchAcq: (
    index: number,
    mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void,
  ) => void;
}

export function SysRowEditor({
  mode,
  record,
  acq,
  subnetCount,
  onPatch,
  errors,
  errorPrefix,
}: {
  mode: EditorMode;
  record: PrlSysRecord | PrlExtSysRecord;
  acq: AcqPickerContext;
  subnetCount: number;
  onPatch: (
    mutator: (draft: PrlSysRecord | PrlExtSysRecord) => void
  ) => void;
  errors: ErrorMap;
  errorPrefix: string;
}) {
  if (mode === "classic") {
    return (
      <ClassicSysEditor
        record={record as PrlSysRecord}
        onPatch={onPatch as (m: (d: PrlSysRecord) => void) => void}
        mode={mode}
        acq={acq}
        errors={errors}
        errorPrefix={errorPrefix}
      />
    );
  }

  const r = record as PrlExtSysRecord;
  if (r.cdma2000) {
    return (
      <ExtCdma2000Editor
        record={r}
        onPatch={onPatch as (m: (d: PrlExtSysRecord) => void) => void}
        mode={mode}
        acq={acq}
        errors={errors}
        errorPrefix={errorPrefix}
      />
    );
  }
  if (r.hrpd) {
    return (
      <ExtHrpdEditor
        record={r}
        onPatch={onPatch as (m: (d: PrlExtSysRecord) => void) => void}
        mode={mode}
        acq={acq}
        subnetCount={subnetCount}
        errors={errors}
        errorPrefix={errorPrefix}
      />
    );
  }
  if (r.mccMnc) {
    return (
      <ExtMccMncEditor
        record={r}
        onPatch={onPatch as (m: (d: PrlExtSysRecord) => void) => void}
        mode={mode}
        acq={acq}
        errors={errors}
        errorPrefix={errorPrefix}
      />
    );
  }

  return (
    <p className="text-dimmed">Unsupported system record subtype.</p>
  );
}
