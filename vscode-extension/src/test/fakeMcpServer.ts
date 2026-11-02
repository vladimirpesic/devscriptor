/**
 * A stand-in for `devscriptor mcp` in the client's tests: JSON-RPC over stdio,
 * one message per line. With `--silent` it never answers `initialize`; with
 * `--noise` it first writes a line that is not JSON-RPC.
 *
 * Its tools: `echo` answers its arguments; `slow` answers them after `ms`
 * milliseconds; `fail` answers with an error; `exit` ends the process without an
 * answer; `ping` and `sample` send the client a request of their name and answer
 * with the client's reply; `who` answers the `initialize` parameters it got and
 * its process id.
 */

import * as readline from 'readline';

interface Message {
    id?: number;
    method?: string;
    params?: { protocolVersion?: string; name?: string; arguments?: Record<string, unknown> };
}

const silent = process.argv.includes('--silent');
if (process.argv.includes('--noise')) {
    process.stdout.write('not JSON-RPC\n');
}

let initializeParams: unknown;
let nextRequest = 1000;
const replies = new Map<number, (reply: unknown) => void>();

function send(message: object): void {
    process.stdout.write(`${JSON.stringify(message)}\n`);
}

/** Answer tool call `id` with `value` as its JSON text. */
function answer(id: number | undefined, value: unknown): void {
    send({ jsonrpc: '2.0', id, result: { content: [{ type: 'text', text: JSON.stringify(value) }], isError: false } });
}

/** Ask the client `method`, and answer tool call `id` with its reply. */
function ask(id: number | undefined, method: string, params: object): void {
    const request = ++nextRequest;
    replies.set(request, (reply) => answer(id, reply));
    send({ jsonrpc: '2.0', id: request, method, params });
}

function call(id: number | undefined, name: string | undefined, args: Record<string, unknown>): void {
    switch (name) {
        case 'echo':
            return answer(id, args);
        case 'slow':
            setTimeout(() => answer(id, args), Number(args.ms));
            return;
        case 'fail':
            return send({ jsonrpc: '2.0', id, error: { code: -32603, message: 'it failed' } });
        case 'exit':
            process.exit(3);
            return;
        case 'ping':
            return ask(id, 'ping', {});
        case 'sample':
            return ask(id, 'sampling/createMessage', { messages: [], maxTokens: 1 });
        case 'who':
            return answer(id, { initialize: initializeParams, pid: process.pid });
        default:
            return send({ jsonrpc: '2.0', id, error: { code: -32602, message: `Unknown tool: ${name}` } });
    }
}

const input = readline.createInterface({ input: process.stdin });
input.on('close', () => process.exit(0));
input.on('line', (line) => {
    const message = JSON.parse(line) as Message & { result?: unknown; error?: unknown };
    if (message.method === undefined) {
        const reply = message.id === undefined ? undefined : replies.get(message.id);
        if (reply && message.id !== undefined) {
            replies.delete(message.id);
            reply(message);
        }
        return;
    }
    switch (message.method) {
        case 'initialize':
            if (!silent) {
                initializeParams = message.params;
                send({
                    jsonrpc: '2.0',
                    id: message.id,
                    result: {
                        protocolVersion: message.params?.protocolVersion,
                        capabilities: { tools: {} },
                        serverInfo: { name: 'fake', version: '0' },
                    },
                });
            }
            return;
        case 'notifications/initialized':
            return;
        case 'tools/call':
            return call(message.id, message.params?.name, message.params?.arguments ?? {});
        default:
            send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: `no ${message.method}` } });
    }
});
