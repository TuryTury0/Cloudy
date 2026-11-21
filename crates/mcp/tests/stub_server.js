// A tiny MCP server over stdio (newline-delimited JSON-RPC, no packages) for the client tests.
const readline = require('node:readline');

const TOOLS = [
  {
    name: 'get-price',
    description: 'Returns the price of an item.',
    inputSchema: {
      type: 'object',
      properties: { item: { type: 'string', enum: ['gold', 'dollar'] }, options: { type: 'object' }, count: { type: ['integer', 'null'] } },
      required: ['item'],
      additionalProperties: false,
      $schema: 'http://json-schema.org/draft-07/schema#'
    }
  },
  { name: 'fails', description: 'Always fails.', inputSchema: { type: 'object', properties: {} } },
  { name: 'crash', description: 'Stops the server.', inputSchema: { type: 'object' } }
];

const send = (message) => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n');
const text = (t, isError = false) => ({ content: [{ type: 'text', text: t }], ...(isError ? { isError: true } : {}) });

function call(name, args) {
  if (name === 'get-price') {
    const price = args.item === 'gold' ? '262,073,000' : '2,695,000';
    return text(`${args.item} = ${price} | options=${JSON.stringify(args.options ?? null)} | env=${process.env.STUB_TOKEN || 'none'}`);
  }
  if (name === 'fails') return text('nothing here', true);
  if (name === 'crash') {
    setTimeout(() => process.exit(1), 50);
    return text('bye');
  }
  return null;
}

readline.createInterface({ input: process.stdin }).on('line', (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (msg.id === undefined || !msg.method) return; // notifications and responses
  const reply = (result) => send({ id: msg.id, result });
  const fail = (code, message) => send({ id: msg.id, error: { code, message } });
  switch (msg.method) {
    case 'initialize':
      return reply({ protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'stub', version: '1.0.0' } });
    case 'ping':
      return reply({});
    case 'tools/list':
      return reply({ tools: TOOLS });
    case 'tools/call': {
      const result = call(msg.params?.name, msg.params?.arguments ?? {});
      return result ? reply(result) : fail(-32602, `Unknown tool ${msg.params?.name}`);
    }
    default:
      return fail(-32601, `Method not found: ${msg.method}`);
  }
});
process.stderr.write('stub server ready\n');
