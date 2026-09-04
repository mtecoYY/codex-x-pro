import type { GatewayObserveRow } from "./types";

export type GatewayDetail = Record<string, unknown>;

export type GatewayObserveSortKey =
  | "id"
  | "channel"
  | "status_code"
  | "model"
  | "request_time_ms"
  | "first_token_ms"
  | "tokens";

export type GatewayScriptSummary = {
  id: string;
  name: string;
  description?: string;
  status: "not_tested" | "testing" | "passed" | "failed";
  enabled: boolean;
  priority: number;
  average_ms?: number | null;
  sample_count?: number;
  test_detail_available?: boolean;
  error?: string | null;
};

export const DEFAULT_SCRIPT_TEST_RAW_TEXT =
  'POST /v1/responses HTTP/1.1\r\ncontent-type: application/json\r\nx-codex-script-test: 1\r\n\r\n{"model":"codex-x-test-model","input":"gateway script test"}';

export type GatewayObserveListResponse = {
  requests?: GatewayObserveRow[];
  history_gap?: boolean;
  next_seq?: number;
};

export type GatewayScriptListResponse = {
  scripts?: GatewayScriptSummary[];
};
