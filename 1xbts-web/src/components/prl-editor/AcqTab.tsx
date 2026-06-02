import { Dispatch } from "react";
import { Card } from "@/components/card";
import { EditorState, EditorAction, modeOf, acqRecordsOf } from "./state";
import { ErrorMap } from "./validation";
import { emptyClassicAcq, emptyExtAcq } from "./builders";
import {
  CLASSIC_ACQ_TYPE_OPTIONS,
  EXTENDED_ACQ_TYPE_OPTIONS,
} from "@/lib/prl-options";
import { SortableList, SortableRow, DragHandle } from "./shared/SortableList";
import { AcqRowEditor } from "./acq/AcqRowEditor";

export function AcqTab({
  state,
  dispatch,
  errors,
}: {
  state: EditorState;
  dispatch: Dispatch<EditorAction>;
  errors: ErrorMap;
}) {
  const mode = modeOf(state.draft);
  const records = acqRecordsOf(state.draft);
  const options =
    mode === "extended" ? EXTENDED_ACQ_TYPE_OPTIONS : CLASSIC_ACQ_TYPE_OPTIONS;

  const addRow = (acqTypeRaw: number) => {
    const record =
      mode === "extended" ? emptyExtAcq(acqTypeRaw) : emptyClassicAcq(acqTypeRaw);
    dispatch({ type: "addAcq", record });
  };

  return (
    <Card title={`ACQ_TABLE (${records.length})`}>
      <div className="flex items-center gap-2 mb-3 text-xs">
        <label className="text-muted">Add row:</label>
        <select
          className="bg-bg border border-border rounded px-2 py-1"
          defaultValue=""
          onChange={(e) => {
            if (e.target.value) {
              addRow(Number(e.target.value));
              e.target.value = "";
            }
          }}
        >
          <option value="">— Pick a type —</option>
          {options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </div>

      {records.length === 0 ? (
        <p className="text-dimmed text-xs">
          No acquisition records yet. Add one to get started — system records
          reference these by ACQ_INDEX.
        </p>
      ) : (
        <SortableList
          ids={state.acqIds}
          onReorder={(from, to) => dispatch({ type: "reorderAcq", from, to })}
        >
          <div className="space-y-2">
            {records.map((rec, index) => (
              <SortableRow key={state.acqIds[index]} id={state.acqIds[index]}>
                {(drag) => (
                  <div className="border border-border rounded p-2 bg-bg/40">
                    <div className="flex items-center gap-2 mb-2">
                      <DragHandle {...drag} />
                      <span className="font-mono text-muted text-[11px]">
                        #{index}
                      </span>
                      <button
                        onClick={() => dispatch({ type: "removeAcq", index })}
                        className="ml-auto text-accent-red text-[11px] hover:underline"
                      >
                        Remove
                      </button>
                    </div>
                    <AcqRowEditor
                      mode={mode}
                      record={rec}
                      onPatch={(mutator) =>
                        dispatch({ type: "patchAcq", index, mutator })
                      }
                      errors={errors}
                      errorPrefix={`acq[${index}]`}
                    />
                  </div>
                )}
              </SortableRow>
            ))}
          </div>
        </SortableList>
      )}
    </Card>
  );
}
