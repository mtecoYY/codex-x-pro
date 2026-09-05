import type { Lang } from "./types";
import { gatewayText } from "./gatewayPageState";

function heading(level: number, title: string) {
  return `${"#".repeat(level)} ${title}`;
}

function fence(language: string, content: string) {
  return [`\`\`\`${language}`, content, "\`\`\`"].join("\n");
}

export function buildGatewayScriptProtocolDocument(lang: Lang) {
  const t = (zh: string, en: string) => gatewayText(lang, zh, en);
  const name = t("用户脚本 raw-text 协议", "User script raw-text protocol");
  const intro = t(
    "这份文档定义了网关与用户脚本处理器之间的 raw-text 协议。脚本拿到的是一整段 HTTP 报文文本，不是 JSON，不是 SDK 事件，也不是表单字段。",
    "This document defines the raw-text protocol between the gateway and user script processors. The script receives a complete HTTP message as raw text, not JSON, not SDK events, and not form fields.",
  );
  const isThisInterface = t(
    "用户脚本会在网关模式下接收每一条真实请求。脚本从 stdin 读取完整 raw-text，再通过 stdout 返回新的请求、直接响应，或者显式丢弃该请求。",
    "In gateway mode, user scripts receive each live request. The script reads complete raw text from stdin, then uses stdout to return a rewritten request, a direct response, or an explicit drop.",
  );
  const layout = t(
    "脚本目录必须放在 `~/.codex-x/gateway-tools/<script-id>/`，每个目录至少包含 `manifest.json` 和一个入口文件。",
    "Each script must live under `~/.codex-x/gateway-tools/<script-id>/` and contain at least `manifest.json` plus an entry file.",
  );
  const inputTitle = t("输入是什么", "What the script receives");
  const outputTitle = t("输出是什么", "What the script must emit");
  const authoringTitle = t("脚本如何编写", "How to write the script");
  const templateTitle = t("脚本模板示例", "Template example");
  const samplesTitle = t("输入/输出样本", "Input/output samples");
  const manifestTitle = t("manifest.json 示例", "manifest.json example");
  const envTitle = t("可用环境变量", "Available environment variables");
  const notesTitle = t("约束说明", "Rules and constraints");

  const inputExample = fence(
    "http",
    t(
      "POST /v1/responses HTTP/1.1\r\nHost: api.example.test\r\nContent-Type: application/json\r\nAuthorization: Bearer [REDACTED]\r\n\r\n{\"model\":\"gpt-4.1\",\"input\":\"hello\"}",
      "POST /v1/responses HTTP/1.1\r\nHost: api.example.test\r\nContent-Type: application/json\r\nAuthorization: Bearer [REDACTED]\r\n\r\n{\"model\":\"gpt-4.1\",\"input\":\"hello\"}",
    ),
  );

  const forwardOutput = fence(
    "http",
    t(
      "POST /v1/responses HTTP/1.1\r\nHost: upstream.example.test\r\nContent-Type: application/json\r\nContent-Length: 36\r\n\r\n{\"model\":\"gpt-4.1\",\"input\":\"hello\"}",
      "POST /v1/responses HTTP/1.1\r\nHost: upstream.example.test\r\nContent-Type: application/json\r\nContent-Length: 36\r\n\r\n{\"model\":\"gpt-4.1\",\"input\":\"hello\"}",
    ),
  );

  const directResponseOutput = fence(
    "http",
    t(
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 31\r\n\r\n{\"ok\":true,\"mode\":\"direct\"}",
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 31\r\n\r\n{\"ok\":true,\"mode\":\"direct\"}",
    ),
  );

  const template = fence(
    "python",
    [
      "#!/usr/bin/env python3",
      "import os",
      "import sys",
      "",
      "def split_raw(raw: bytes):",
      "    header, body = raw.split(b\"\\r\\n\\r\\n\", 1)",
      "    return header, body",
      "",
      "raw = sys.stdin.buffer.read()",
      "mode = os.environ.get(\"CODEX_X_SCRIPT_MODE\", \"live\")",
      "request_id = os.environ.get(\"CODEX_X_SCRIPT_REQUEST_ID\", \"\")",
      "",
      "header, body = split_raw(raw)",
      "# Read/inspect the request here.",
      "# If you rewrite the body, make sure the HTTP headers stay valid.",
      "",
      "if b\"/health\" in header:",
      "    response = b\"HTTP/1.1 200 OK\\r\\nContent-Type: text/plain\\r\\nContent-Length: 2\\r\\n\\r\\nok\"",
      "    sys.stdout.buffer.write(response)",
      "    raise SystemExit(10)",
      "",
      "# Forward the request unchanged.",
      "sys.stdout.buffer.write(raw)",
      "raise SystemExit(0)",
    ].join("\n"),
  );

  const manifest = fence(
    "json",
    [
      "{",
      '  "protocol_version": 1,',
      '  "id": "my-rewriter",',
      '  "name": "My Rewriter",',
      '  "description": "Rewrite request packets",',
      '  "version": "1.0.0",',
      '  "entry": {',
      '    "program": "python",',
      '    "args": ["main.py"]',
      '  },',
      '  "exit": {',
      '    "format": "raw-text",',
      '    "one_request_per_process": true',
      '  },',
      '  "directions": ["request"]',
      "}",
    ].join("\n"),
  );

  return [
    `# ${name}`,
    "",
    intro,
    "",
    heading(2, t("这是什么接口", "What this interface is")),
    isThisInterface,
    "",
    heading(2, t("目录和清单", "Directory and manifest")),
    layout,
    "",
    heading(3, t("manifest.json 规则", "manifest.json rules")),
    t(
      "`protocol_version` 必须是 `1`；`exit.format` 必须是 `raw-text`；`directions` 必须只包含 `request`。入口程序写在 `entry.program` 和 `entry.args` 中。",
      "`protocol_version` must be `1`; `exit.format` must be `raw-text`; `directions` must contain only `request`. The entry command lives in `entry.program` and `entry.args`.",
    ),
    "",
    manifestTitle,
    manifest,
    "",
    heading(2, inputTitle),
    t(
      "stdin 接收完整 HTTP 请求 raw-text。你会拿到请求行、头部、空行和正文。它不是 JSON 对象，也不是已经拆好的字段。",
      "stdin receives the complete HTTP request as raw text. You get the request line, headers, blank line, and body. It is not a JSON object and not a pre-split field map.",
    ),
    "",
    heading(3, envTitle),
    [
      "- `CODEX_X_SCRIPT_MODE`: `live` 或 `test`",
      "- `CODEX_X_SCRIPT_REQUEST_ID`: 当前请求 ID",
      "- `CODEX_X_SCRIPT_DIRECTION`: 当前固定为 `request`",
    ].join("\n"),
    "",
    heading(3, t("输入样本", "Sample input")),
    inputExample,
    "",
    heading(2, authoringTitle),
    [
      t("1. 用 stdin 读取整段 raw-text，不要依赖控制台回显。", "1. Read the full raw text from stdin; do not depend on console echo."),
      t("2. 如需修改正文，务必同步维护 `Content-Length`。", "2. If you modify the body, update `Content-Length` accordingly."),
      t("3. 只把调试日志写到 stderr。stdout 只能输出协议数据。", "3. Write debug logs to stderr only. stdout must contain protocol data only."),
      t("4. 先判定是转发、直接响应还是丢弃，再选择退出码。", "4. Decide whether to forward, respond directly, or drop, then choose the matching exit code."),
    ].join("\n"),
    "",
    heading(2, outputTitle),
    [
      t("- `exit 0`：stdout 必须是一段完整 HTTP 请求 raw-text，网关会继续把它转发到上游。", "- `exit 0`: stdout must be a complete HTTP request raw text, and the gateway will keep forwarding it upstream."),
      t("- `exit 10`：stdout 必须是一段完整 HTTP 响应 raw-text，网关直接把它返回给客户端。", "- `exit 10`: stdout must be a complete HTTP response raw text, and the gateway returns it directly to the client."),
      t("- `exit 11`：表示丢弃当前请求。", "- `exit 11`: drop the current request."),
      t("- 其他非零退出码：视为脚本执行错误，脚本输出不会被当成协议结果。", "- Any other non-zero exit code: script execution error; the output is not treated as a protocol result."),
    ].join("\n"),
    "",
    heading(3, t("转发输出样本", "Forwarded output sample")),
    forwardOutput,
    "",
    heading(3, t("直接响应样本", "Direct response sample")),
    directResponseOutput,
    "",
    heading(2, templateTitle),
    t(
      "下面这个模板展示了最小可用写法：读取 stdin、检查路径、直接响应或原样转发。你可以把它改成 Python、Node、Go、Rust、Shell 或任何可执行程序。",
      "The template below shows a minimal usable script: read stdin, inspect the path, then either return a direct response or forward the request unchanged. You can adapt it to Python, Node, Go, Rust, Shell, or any executable program.",
    ),
    "",
    template,
    "",
    heading(2, notesTitle),
    [
      t("- 不要在 stdout 上打印调试信息。", "- Do not print debug logs to stdout."),
      t("- 不要把请求当成 JSON 字符串整体替换，除非你已经确认它确实是 JSON。", "- Do not blindly replace the request as a JSON string unless you have confirmed it is JSON."),
      t("- 若脚本修改了请求头或正文，必须保证最终 raw-text 仍然是合法 HTTP 报文。", "- If the script modifies headers or body bytes, the final raw text must still be a valid HTTP message."),
      t("- 测试模式只校验协议和结构，不代表真实业务语义。", "- Test mode validates protocol and structure only; it does not prove business correctness."),
    ].join("\n"),
  ].join("\n");
}
