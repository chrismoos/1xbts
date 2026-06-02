import { Dispatch } from "react";
import { Card } from "@/components/card";
import { EditorState, EditorAction, subnetRecordsOf } from "./state";
import { ErrorMap } from "./validation";
import { emptyCommonSubnet } from "./builders";
import { NumericInput } from "./shared/NumericInput";
import { HexBytesInput } from "./shared/HexBytesInput";
import { SortableList, SortableRow, DragHandle } from "./shared/SortableList";

export function CommonSubnetTab({
  state,
  dispatch,
  errors,
}: {
  state: EditorState;
  dispatch: Dispatch<EditorAction>;
  errors: ErrorMap;
}) {
  const rows = subnetRecordsOf(state.draft);

  return (
    <Card title={`Common Subnet Table (${rows.length})`}>
      <p className="text-dimmed text-[11px] mb-3">
        Common subnet records are referenced by HRPD system records'
        SUBNET_COMMON_OFFSET. Each record carries a number of octets of
        most-significant HRPD subnet bits.
      </p>
      <button
        onClick={() => dispatch({ type: "addSubnet", record: emptyCommonSubnet() })}
        className="text-accent-blue text-xs hover:underline mb-3"
      >
        + Add row
      </button>
      {rows.length === 0 ? (
        <p className="text-dimmed text-xs">No common subnet records.</p>
      ) : (
        <SortableList
          ids={state.subnetIds}
          onReorder={(from, to) =>
            dispatch({ type: "reorderSubnet", from, to })
          }
        >
        <div className="space-y-2">
          {rows.map((r, index) => (
            <SortableRow key={state.subnetIds[index]} id={state.subnetIds[index]}>
              {(drag) => (
            <div className="border border-border rounded p-2 bg-bg/40">
              <div className="flex items-center gap-2 mb-2 text-xs">
                <DragHandle {...drag} />
                <span className="font-mono text-muted text-[11px]">
                  #{index}
                </span>
                <span className="text-dimmed">
                  Referenced by SUBNET_COMMON_OFFSET = {index}
                </span>
                <button
                  onClick={() => dispatch({ type: "removeSubnet", index })}
                  className="ml-auto text-accent-red text-[11px] hover:underline"
                >
                  Remove
                </button>
              </div>
              <div className="grid grid-cols-2 gap-2">
                <NumericInput
                  label="SUBNET_COMMON_LENGTH (octets)"
                  min={0}
                  max={15}
                  value={r.subnetCommonLengthOctets}
                  onChange={(v) =>
                    dispatch({
                      type: "patchSubnet",
                      index,
                      mutator: (d) => {
                        d.subnetCommonLengthOctets = v;
                        const want = v * 2;
                        if (d.subnetCommonHex.length < want)
                          d.subnetCommonHex =
                            d.subnetCommonHex +
                            "0".repeat(want - d.subnetCommonHex.length);
                        else if (d.subnetCommonHex.length > want)
                          d.subnetCommonHex = d.subnetCommonHex.slice(0, want);
                      },
                    })
                  }
                  error={errors.get(`subnet[${index}].subnetCommonLengthOctets`)}
                />
                <HexBytesInput
                  label="SUBNET_COMMON (hex)"
                  lengthBits={r.subnetCommonLengthOctets * 8}
                  value={r.subnetCommonHex}
                  onChange={(v) =>
                    dispatch({
                      type: "patchSubnet",
                      index,
                      mutator: (d) => void (d.subnetCommonHex = v),
                    })
                  }
                  error={errors.get(`subnet[${index}].subnetCommonHex`)}
                />
              </div>
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
