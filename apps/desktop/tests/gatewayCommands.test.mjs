import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createGatewayCommands } from "../src/gatewayCommands.ts";

function createRecorder() {
  const calls = [];
  const invoke = async (command, args) => {
    calls.push({ command, args });
    return { command, args };
  };
  return { calls, commands: createGatewayCommands(invoke) };
}

test("gateway_request uses the Rust command's required input wrapper", async () => {
  const { calls, commands } = createRecorder();
  const body = { model: "test-model", input: "synthetic request" };

  await commands.request({
    listenPort: 8788,
    method: "PUT",
    path: "/state/provider",
    body,
  });

  assert.deepEqual(calls, [{
    command: "gateway_request",
    args: {
      input: {
        listenPort: 8788,
        method: "PUT",
        path: "/state/provider",
        body,
      },
    },
  }]);
});

test("gateway commands preserve null bodies and do not double-wrap start input", async () => {
  const { calls, commands } = createRecorder();

  await commands.getProcessState(8788);
  await commands.request({
    listenPort: 8788,
    method: "GET",
    path: "/observe/state",
    body: null,
  });
  await commands.start({
    listenHost: "127.0.0.1",
    listenPort: 8788,
    upstream: "http://127.0.0.1:19090",
    configDir: null,
  });
  await commands.recover();
  await commands.stop();

  assert.deepEqual(calls, [
    {
      command: "get_gateway_process_state",
      args: { listenPort: 8788 },
    },
    {
      command: "gateway_request",
      args: {
        input: {
          listenPort: 8788,
          method: "GET",
          path: "/observe/state",
          body: null,
        },
      },
    },
    {
      command: "start_gateway",
      args: {
        input: {
          listenHost: "127.0.0.1",
          listenPort: 8788,
          upstream: "http://127.0.0.1:19090",
          configDir: null,
        },
      },
    },
    { command: "recover_gateway", args: undefined },
    { command: "stop_gateway", args: undefined },
  ]);
});

async function sourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await sourceFiles(path));
    } else if (/\.(ts|tsx)$/.test(entry.name)) {
      files.push(path);
    }
  }
  return files;
}

test("all frontend source files keep gateway IPC behind the typed adapter", async () => {
  const sourceRoot = fileURLToPath(new URL("../src/", import.meta.url));
  const files = (await sourceFiles(sourceRoot))
    .filter((file) => !file.endsWith(`${join("src", "gatewayCommands.ts")}`));

  for (const file of files) {
    const source = await readFile(file, "utf8");
    assert.doesNotMatch(
      source,
      /invoke(?:<[^>]+>)?\(\s*["'](?:get_gateway_process_state|start_gateway|recover_gateway|stop_gateway|gateway_request)["']/,
      `${file} contains a raw gateway command invoke`,
    );
  }
});

test("gateway request adapter covers every supported control method", async () => {
  const { calls, commands } = createRecorder();
  for (const method of ["GET", "POST", "PUT"]) {
    await commands.request({
      listenPort: 18787,
      method,
      path: "/test",
      body: null,
    });
  }

  assert.deepEqual(
    calls.map(({ command, args }) => [command, args.input.method]),
    [
      ["gateway_request", "GET"],
      ["gateway_request", "POST"],
      ["gateway_request", "PUT"],
    ],
  );
});
