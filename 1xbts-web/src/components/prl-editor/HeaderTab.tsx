import { Dispatch } from "react";
import { Card } from "@/components/card";
import { EditorState, modeOf } from "./state";
import { ErrorMap } from "./validation";
import { EditorAction } from "./state";
import { NumericInput } from "./shared/NumericInput";
import { RoamingIndicatorSelect } from "./shared/RoamingIndicatorSelect";

export function HeaderTab({
  state,
  dispatch,
  errors,
}: {
  state: EditorState;
  dispatch: Dispatch<EditorAction>;
  errors: ErrorMap;
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
    <Card title={`Header (${mode === "extended" ? "Extended" : "Classic"} PRL)`}>
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3 text-xs">
        <NumericInput
          label="PR_LIST_ID"
          hint="0–65535"
          value={hdr.prListId}
          onChange={setPrListId}
          min={0}
          max={0xffff}
          error={errors.get("header.prListId")}
        />
        <label className="block">
          <span className="text-muted text-[11px]">PREF_ONLY</span>
          <div className="mt-1">
            <label className="inline-flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={hdr.prefOnly}
                onChange={(e) => setPrefOnly(e.target.checked)}
                className="rounded"
              />
              <span>
                {hdr.prefOnly
                  ? "Only operate on listed systems"
                  : "Allow non-listed systems too"}
              </span>
            </label>
          </div>
        </label>
        <RoamingIndicatorSelect
          label="DEF_ROAM_IND"
          hint="Indicator shown for systems not in SYS_TABLE"
          value={hdr.defRoamInd?.raw ?? 0}
          onChange={setDefRoamIndRaw}
          error={errors.get("header.defRoamInd")}
        />
        {mode === "extended" && (
          <div className="text-dimmed">
            <span className="text-muted text-[11px]">CUR_SSPR_P_REV</span>
            <div className="mt-1 font-mono">3 (fixed)</div>
          </div>
        )}
      </div>
      <p className="text-dimmed text-[11px] mt-3">
        PR_LIST_SIZE and PR_LIST_CRC are computed by the server on save.
      </p>
    </Card>
  );
}
