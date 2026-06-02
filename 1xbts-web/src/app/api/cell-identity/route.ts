import { getBtsManagementClient } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

// Returns the cell's MCC and IMSI_11_12 as digit strings so the
// subscriber editor can build a 15-digit IMSI from a phone number
// without duplicating the C.S0005-E §2.3.1 decode in JavaScript.
export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const client = getBtsManagementClient();
    const cfg = await client.getBtsConfig({}, { signal: abort.signal });
    const mcc = cfg.overhead?.mccDigits ?? "";
    const imsi1112 = cfg.overhead?.imsi1112Digits ?? "";
    return Response.json({ mcc, imsi1112 });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
