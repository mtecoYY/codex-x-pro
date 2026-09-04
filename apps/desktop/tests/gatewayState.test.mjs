import assert from "node:assert/strict";
import test from "node:test";
import { gatewayCanStart, gatewayControlsDisabled, gatewayDisplayMode, gatewayRouteActive, gatewayUsesRuntime } from "../src/gatewayState.ts";

test("unknown gateway state never falls back to direct mode", () => {
  assert.equal(gatewayDisplayMode(null), "unknown");
  assert.equal(gatewayCanStart(null), false);
  assert.equal(gatewayControlsDisabled(null, false), true);
  assert.equal(gatewayRouteActive(null), false);
});

test("degraded gateway state keeps live controls disabled", () => {
  const state = { running: false, managedByCodexX: true, codexRouteActive: false, listenPort: 8787, degraded: true };
  assert.equal(gatewayDisplayMode(state), "degraded");
  assert.equal(gatewayCanStart(state), false);
  assert.equal(gatewayControlsDisabled(state, false), true);
  assert.equal(gatewayUsesRuntime(state), false);
});

test("an unmanaged running gateway is external and does not put Codex-X-Pro in gateway mode", () => {
  const state = { running: true, managedByCodexX: false, codexRouteActive: false, listenPort: 8787 };
  assert.equal(gatewayDisplayMode(state), "external");
  assert.equal(gatewayCanStart(state), true);
  assert.equal(gatewayControlsDisabled(state, false), true);
  assert.equal(gatewayUsesRuntime(state), false);
});

test("a managed running gateway keeps the stop and observation controls active", () => {
  const state = { running: true, managedByCodexX: true, codexRouteActive: true, listenPort: 8888 };
  assert.equal(gatewayDisplayMode(state), "managed");
  assert.equal(gatewayCanStart(state), false);
  assert.equal(gatewayControlsDisabled(state, false), false);
  assert.equal(gatewayUsesRuntime(state), true);
});

test("a stopped gateway can be started", () => {
  const state = { running: false, managedByCodexX: false, codexRouteActive: false, listenPort: 8888 };
  assert.equal(gatewayDisplayMode(state), "stopped");
  assert.equal(gatewayCanStart(state), true);
  assert.equal(gatewayControlsDisabled(state, false), true);
  assert.equal(gatewayUsesRuntime(state), false);
});

test("a managed gateway with an externally changed Codex route is disconnected but retained", () => {
  const state = { running: true, managedByCodexX: true, codexRouteActive: false, listenPort: 8888 };
  assert.equal(gatewayDisplayMode(state), "disconnected");
  assert.equal(gatewayCanStart(state), false);
  assert.equal(gatewayControlsDisabled(state, false), true);
  assert.equal(gatewayRouteActive(state), false);
  assert.equal(gatewayUsesRuntime(state), true);
});

test("external and stopped gateways never use the managed runtime route", () => {
  assert.equal(gatewayUsesRuntime({ running: true, managedByCodexX: false, codexRouteActive: false, listenPort: 8787 }), false);
  assert.equal(gatewayUsesRuntime({ running: false, managedByCodexX: false, codexRouteActive: false, listenPort: 8787 }), false);
});
