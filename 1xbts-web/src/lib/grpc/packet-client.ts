import { createClient } from "nice-grpc";
import { PacketServiceDefinition } from "../proto/packet/v1/service";

// Reuse the same channel as BSC — all services are on the same port
import { channel } from "./client";
export { waitForBscReady as waitForPacketReady } from "./client";

export function getPacketClient() {
  return createClient(PacketServiceDefinition, channel);
}
