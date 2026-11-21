import { describe, expect, it } from 'vitest';
import { formatKeyValues, joinCommandLine, parseKeyValues, splitCommandLine } from './mcp-form';

describe('splitCommandLine', () => {
  it('splits on spaces and keeps quoted parts together', () => {
    expect(splitCommandLine('npx -y @modelcontextprotocol/server-github')).toEqual(['npx', '-y', '@modelcontextprotocol/server-github']);
    expect(splitCommandLine('  node "C:\\My Tools\\server.js"  --port 3 ')).toEqual(['node', 'C:\\My Tools\\server.js', '--port', '3']);
    expect(splitCommandLine(`uvx mcp-server --name 'a b' ""`)).toEqual(['uvx', 'mcp-server', '--name', 'a b', '']);
    expect(splitCommandLine('   ')).toEqual([]);
  });

  it('round-trips through joinCommandLine', () => {
    const parts = ['node', 'C:\\My Tools\\server.js', '--flag'];
    expect(splitCommandLine(joinCommandLine(parts))).toEqual(parts);
  });
});

describe('key values', () => {
  it('reads KEY=VALUE lines, keeping "=" inside values', () => {
    expect(parseKeyValues('TOKEN=secret:GITHUB\n\nbad line\nA = b=c ')).toEqual({ TOKEN: 'secret:GITHUB', A: 'b=c' });
    expect(formatKeyValues({ A: '1', B: 'secret:X' })).toBe('A=1\nB=secret:X');
    expect(formatKeyValues(undefined)).toBe('');
  });
});
