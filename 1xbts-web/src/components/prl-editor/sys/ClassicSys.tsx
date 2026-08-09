import {
  PrlNidInclusion,
  PrlPrefNeg,
  PrlSysRecord,
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

export function ClassicSysEditor({
  record,
  onPatch,
  mode,
  acq,
  errors,
  errorPrefix,
}: {
  record: PrlSysRecord;
  onPatch: (mutator: (draft: PrlSysRecord) => void) => void;
  mode: EditorMode;
  acq: AcqPickerContext;
  errors: ErrorMap;
  errorPrefix: string;
}) {
  const includesNid =
    record.nidIncl === PrlNidInclusion.PRL_NID_INCLUSION_SINGLE;
  const isPref = record.prefNeg === PrlPrefNeg.PRL_PREF_NEG_PREFERRED;

  return (
    <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
      <NumericInput
        label="SID"
        bits={15}
        value={record.sid}
        onChange={(v) => onPatch((d) => void (d.sid = v))}
        error={errors.get(`${errorPrefix}.sid`)}
      />
      <EnumSelect
        label="NID_INCL"
        value={record.nidIncl}
        options={NID_INCL_OPTIONS}
        onChange={(next) =>
          onPatch((d) => {
            d.nidIncl = next;
            if (next !== PrlNidInclusion.PRL_NID_INCLUSION_SINGLE) {
              d.nid = undefined;
            } else if (d.nid == null) {
              d.nid = 0xffff;
            }
          })
        }
      />
      {includesNid && (
        <NumericInput
          label="NID"
          bits={16}
          value={record.nid ?? 0}
          onChange={(v) => onPatch((d) => void (d.nid = v))}
          error={errors.get(`${errorPrefix}.nid`)}
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
        {errors.get(`${errorPrefix}.sameGeoAsPrev`) && (
          <span className="text-accent-red text-[11px]">
            {errors.get(`${errorPrefix}.sameGeoAsPrev`)}
          </span>
        )}
      </label>
      <EnumSelect
        label="PREF_NEG"
        value={record.prefNeg}
        options={PREF_NEG_OPTIONS}
        onChange={(next) =>
          onPatch((d) => {
            d.prefNeg = next;
            if (next === PrlPrefNeg.PRL_PREF_NEG_NEGATIVE) {
              d.priority = undefined as unknown as number;
              d.roamingIndicator = undefined;
            }
          })
        }
      />
      {isPref && (
        <>
          <EnumSelect
            label="PRI"
            value={record.priority ?? 0}
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
        </>
      )}
    </div>
  );
}
