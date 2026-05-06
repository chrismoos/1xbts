import { getBscManagementClient, waitForBscReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const encoder = new TextEncoder();
  const abort = new AbortController();

  // Abort the gRPC stream when the client disconnects or the request is aborted.
  request.signal.addEventListener("abort", () => abort.abort());

  const stream = new ReadableStream({
    async start(controller) {
      const send = (chunk: string) => {
        if (!abort.signal.aborted) {
          try {
            controller.enqueue(encoder.encode(chunk));
          } catch {
            abort.abort();
          }
        }
      };

      send("retry: 2000\n\n");

      try {
        await waitForBscReady();
        console.log("[paging-events] starting gRPC stream");
        const client = getBscManagementClient();
        for await (const event of client.streamPagingEvents(
          {},
          { signal: abort.signal }
        )) {
          if (abort.signal.aborted) break;
          send(`data: ${JSON.stringify(event)}\n\n`);
        }
        console.log("[paging-events] gRPC stream ended");
      } catch (err) {
        if (!abort.signal.aborted) {
          const msg = err instanceof Error ? err.message : "unknown error";
          console.log(`[paging-events] gRPC error: ${msg}`);
          send(
            `event: status\ndata: ${JSON.stringify({
              state: "connecting",
              error: msg,
            })}\n\n`
          );
        } else {
          console.log("[paging-events] aborted");
        }
      }
      try {
        controller.close();
      } catch {
        // already closed
      }
    },
    cancel() {
      console.log("[paging-events] client disconnected");
      abort.abort();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}
