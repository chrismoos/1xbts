export function smsStateColor(state: string): string {
  switch (state) {
    case "delivered":
    case "sent":
      return "bg-badge-green-bg text-badge-green-text";
    case "paging":
    case "page_response_received":
      return "bg-badge-yellow-bg text-badge-yellow-text";
    case "failed":
    case "expired":
      return "bg-badge-red-bg text-badge-red-text";
    default:
      return "bg-badge-blue-bg text-badge-blue-text";
  }
}

const IN_FLIGHT_STATES = new Set(["accepted", "paging", "page_response_received"]);

export function smsInFlight(state: string): boolean {
  return IN_FLIGHT_STATES.has(state);
}
