export type ProviderProfile = {
  baseUrl?: string | null;
  providerName?: string | null;
  model?: string | null;
  apiKey?: string | null;
};

function normalizeProviderBaseUrl(value?: string | null) {
  const raw = (value || "").trim();
  if (!raw) return "";
  try {
    const parsed = new URL(raw);
    const credentials = parsed.username
      ? `${parsed.username}${parsed.password ? `:${parsed.password}` : ""}@`
      : "";
    const path = parsed.pathname.replace(/\/+$/, "");
    return `${parsed.protocol.toLowerCase()}//${credentials}${parsed.host.toLowerCase()}${path}${parsed.search}`;
  } catch {
    return raw.replace(/\/+$/, "");
  }
}

function normalizeProviderName(value?: string | null) {
  return (value || "").trim().replace(/\s+/gu, " ").toLowerCase();
}

export function providerProfilesMatch(left: ProviderProfile, right: ProviderProfile) {
  const leftUrl = normalizeProviderBaseUrl(left.baseUrl);
  if (!leftUrl || leftUrl !== normalizeProviderBaseUrl(right.baseUrl)) return false;
  if ((left.model || "").trim() !== (right.model || "").trim()) return false;

  const leftKey = (left.apiKey || "").trim();
  const rightKey = (right.apiKey || "").trim();
  if (leftKey && rightKey) return leftKey === rightKey;
  return normalizeProviderName(left.providerName) === normalizeProviderName(right.providerName);
}
