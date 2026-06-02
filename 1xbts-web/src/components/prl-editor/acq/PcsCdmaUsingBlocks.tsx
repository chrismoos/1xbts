import {
  PrlAcqRecord,
  PrlExtAcqRecord,
  PrlPcsBlock,
} from "@/lib/proto/hlr/v1/service";
import { PCS_BLOCK_OPTIONS } from "@/lib/prl-options";

export function PcsCdmaUsingBlocksEditor({
  record,
  onPatch,
  error,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
  error?: string;
}) {
  const blocks = record.pcsCdmaUsingBlocks?.blocks ?? [];

  const add = () =>
    onPatch((d) => {
      if (d.pcsCdmaUsingBlocks)
        d.pcsCdmaUsingBlocks.blocks.push(PrlPcsBlock.PRL_PCS_BLOCK_A);
    });

  const removeAt = (i: number) =>
    onPatch((d) => {
      if (d.pcsCdmaUsingBlocks)
        d.pcsCdmaUsingBlocks.blocks.splice(i, 1);
    });

  const updateAt = (i: number, value: PrlPcsBlock) =>
    onPatch((d) => {
      if (d.pcsCdmaUsingBlocks) d.pcsCdmaUsingBlocks.blocks[i] = value;
    });

  return (
    <div className="space-y-1">
      <div className="text-muted text-[11px]">
        PCS frequency blocks{" "}
        <span className="text-dimmed">
          ({blocks.length} blocks, 3-bit count max 7)
        </span>
      </div>
      {blocks.map((b, i) => (
        <div key={i} className="flex gap-1 items-center">
          <span className="font-mono text-dimmed text-[11px] w-4">{i}</span>
          <select
            className="flex-1 bg-bg border border-border rounded px-2 py-0.5 text-xs"
            value={b}
            onChange={(e) => updateAt(i, Number(e.target.value) as PrlPcsBlock)}
          >
            {PCS_BLOCK_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <button
            onClick={() => removeAt(i)}
            className="text-accent-red text-[11px] px-1 hover:underline"
          >
            ✕
          </button>
        </div>
      ))}
      <button
        onClick={add}
        disabled={blocks.length >= 7}
        className="text-accent-blue text-[11px] hover:underline disabled:opacity-50"
      >
        + Add block
      </button>
      {error && <p className="text-accent-red text-[11px]">{error}</p>}
    </div>
  );
}
