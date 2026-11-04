import { getMscManagementClient } from "@/lib/grpc/client";
import { apiError } from "@/lib/api-error";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const client = getMscManagementClient();
    const result = await client.listBaseStations({}, { signal: abort.signal });
    return Response.json(result);
  } catch (err) {
    return apiError(err, "load base stations");
  } finally {
    clearTimeout(timeout);
  }
}
