import http from "node:http";
import fs from "node:fs/promises";
import process from "node:process";

const base = process.env.WEBDRIVER_URL || "http://127.0.0.1:4444";
const port = Number(process.env.CODEX_X_TEST_PORT || 18787);
const mockPort = Number(process.env.CODEX_X_MOCK_PORT || 19090);
const timeout = Number(process.env.E2E_TIMEOUT_MS || 30000);

function request(method, path, body) {
  return new Promise((resolve, reject) => {
    const url = new URL(path, base);
    const data = body === undefined ? undefined : JSON.stringify(body);
    const req = http.request(url, { method, headers: data ? { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(data) } : {} }, (res) => {
      let text = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => { text += chunk; });
      res.on("end", () => {
        let value = text;
        try { value = text ? JSON.parse(text) : null; } catch {}
        if (res.statusCode >= 400) reject(new Error(`${method} ${path}: ${res.statusCode} ${text}`));
        else resolve(value);
      });
    });
    req.on("error", reject);
    if (data) req.write(data);
    req.end();
  });
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function waitFor(check, label) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try { if (await check()) return; } catch {}
    await sleep(250);
  }
  throw new Error(`Timed out: ${label}`);
}

let session;
async function openSession() {
  const created = await request("POST", "/session", { capabilities: { alwaysMatch: { "tauri:options": { application: process.env.CODEX_X_EXE } } } });
  session = created.value?.sessionId || created.sessionId;
  if (!session) throw new Error("WebDriver did not return a session id");
  try {
    await waitFor(() => state(".cx-app-shell"), "Codex-X-Pro shell");
  } catch (error) {
    const diagnostics = await script("return { title: document.title, url: location.href, body: document.body?.innerText?.slice(0, 500) || '', html: document.documentElement?.outerHTML?.slice(0, 500) || '' }").catch((nextError) => ({ value: null, error: String(nextError) }));
    const source = await request("GET", `/session/${session}/source`).catch((nextError) => ({ value: null, error: String(nextError) }));
    const title = await request("GET", `/session/${session}/title`).catch((nextError) => ({ value: null, error: String(nextError) }));
    console.error(`SHELL_DIAGNOSTICS=${JSON.stringify({ diagnostics, source, title })}`);
    throw error;
  }
  await script(
    "localStorage.setItem('codexx.gateway.port', arguments[0]); window.dispatchEvent(new CustomEvent('codexx-gateway-port-changed', { detail: Number(arguments[0]) })); return localStorage.getItem('codexx.gateway.port')",
    [String(port)],
  );
}
async function closeSession() {
  if (!session) return;
  try { await request("DELETE", `/session/${session}`); } catch {}
  session = undefined;
}
async function gatewayHealthy() {
  try { return (await fetch(`http://127.0.0.1:${port}/health`)).ok; } catch { return false; }
}
async function script(command, args = []) {
  return request("POST", `/session/${session}/execute/sync`, { script: command, args });
}
async function find(selector) {
  const result = await request("POST", `/session/${session}/element`, { using: "css selector", value: selector });
  return result.value;
}
async function click(selector) {
  const element = await find(selector);
  await request("POST", `/session/${session}/element/${element["element-6066-11e4-a52e-4f735466cecf"] || element["ELEMENT"]}/click`, {});
}
async function fill(selector, text) {
  const element = await find(selector);
  const id = element["element-6066-11e4-a52e-4f735466cecf"] || element["ELEMENT"];
  await request("POST", `/session/${session}/element/${id}/clear`, {});
  await request("POST", `/session/${session}/element/${id}/value`, { text: String(text), value: [...String(text)] });
}
async function clickText(text) {
  return script("return [...document.querySelectorAll('button')].find((el) => el.textContent.includes(arguments[0]))?.click()", [text]);
}
async function value(selector) {
  const element = await find(selector);
  const id = element["element-6066-11e4-a52e-4f735466cecf"] || element["ELEMENT"];
  return request("GET", `/session/${session}/element/${id}/property/value`);
}
async function state(selector) {
  return Boolean((await script("return document.querySelector(arguments[0])?.textContent || ''", [selector]))?.value);
}
async function assertPort(expected, label) {
  await waitFor(async () => (await script("return Boolean(document.querySelector('[data-testid=\\\"gateway-port\\\"]'))")).value, label);
  const actual = await value("[data-testid='gateway-port']");
  if (String(actual.value ?? actual) !== String(expected)) throw new Error(`${label}: expected port ${expected}`);
}
async function isolatedConfigText() {
  const configPath = process.env.CODEX_HOME && `${process.env.CODEX_HOME}/config.toml`;
  return configPath ? fs.readFile(configPath, "utf8") : "";
}
function assertConfigRoute(text, expectedPort, label) {
  const route = `127.0.0.1:${expectedPort}/v1`;
  if (!String(text).includes(route)) throw new Error(`${label}: expected config route ${route}`);
}
function assertConfigNotRoute(text, forbiddenPort, label) {
  const route = `127.0.0.1:${forbiddenPort}/v1`;
  const source = String(text);
  const providerId = source.match(/^\s*model_provider\s*=\s*["']([^"']+)["']/m)?.[1] || "";
  const topLevel = source.split(/^\s*\[/m, 1)[0];
  const providerSection = providerId
    ? source.match(new RegExp(`\\[model_providers\\.${providerId.replace(/[.*+?^${}()|[\\]\\\\]/g, "\\\\$&")}\\]([\\s\\S]*?)(?=\\n\\s*\\[|$)`))?.[1] || ""
    : "";
  if (topLevel.includes(route) || providerSection.includes(route)) throw new Error(`${label}: active gateway route still present: ${route}`);
}
function configHasActiveRoute(text, portNumber) {
  const source = String(text);
  const route = `127.0.0.1:${portNumber}/v1`;
  const providerId = source.match(/^\s*model_provider\s*=\s*["']([^"']+)["']/m)?.[1] || "";
  const topLevel = source.split(/^\s*\[/m, 1)[0];
  const escapedId = providerId.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const providerSection = providerId
    ? source.match(new RegExp(`\\[model_providers\\.${escapedId}\\]([\\s\\S]*?)(?=\\n\\s*\\[|$)`))?.[1] || ""
    : "";
  return topLevel.includes(route) || providerSection.includes(route);
}

try {
  await openSession();

  await click("[data-testid='startup-enter']").catch(() => clickText("Enter Codex-X-Pro").catch(() => undefined));
  await click("[data-testid='nav-gateway']");
  await waitFor(async () => (await script("return Boolean(document.querySelector('[data-testid=\\\"gateway-port\\\"]'))")).value, "gateway port field");
  const initialPort = await value("[data-testid='gateway-port']");
  if (String(initialPort.value ?? initialPort) !== String(port)) await fill("[data-testid='gateway-port']", port);
  await assertPort(port, "gateway page");
  await waitFor(async () => (await script("return Boolean(document.querySelector('[data-testid=\\\"gateway-start\\\"], [data-testid=\\\"gateway-stop\\\"], [data-testid=\\\"gateway-recover\\\"], [data-testid=\\\"gateway-refresh\\\"]'))")).value, "gateway controls");
  const refreshControl = await script("return Boolean(document.querySelector('[data-testid=\\\"gateway-refresh\\\"]')) && !document.querySelector('[data-testid=\\\"gateway-start\\\"], [data-testid=\\\"gateway-stop\\\"], [data-testid=\\\"gateway-recover\\\"]')");
  if (refreshControl.value) {
    await click("[data-testid='gateway-refresh']");
    await waitFor(async () => (await script("return Boolean(document.querySelector('[data-testid=\\\"gateway-start\\\"], [data-testid=\\\"gateway-stop\\\"], [data-testid=\\\"gateway-recover\\\"]'))")).value, "gateway controls after refresh");
  }
  await click("[data-testid='gateway-start']");
  await waitFor(() => state("[data-testid='gateway-stop']"), "gateway started");
  await waitFor(gatewayHealthy, "gateway health after start");
  console.log("STEP1_GATEWAY_MODE=True");
  console.log(`STEP1_GATEWAY_PORT=${port}`);

  await closeSession();
  await waitFor(gatewayHealthy, "gateway after Codex-X-Pro exit");
  console.log("STEP2_CODEX_RESTART=ORCHESTRATOR_CHECKED");
  console.log("STEP3_APP_EXIT_GATEWAY=True");
  await openSession();
  await click("[data-testid='startup-enter']").catch(() => clickText("Enter Codex-X-Pro").catch(() => undefined));
  await click("[data-testid='nav-provider']");
  await waitFor(() => state("[data-testid^='provider-row-']"), "provider list");
  const visibleProviderUrls = await script("return [...document.querySelectorAll('[data-testid^=\\\"provider-row-\\\"] code')].map((el) => el.textContent || '')");
  if (visibleProviderUrls.value?.some((url) => String(url).includes(`127.0.0.1:${port}`))) {
    throw new Error(`Gateway listen address leaked into provider list: ${visibleProviderUrls.value.join(', ')}`);
  }
  const rows = await script("return [...document.querySelectorAll('[data-testid^=\\\"provider-row-\\\"]')].map((el) => el.dataset.providerId)");
  if (!rows.value?.length) throw new Error("No provider rows available for switch test");
  console.log(`STEP4_PROVIDER_ROWS=${rows.value.length}`);
  let switchButton = await script("return [...document.querySelectorAll('[data-testid^=\\\"provider-switch-\\\"]')].find((el) => !el.disabled)?.getAttribute('data-testid') || ''");
  if (!switchButton.value) {
    await click("[data-testid='provider-add']");
    await waitFor(() => state("[data-testid='provider-save']"), "provider add form");
    await fill("[data-testid='provider-name']", "Isolated Switch Provider");
    await fill("[data-testid='provider-base-url']", `http://127.0.0.1:${mockPort}/synthetic-provider`);
    await fill("[data-testid='provider-model']", "isolated-switch-model");
    await fill("[data-testid='provider-api-key']", "isolated-provider-key");
    await click("[data-testid='provider-save']");
    await waitFor(() => state("[data-testid^='provider-row-']"), "provider list after add");
    switchButton = await script("return [...document.querySelectorAll('[data-testid^=\\\"provider-switch-\\\"]')].find((el) => !el.disabled)?.getAttribute('data-testid') || ''");
  }
  if (switchButton.value) {
    await click(`[data-testid='${switchButton.value}']`);
    await sleep(500);
    console.log("STEP4_PROVIDER_SWITCH=True");
    const switchedProviderUrls = await script("return [...document.querySelectorAll('[data-testid^=\\\"provider-row-\\\"] code')].map((el) => el.textContent || '')");
    if (switchedProviderUrls.value?.some((url) => String(url).includes(`127.0.0.1:${port}`))) {
      throw new Error(`Gateway listen address leaked after provider switch: ${switchedProviderUrls.value.join(', ')}`);
    }
  } else {
    throw new Error("No enabled provider switch button available");
  }
  await click("[data-testid='nav-gateway']");
  await waitFor(() => state("[data-testid='gateway-stop']"), "gateway after provider switch");
  console.log("STEP4_GATEWAY_STILL_RUNNING=True");
  await closeSession();
  await waitFor(gatewayHealthy, "gateway after second Codex-X-Pro exit");
  const configPath = process.env.CODEX_HOME && `${process.env.CODEX_HOME}/config.toml`;
  if (configPath) await fs.appendFile(configPath, "\n# isolated reload probe\n");
  await openSession();
  console.log("STEP5_CONFIG_RELOAD=True");
  const reloadedConfig = await isolatedConfigText();
  assertConfigRoute(reloadedConfig, port, "config after reload");
  console.log("STEP6_CONFIG_REOPEN=True");

  // Step 7: after editing the isolated config, switch provider again.
  await click("[data-testid='startup-enter']").catch(() => clickText("Enter Codex-X-Pro").catch(() => undefined));
  await click("[data-testid='nav-provider']");
  await waitFor(() => state("[data-testid^='provider-row-']"), "provider list after config reload");
  let secondSwitch = await script("return [...document.querySelectorAll('[data-testid^=\"provider-switch-\"]')].find((el) => !el.disabled)?.getAttribute('data-testid') || ''");
  if (!secondSwitch.value) throw new Error("No enabled provider switch button after config reload");
  await click(`[data-testid='${secondSwitch.value}']`);
  await sleep(500);
  console.log("STEP7_PROVIDER_SWITCH_AFTER_RELOAD=True");

  // Step 8: close the gateway and verify the isolated config returns to direct mode.
  await click("[data-testid='nav-gateway']");
  await waitFor(() => state("[data-testid='gateway-stop']"), "gateway stop control before shutdown");
  await click("[data-testid='gateway-stop']");
  await waitFor(async () => !(await gatewayHealthy()), "gateway stopped");
  await waitFor(async () => {
    const text = await isolatedConfigText();
    return !configHasActiveRoute(text, port);
  }, "direct config restored after gateway stop");
  const directConfig = await isolatedConfigText();
  assertConfigNotRoute(directConfig, port, "config after gateway stop");
  console.log("STEP8_GATEWAY_STOP=True");
  console.log("STEP8_CONFIG_DIRECT=True");

  // Step 9: start the gateway again and verify the isolated config is projected back.
  await waitFor(() => state("[data-testid='gateway-start']"), "gateway start control after shutdown");
  await click("[data-testid='gateway-start']");
  await waitFor(() => state("[data-testid='gateway-stop']"), "gateway restarted");
  await waitFor(gatewayHealthy, "gateway health after restart");
  const restartedConfig = await isolatedConfigText();
  assertConfigRoute(restartedConfig, port, "config after gateway restart");
  console.log("STEP9_GATEWAY_RESTART=True");
  console.log("STEP9_CONFIG_GATEWAY=True");

  // Step 10: close Codex-X-Pro while the gateway remains enabled.
  await closeSession();
  await waitFor(gatewayHealthy, "gateway after second Codex-X-Pro exit");
  console.log("STEP10_CODEX_X_EXIT_GATEWAY_RUNNING=True");

  // Step 11: edit the isolated original config and reopen Codex-X-Pro.
  const originalEditMarker = "# isolated original config edit step 11";
  const beforeOriginalEdit = await isolatedConfigText();
  if (!beforeOriginalEdit.includes(originalEditMarker)) {
    const configPath = process.env.CODEX_HOME && `${process.env.CODEX_HOME}/config.toml`;
    if (!configPath) throw new Error("CODEX_HOME is unavailable for original config edit");
    await fs.appendFile(configPath, `\n${originalEditMarker}\n`);
  }
  const editedBeforeReopen = await isolatedConfigText();
  if (!editedBeforeReopen.includes(originalEditMarker)) throw new Error("Original isolated config edit was not written");
  await openSession();
  await click("[data-testid='startup-enter']").catch(() => clickText("Enter Codex-X-Pro").catch(() => undefined));
  await click("[data-testid='nav-gateway']");
  await assertPort(port, "gateway after original config edit");
  await waitFor(gatewayHealthy, "gateway after original config edit reopen");
  const editedConfig = await isolatedConfigText();
  if (!editedConfig.includes(originalEditMarker)) throw new Error("Original isolated config edit was lost");
  assertConfigRoute(editedConfig, port, "config after original edit reopen");
  console.log("STEP11_ORIGINAL_CONFIG_EDIT_REOPEN=True");

  // Step 12: switch provider again after the original config edit.
  await click("[data-testid='nav-provider']");
  await waitFor(() => state("[data-testid^='provider-row-']"), "provider list after original config edit");
  const thirdSwitch = await script("return [...document.querySelectorAll('[data-testid^=\"provider-switch-\"]')].find((el) => !el.disabled)?.getAttribute('data-testid') || ''");
  if (!thirdSwitch.value) throw new Error("No enabled provider switch button after original config edit");
  await click(`[data-testid='${thirdSwitch.value}']`);
  await sleep(500);
  console.log("STEP12_PROVIDER_SWITCH_AFTER_ORIGINAL_EDIT=True");

  // Step 13: stop the gateway and verify the edited config is restored to direct mode.
  await click("[data-testid='nav-gateway']");
  await waitFor(() => state("[data-testid='gateway-stop']"), "gateway stop control before step 13");
  await click("[data-testid='gateway-stop']");
  await waitFor(async () => !(await gatewayHealthy()), "gateway stopped at step 13");
  await waitFor(async () => !configHasActiveRoute(await isolatedConfigText(), port), "direct config after step 13 stop");
  const step13Config = await isolatedConfigText();
  assertConfigNotRoute(step13Config, port, "config after step 13 gateway stop");
  if (!step13Config.includes(originalEditMarker)) throw new Error("Original config edit was not preserved after step 13 stop");
  console.log("STEP13_GATEWAY_STOP=True");
  console.log("STEP13_CONFIG_DIRECT=True");

  // Step 14: start the gateway and verify the edited config is projected again.
  await waitFor(() => state("[data-testid='gateway-start']"), "gateway start control before step 14");
  await click("[data-testid='gateway-start']");
  await waitFor(() => state("[data-testid='gateway-stop']"), "gateway restarted at step 14");
  await waitFor(gatewayHealthy, "gateway health at step 14");
  const step14Config = await isolatedConfigText();
  assertConfigRoute(step14Config, port, "config after step 14 gateway restart");
  if (!step14Config.includes(originalEditMarker)) throw new Error("Original config edit was lost after step 14 restart");
  console.log("STEP14_GATEWAY_RESTART=True");
  console.log("STEP14_CONFIG_GATEWAY=True");
  console.log("E2E_UI_PASS=True");
} finally {
  await closeSession();
}
