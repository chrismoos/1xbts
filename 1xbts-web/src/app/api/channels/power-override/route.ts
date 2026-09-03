import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";
import { parseCellToken } from "@/lib/cell";

export const dynamic = "force-dynamic";

interface PowerOverrideBody {
  walshCode?: number;
  targetDb?: number;
  clear?: boolean;
  cell?: string;
  baseStation?: string;
}

export async function POST(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const body = (await request.json()) as PowerOverrideBody;
    const walshCode = Number(body.walshCode);
    if (!Number.isInteger(walshCode) || walshCode < 0) {
      return Response.json(
        { accepted: false, message: "invalid walshCode" },
        { status: 400 }
      );
    }

    const clear = body.clear === true;
    const targetDb = Number(body.targetDb);
    if (!clear && !Number.isFinite(targetDb)) {
      return Response.json(
        { accepted: false, message: "invalid targetDb" },
        { status: 400 }
      );
    }

    await waitForManagementReady();
    const client = getNetworkManagementClient();
    // Walsh codes are allocated per cell, so the code alone does not name a
    // channel once more than one cell is enrolled.
    const cell = parseCellToken(body.cell);
    const result = await client.setTrafficChannelPowerOverride(
      {
        selector: { baseStation: body.baseStation ?? "" },
        request: clear
          ? { walshCode, cell, clear: true }
          : { walshCode, cell, setTargetEbNtDb: targetDb },
      },
      { signal: abort.signal }
    );

    return Response.json(result, { status: result.accepted ? 200 : 400 });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ accepted: false, message: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
