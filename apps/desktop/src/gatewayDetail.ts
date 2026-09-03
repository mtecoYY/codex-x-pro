export type GatewayDetailView = "raw-text" | "request body JSON" | "response body JSON";

export type GatewayProbe = {
  raw_text?: unknown;
  request_body_json?: unknown;
  response_body_json?: unknown;
  raw_text_truncated?: unknown;
  truncate_reason?: unknown;
  original_bytes?: unknown;
  retained_bytes?: unknown;
};

export function gatewayProbeValue(probe: GatewayProbe | undefined, view: GatewayDetailView): unknown {
  if (!probe) return null;
  if (view === "raw-text") return probe.raw_text ?? null;
  if (view === "request body JSON") return probe.request_body_json ?? null;
  return probe.response_body_json ?? null;
}

export function gatewayProbeTruncation(probe: GatewayProbe | undefined): string | null {
  if (!probe || !probe.raw_text_truncated) return null;
  const reason = typeof probe.truncate_reason === "string" ? probe.truncate_reason : "OBSERVE_DETAIL_TRUNCATED";
  const original = typeof probe.original_bytes === "number" ? probe.original_bytes : "?";
  const retained = typeof probe.retained_bytes === "number" ? probe.retained_bytes : "?";
  return `${reason}: original_bytes=${original}, retained_bytes=${retained}`;
}
