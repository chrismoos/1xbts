import { Dispatch } from "react";
import { Card } from "@/components/card";
import { EditorState, modeOf } from "./state";
import { ErrorMap } from "./validation";
import { EditorAction } from "./state";
import { NumericInput } from "./shared/NumericInput";
import { RoamingIndicatorSelect } from "./shared/RoamingIndicatorSelect";

export interface PrlGeneralMetadata {
  savedName: string;
  name: string;
  savedNotes: string;
  notes: string;
  isDefault: boolean;
  rawBytesSize: number;
  busy: boolean;
  onNameChange: (value: string) => void;
  onNotesChange: (value: string) => void;
  onSetDefault: () => void;
  onDelete: () => void;
}

export function HeaderTab({
  state,
  dispatch,
  errors,
  metadata,
}: {
  state: EditorState;
  dispatch: Dispatch<EditorAction>;
  errors: ErrorMap;
  metadata: PrlGeneralMetadata;
}) {
  const mode = modeOf(state.draft);
  const hdr = mode === "extended" ? state.draft.extended! : state.draft.classic!;

  const setPrListId = (v: number) => {
    if (mode === "extended") {
      dispatch({
        type: "patchExtended",
        mutator: (d) => {
          d.prListId = v;
        },
      });
    } else {
      dispatch({
        type: "patchClassic",
        mutator: (d) => {
          d.prListId = v;
        },
      });
    }
  };

  const setPrefOnly = (v: boolean) => {
    if (mode === "extended") {
      dispatch({
        type: "patchExtended",
        mutator: (d) => {
          d.prefOnly = v;
        },
      });
    } else {
      dispatch({
        type: "patchClassic",
        mutator: (d) => {
          d.prefOnly = v;
        },
      });
    }
  };

  const setDefRoamIndRaw = (v: number) => {
    if (mode === "extended") {
      dispatch({
        type: "patchExtended",
        mutator: (d) => {
          if (d.defRoamInd) d.defRoamInd.raw = v;
        },
      });
    } else {
      dispatch({
        type: "patchClassic",
        mutator: (d) => {
          if (d.defRoamInd) d.defRoamInd.raw = v;
        },
      });
    }
  };

  return (
    <Card title="General settings" className="overflow-visible">
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3 text-xs">
        <label className="block">
          <span className="text-muted text-[11px]">Name</span>
          <input
            className={`block w-full mt-0.5 bg-bg border rounded px-2 py-1 ${
              metadata.name.trim() ? "border-border" : "border-accent-red"
            }`}
            value={metadata.name}
            onChange={(event) => metadata.onNameChange(event.target.value)}
            maxLength={120}
          />
          {!metadata.name.trim() && (
            <span className="text-accent-red text-[10px]">Name is required.</span>
          )}
        </label>
        <NumericInput
          label="PRL ID"
          hint="0–65535"
          value={hdr.prListId}
          onChange={setPrListId}
          min={0}
          max={0xffff}
          error={errors.get("header.prListId")}
        />
        <label className="block md:col-span-2">
          <span className="text-muted text-[11px]">Notes</span>
          <textarea
            className="block w-full mt-0.5 bg-bg border border-border rounded px-2 py-1 font-mono"
            rows={2}
            value={metadata.notes}
            onChange={(event) => metadata.onNotesChange(event.target.value)}
          />
        </label>
        <div className="text-dimmed">
          <span className="text-muted text-[11px]">PRL format</span>
          <div className="mt-1">
            {mode === "extended" ? "Extended (revision 3)" : "Classic (revision 1)"}
          </div>
        </div>
        <div className="text-dimmed">
          <span className="text-muted text-[11px]">Encoded size</span>
          <div className="mt-1 font-mono">{metadata.rawBytesSize} octets</div>
        </div>
        <div>
          <span className="text-muted text-[11px]">Deployment</span>
          <div className="mt-1">
            {metadata.isDefault ? (
              <span className="text-accent-green">✓ Default PRL</span>
            ) : (
              <button
                type="button"
                onClick={metadata.onSetDefault}
                disabled={metadata.busy}
                className="text-accent-blue hover:underline disabled:opacity-50"
              >
                Make this the default PRL
              </button>
            )}
          </div>
        </div>
        <label className="block">
          <span className="text-muted text-[11px]">Unlisted systems</span>
          <div className="mt-1">
            <label className="inline-flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={!hdr.prefOnly}
                onChange={(e) => setPrefOnly(!e.target.checked)}
                className="rounded"
              />
              <span>Allow systems not listed in this PRL</span>
            </label>
          </div>
        </label>
        <RoamingIndicatorSelect
          label="Status for unlisted systems"
          hint="What the phone displays when it uses a system not listed here"
          value={hdr.defRoamInd?.raw ?? 0}
          onChange={setDefRoamIndRaw}
          error={errors.get("header.defRoamInd")}
        />
        <div className="flex items-end justify-end md:col-span-2">
          <button
            type="button"
            onClick={metadata.onDelete}
            disabled={metadata.busy}
            className="text-accent-red hover:underline disabled:opacity-50"
          >
            Delete PRL
          </button>
        </div>
      </div>
    </Card>
  );
}
