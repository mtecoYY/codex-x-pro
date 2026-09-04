export type GatewayDetailView = "raw-text" | "request body JSON" | "response body JSON" | "conversation";

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
  if (view === "conversation") return conversationMessages(probe.request_body_json ?? probe.response_body_json);
  return probe.response_body_json ?? null;
}

export type GatewayConversationMessage = {
  role: string;
  text: string;
};

function contentText(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) {
    return value.map((item) => contentText(item)).filter(Boolean).join("\n");
  }
  if (value && typeof value === "object") {
    const item = value as Record<string, unknown>;
    for (const key of ["text", "value", "content", "output_text", "arguments"]) {
      if (item[key] !== undefined) {
        const text = contentText(item[key]);
        if (text) return text;
      }
    }
    return JSON.stringify(value, null, 2);
  }
  return value == null ? "" : String(value);
}

function messageFrom(value: unknown, fallbackRole = "message"): GatewayConversationMessage | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  const role = typeof item.role === "string" ? item.role : fallbackRole;
  const text = contentText(item.content ?? item.text ?? item.output_text ?? item.arguments ?? item);
  return text ? { role, text } : null;
}

export function conversationMessages(value: unknown): GatewayConversationMessage[] | null {
  if (!value) return null;
  if (Array.isArray(value)) {
    const messages = value.flatMap((item) => {
      const message = messageFrom(item);
      return message ? [message] : [];
    });
    return messages.length ? messages : null;
  }
  if (typeof value !== "object") return null;
  const document = value as Record<string, unknown>;
  const messages: GatewayConversationMessage[] = [];
  if (Array.isArray(document.messages)) {
    messages.push(...document.messages.flatMap((item) => {
      const message = messageFrom(item);
      return message ? [message] : [];
    }));
  }
  if (Array.isArray(document.input)) {
    messages.push(...document.input.flatMap((item) => {
      const message = messageFrom(item, "user");
      return message ? [message] : [];
    }));
  }
  if (Array.isArray(document.output)) {
    messages.push(...document.output.flatMap((item) => {
      const message = messageFrom(item, "assistant");
      return message ? [message] : [];
    }));
  }
  if (Array.isArray(document.choices)) {
    messages.push(...document.choices.flatMap((item) => {
      const choice = item && typeof item === "object" ? item as Record<string, unknown> : {};
      const message = messageFrom(choice.message ?? choice.delta, "assistant");
      return message ? [message] : [];
    }));
  }
  return messages.length ? messages : null;
}

export function conversationText(value: unknown): string | null {
  const messages = conversationMessages(value);
  if (!messages) return null;
  return messages.map((message) => `${message.role}\n${message.text}`).join("\n\n");
}

export function gatewayProbeTruncation(probe: GatewayProbe | undefined): string | null {
  if (!probe || !probe.raw_text_truncated) return null;
  const reason = typeof probe.truncate_reason === "string" ? probe.truncate_reason : "OBSERVE_DETAIL_TRUNCATED";
  const original = typeof probe.original_bytes === "number" ? probe.original_bytes : "?";
  const retained = typeof probe.retained_bytes === "number" ? probe.retained_bytes : "?";
  return `${reason}: original_bytes=${original}, retained_bytes=${retained}`;
}
