import { cellRequestFromUrl, selectorFromUrl } from "@/lib/cell";
import { getNetworkManagementClient, waitForManagementReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

interface PowerOverrideBody {
  walshCode?: number;
  targetDb?: number;
  clear?: boolean;
}

export async function POST(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const selector = selectorFromUrl(request.url);
    const { cell } = cellRequestFromUrl(request.url);
    const body = (await request.json()) as PowerOverrideBody;
    const walshCode = Number(body.walshCode);
    if (!Number.isInteger(walshCode) || walshCode < 0) {
      return Response.json(
        { accepted: false, message: "invalid walshCode" },
        { status: 400 },
      );
    }

    const clear = body.clear === true;
    const targetDb = Number(body.targetDb);
    if (!clear && !Number.isFinite(targetDb)) {
      return Response.json(
        { accepted: false, message: "invalid targetDb" },
        { status: 400 },
      );
    }

    await waitForManagementReady();
    const client = getNetworkManagementClient();
    const result = await client.setReversePowerControlOverride(
      {
        selector,
        request: clear
          ? { walshCode, clear: true, cell }
          : { walshCode, setTargetEbNtDb: targetDb, cell },
      },
      { signal: abort.signal },
    );

    return Response.json(result, { status: result.accepted ? 200 : 400 });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ accepted: false, message: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
