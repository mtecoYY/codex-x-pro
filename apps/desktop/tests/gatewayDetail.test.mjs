import assert from "node:assert/strict";
import test from "node:test";
import { conversationText, gatewayProbeTruncation, gatewayProbeValue } from "../src/gatewayDetail.ts";

const probe = { raw_text: "POST / HTTP/1.1\r\n\r\n{}", request_body_json: { model: "m" }, response_body_json: null, raw_text_truncated: true, truncate_reason: "OBSERVE_DETAIL_TRUNCATED", original_bytes: 200, retained_bytes: 100 };

test("probe view selection is independent of probe name", () => {
  assert.equal(gatewayProbeValue(probe, "raw-text"), probe.raw_text);
  assert.deepEqual(gatewayProbeValue(probe, "request body JSON"), { model: "m" });
  assert.equal(gatewayProbeValue(probe, "response body JSON"), null);
});

test("truncation metadata is explicit and complete", () => {
  assert.equal(gatewayProbeTruncation(probe), "OBSERVE_DETAIL_TRUNCATED: original_bytes=200, retained_bytes=100");
  assert.equal(gatewayProbeTruncation({ ...probe, raw_text_truncated: false }), null);
});

test("conversation view normalizes Responses and Chat Completions messages", () => {
  const input = { input: [{ role: "user", content: [{ type: "input_text", text: "hello" }] }], output: [{ role: "assistant", content: [{ type: "output_text", text: "world" }] }] };
  assert.equal(conversationText(input), "user\nhello\n\nassistant\nworld");
  assert.deepEqual(gatewayProbeValue({ request_body_json: { messages: [{ role: "system", content: "rules" }, { role: "user", content: "question" }] } }, "conversation"), [{ role: "system", text: "rules" }, { role: "user", text: "question" }]);
});
