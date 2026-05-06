import { createClient } from "nice-grpc";
import { SmscServiceDefinition } from "../proto/smsc/v1/service";

// Reuse the same channel as BSC — all services are on the same port
import { channel } from "./client";

export function getSmscClient() {
  return createClient(SmscServiceDefinition, channel);
}
