export function nextCopyName(sourceName: string, existingNames: Iterable<string>): string {
  const existing = new Set(Array.from(existingNames, (name) => name.toLowerCase()));
  const base = `${sourceName}-copy`;

  if (!existing.has(base.toLowerCase())) {
    return base;
  }

  let index = 2;
  while (existing.has(`${base}-${index}`.toLowerCase())) {
    index += 1;
  }

  return `${base}-${index}`;
}
