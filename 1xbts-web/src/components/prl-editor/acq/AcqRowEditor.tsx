// Dispatcher: renders the per-type editor for a given acquisition record.
// Handles both classic (PrlAcqRecord) and extended (PrlExtAcqRecord)
// since most of the body fields are shared.

import {
  PrlAcqRecord,
  PrlExtAcqRecord,
} from "@/lib/proto/hlr/v1/service";
import { EditorMode } from "../state";
import { ErrorMap } from "../validation";

import { CellularAnalogEditor } from "./CellularAnalog";
import { CellularCdmaStandardEditor } from "./CellularCdmaStandard";
import { CellularCdmaCustomEditor } from "./CellularCdmaCustom";
import { CellularCdmaPreferredEditor } from "./CellularCdmaPreferred";
import { PcsCdmaUsingBlocksEditor } from "./PcsCdmaUsingBlocks";
import { PcsCdmaUsingChannelsEditor } from "./PcsCdmaUsingChannels";
import { JtacsCdmaStandardEditor } from "./JtacsCdmaStandard";
import { JtacsCdmaCustomEditor } from "./JtacsCdmaCustom";
import { BandClass6UsingChannelsEditor } from "./BandClass6UsingChannels";
import { Generic1xIs95Editor } from "./Generic1xIs95";
import { GenericHrpdEditor } from "./GenericHrpd";
import { UmbCommonTableEditor } from "./UmbCommonTable";
import { GenericUmbEditor } from "./GenericUmb";

export function AcqRowEditor({
  mode: _mode,
  record,
  onPatch,
  errors,
  errorPrefix,
}: {
  mode: EditorMode;
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
  errors: ErrorMap;
  errorPrefix: string;
}) {
  if (record.cellularAnalog)
    return <CellularAnalogEditor record={record} onPatch={onPatch} />;
  if (record.cellularCdmaStandard)
    return <CellularCdmaStandardEditor record={record} onPatch={onPatch} />;
  if (record.cellularCdmaCustom)
    return (
      <CellularCdmaCustomEditor
        record={record}
        onPatch={onPatch}
        error={errors.get(`${errorPrefix}.channels`)}
      />
    );
  if (record.cellularCdmaPreferred)
    return <CellularCdmaPreferredEditor record={record} onPatch={onPatch} />;
  if (record.pcsCdmaUsingBlocks)
    return (
      <PcsCdmaUsingBlocksEditor
        record={record}
        onPatch={onPatch}
        error={errors.get(`${errorPrefix}.blocks`)}
      />
    );
  if (record.pcsCdmaUsingChannels)
    return (
      <PcsCdmaUsingChannelsEditor
        record={record}
        onPatch={onPatch}
        error={errors.get(`${errorPrefix}.channels`)}
      />
    );
  if (record.jtacsCdmaStandard)
    return <JtacsCdmaStandardEditor record={record} onPatch={onPatch} />;
  if (record.jtacsCdmaCustom)
    return (
      <JtacsCdmaCustomEditor
        record={record}
        onPatch={onPatch}
        error={errors.get(`${errorPrefix}.channels`)}
      />
    );
  if (record.bandClass6UsingChannels)
    return (
      <BandClass6UsingChannelsEditor
        record={record}
        onPatch={onPatch}
        error={errors.get(`${errorPrefix}.channels`)}
      />
    );

  // Extended-only variants live on PrlExtAcqRecord.
  if ("generic1xIs95" in record && record.generic1xIs95)
    return <Generic1xIs95Editor record={record} onPatch={onPatch} />;
  if ("genericHrpd" in record && record.genericHrpd)
    return <GenericHrpdEditor record={record} onPatch={onPatch} />;
  if ("umbCommonTable" in record && record.umbCommonTable)
    return <UmbCommonTableEditor record={record} onPatch={onPatch} />;
  if ("genericUmb" in record && record.genericUmb)
    return <GenericUmbEditor record={record} onPatch={onPatch} />;

  return (
    <p className="text-dimmed text-xs">
      Unrecognised acquisition record (raw ACQ_TYPE=0x
      {record.acqTypeRaw.toString(16).padStart(2, "0")}). Save will reject.
    </p>
  );
}
