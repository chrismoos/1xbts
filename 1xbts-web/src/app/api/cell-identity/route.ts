import { cellScopedRequestFromUrl } from "@/lib/cell";
import { getNetworkManagementClient } from "@/lib/grpc/client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

// Returns the cell's MCC and IMSI_11_12 as digit strings so the
// subscriber editor can build a 15-digit IMSI from a phone number
// without duplicating the C.S0005-E §2.3.1 decode in JavaScript.
export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const client = getNetworkManagementClient();
    const cfg = await client.getBtsConfig(
      cellScopedRequestFromUrl(request.url),
      { signal: abort.signal },
    );
    const mcc = cfg.overhead?.mccDigits ?? "";
    const imsi1112 = cfg.overhead?.imsi1112Digits ?? "";
    return Response.json({ mcc, imsi1112 });
  } catch (err) {
    return apiError(err, "load the cell identity");
  } finally {
    clearTimeout(timeout);
  }
}
