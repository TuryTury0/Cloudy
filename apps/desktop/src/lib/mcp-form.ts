// Helpers for the MCP server form: one command line in, command + args out; KEY=VALUE lines for env and headers.

/** Splits a command line like a shell does for simple cases: spaces separate, quotes keep spaces together. */
export function splitCommandLine(line: string): string[] {
  const parts: string[] = [];
  let current = '';
  let quote: '"' | "'" | null = null;
  let started = false;
  for (const ch of line.trim()) {
    if (quote) {
      if (ch === quote) quote = null;
      else current += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      started = true;
    } else if (/\s/.test(ch)) {
      if (started || current) parts.push(current);
      current = '';
      started = false;
    } else {
      current += ch;
    }
  }
  if (started || current) parts.push(current);
  return parts;
}

/** The reverse: quotes only the parts that need it. */
export function joinCommandLine(parts: string[]): string {
  return parts.map((p) => (p === '' || /[\s"']/.test(p) ? `"${p.replace(/"/g, '')}"` : p)).join(' ');
}

/** "KEY=VALUE" per line (empty lines and lines without "=" are skipped; the first "=" splits). */
export function parseKeyValues(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    const at = line.indexOf('=');
    if (at <= 0) continue;
    out[line.slice(0, at).trim()] = line.slice(at + 1).trim();
  }
  return out;
}

export function formatKeyValues(values: Record<string, string> | undefined): string {
  return Object.entries(values ?? {})
    .map(([k, v]) => `${k}=${v}`)
    .join('\n');
}
