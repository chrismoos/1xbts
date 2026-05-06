import { createClient } from "nice-grpc";
import { HlrServiceDefinition } from "../proto/hlr/v1/service";

// Reuse the same channel as BSC — all services are on the same port
import { channel } from "./client";
export { waitForBscReady as waitForHlrReady } from "./client";

export function getHlrClient() {
  return createClient(HlrServiceDefinition, channel);
}
