import { useState } from "react";
import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { EditorMode } from "../state";
import { ErrorMap } from "../validation";
import { acqDetailSummary, acqRowSummary } from "../acq-label";
import { AcqRowEditor } from "../acq/AcqRowEditor";
import { SearchableSelect } from "./SearchableSelect";

export function AcqIndexPicker({
  value,
  onChange,
  mode,
  acqRecords,
  patchAcq,
  errors,
  errorPrefix,
  error,
}: {
  value: number;
  onChange: (next: number) => void;
  mode: EditorMode;
  acqRecords: (PrlAcqRecord | PrlExtAcqRecord)[];
  patchAcq: (
    index: number,
    mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void,
  ) => void;
  errors: ErrorMap;
  errorPrefix: string;
  error?: string;
}) {
  const [showDetails, setShowDetails] = useState(false);
  const selected = acqRecords[value];

  return (
    <div className="col-span-full">
      <div className="flex items-end gap-2">
        <div className="block flex-1">
          <span className="text-muted text-[11px]">ACQ_INDEX</span>
          <SearchableSelect
            className="mt-1"
            value={String(value)}
            options={
              acqRecords.length === 0
                ? [{ value: "0", label: "No ACQ rows yet" }]
                : acqRecords.map((record, index) => ({
                    value: String(index),
                    label: acqRowSummary(index, record, mode),
                    searchText: `${acqDetailSummary(record)} ${JSON.stringify(record)}`,
                  }))
            }
            onChange={(next) => onChange(Number(next))}
            placeholder="Search acquisition records…"
            ariaLabel="ACQ_INDEX"
            invalid={Boolean(error)}
          />
          {error && (
            <span className="text-accent-red text-[10px]">{error}</span>
          )}
        </div>
        <button
          type="button"
          onClick={() => setShowDetails((v) => !v)}
          disabled={!selected}
          className="text-accent-blue text-[11px] hover:underline disabled:opacity-30 pb-1"
        >
          {showDetails ? "Hide acq details" : "Show acq details"}
        </button>
      </div>
      {showDetails && selected && (
        <div className="mt-2 border border-border rounded p-2 bg-bg/30">
          <div className="text-[11px] text-dimmed mb-2">
            Editing ACQ #{value} — changes here update the shared
            acquisition record and affect every SYS row that references it.
          </div>
          <AcqRowEditor
            mode={mode}
            record={selected}
            onPatch={(mutator) => patchAcq(value, mutator)}
            errors={errors}
            errorPrefix={`${errorPrefix}.acq[${value}]`}
          />
        </div>
      )}
    </div>
  );
}
