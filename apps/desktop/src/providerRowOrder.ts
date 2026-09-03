type OrderedProviderRow = {
  isCurrent: boolean;
};

export function orderProviderRows<
  Official extends OrderedProviderRow,
  Detected extends OrderedProviderRow,
  Local extends OrderedProviderRow,
>(
  official: Official,
  detected: readonly Detected[],
  local: readonly Local[],
): Array<Official | Detected | Local> {
  const rows: Array<Official | Detected | Local> = [official];
  if (!local.some((row) => row.isCurrent)) rows.push(...detected);
  rows.push(...local);
  return rows;
}
