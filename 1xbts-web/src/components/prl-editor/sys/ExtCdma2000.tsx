import {
  PrlExtSysRecord,
  PrlNidInclusion,
} from "@/lib/proto/hlr/v1/service";
import {
  NID_INCL_OPTIONS,
  PREF_NEG_OPTIONS,
  PRIORITY_OPTIONS,
} from "@/lib/prl-options";
import { EnumSelect } from "../shared/EnumSelect";
import { NumericInput } from "../shared/NumericInput";
import { AcqIndexPicker } from "../shared/AcqIndexPicker";
import { RoamingIndicatorSelect } from "../shared/RoamingIndicatorSelect";
import { ErrorMap } from "../validation";
import { EditorMode } from "../state";
import { AcqPickerContext } from "./SysRowEditor";
import { defaultRoamingIndicator } from "../builders";

export function ExtCdma2000Editor({
  record,
  onPatch,
  mode,
  acq,
  errors,
  errorPrefix,
}: {
  record: PrlExtSysRecord;
  onPatch: (mutator: (draft: PrlExtSysRecord) => void) => void;
  mode: EditorMode;
  acq: AcqPickerContext;
  errors: ErrorMap;
  errorPrefix: string;
}) {
  const sysId = record.cdma2000!;
  const includesNid =
    sysId.nidIncl === PrlNidInclusion.PRL_NID_INCLUSION_SINGLE;

  return (
    <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
      <NumericInput
        label="SID"
        bits={15}
        value={sysId.sid}
        onChange={(v) =>
          onPatch((d) => {
            if (d.cdma2000) d.cdma2000.sid = v;
          })
        }
        error={errors.get(`${errorPrefix}.cdma2000.sid`)}
      />
      <EnumSelect
        label="NID_INCL"
        value={sysId.nidIncl}
        options={NID_INCL_OPTIONS}
        onChange={(next) =>
          onPatch((d) => {
            if (!d.cdma2000) return;
            d.cdma2000.nidIncl = next;
            if (next !== PrlNidInclusion.PRL_NID_INCLUSION_SINGLE) {
              d.cdma2000.nid = undefined;
            } else if (d.cdma2000.nid == null) {
              d.cdma2000.nid = 0xffff;
            }
          })
        }
      />
      {includesNid && (
        <NumericInput
          label="NID"
          bits={16}
          value={sysId.nid ?? 0}
          onChange={(v) =>
            onPatch((d) => {
              if (d.cdma2000) d.cdma2000.nid = v;
            })
          }
        />
      )}
      <AcqIndexPicker
        value={record.acqIndex}
        onChange={(v) => onPatch((d) => void (d.acqIndex = v))}
        mode={mode}
        acqRecords={acq.acqRecords}
        patchAcq={acq.patchAcq}
        errors={errors}
        errorPrefix={errorPrefix}
        error={errors.get(`${errorPrefix}.acqIndex`)}
      />
      <label className="block">
        <span className="text-muted text-[11px]">GEO (same region as prev)</span>
        <div className="mt-1">
          <input
            type="checkbox"
            checked={record.sameGeoAsPrev}
            onChange={(e) =>
              onPatch((d) => void (d.sameGeoAsPrev = e.target.checked))
            }
          />
        </div>
      </label>
      <EnumSelect
        label="PREF_NEG"
        value={record.prefNeg}
        options={PREF_NEG_OPTIONS}
        onChange={(v) => onPatch((d) => void (d.prefNeg = v))}
      />
      <EnumSelect
        label="PRI"
        value={record.priority}
        options={PRIORITY_OPTIONS}
        onChange={(v) => onPatch((d) => void (d.priority = v))}
      />
      <RoamingIndicatorSelect
        label="ROAM_IND"
        value={record.roamingIndicator?.raw ?? 1}
        onChange={(v) =>
          onPatch((d) => {
            if (!d.roamingIndicator) {
              d.roamingIndicator = defaultRoamingIndicator();
            }
            d.roamingIndicator.raw = v;
          })
        }
        error={errors.get(`${errorPrefix}.roamingIndicator`)}
      />
    </div>
  );
}
