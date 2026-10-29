import * as assert from 'assert';
import * as path from 'path';
import {
    McpClient,
    McpConnectionError,
    McpError,
    PROTOCOL_VERSION,
    withCliFallback,
} from '../../mcpClient';

/** The stand-in for `devscriptor mcp`, compiled next to this suite. */
const FAKE_SERVER = path.join(__dirname, '..', 'fakeMcpServer.js');

/** A client of the fake server, started with `flags`. */
function fake(flags: string[] = [], initializeTimeoutMs?: number): McpClient {
    return new McpClient(
        {
            command: process.execPath,
            args: [FAKE_SERVER, ...flags],
            cwd: __dirname,
            // Under the VS Code test host, `process.execPath` is Electron, which
            // runs a script as Node only when asked to.
            env: { ...process.env, ELECTRON_RUN_AS_NODE: '1' },
        },
        '1.2.3',
        () => undefined,
        initializeTimeoutMs,
    );
}

/** The process id of the fake server `client` talks to. */
async function serverPid(client: McpClient): Promise<number> {
    return ((await client.callTool('who', {})) as { pid: number }).pid;
}

describe('MCP client', function () {
    this.timeout(20000);
    let client: McpClient | undefined;

    afterEach(() => {
        client?.close();
        client = undefined;
    });

    it('completes the handshake and answers with the tool result', async () => {
        client = fake();
        assert.deepStrictEqual(await client.callTool('echo', { query: 'x', limit: 2 }), { query: 'x', limit: 2 });
        assert.ok(client.connected);
        const { initialize } = (await client.callTool('who', {})) as {
            initialize: { protocolVersion: string; clientInfo: unknown };
        };
        assert.strictEqual(initialize.protocolVersion, PROTOCOL_VERSION);
        assert.deepStrictEqual(initialize.clientInfo, { name: 'devscriptor-vscode', version: '1.2.3' });
    });

    it('gives each call its own answer', async () => {
        client = fake();
        const [slow, quick] = await Promise.all([
            client.callTool('slow', { ms: 300 }),
            client.callTool('echo', { quick: true }),
        ]);
        assert.deepStrictEqual(slow, { ms: 300 });
        assert.deepStrictEqual(quick, { quick: true });
    });

    it('starts one server for calls made before it answers', async () => {
        client = fake();
        const [first, second] = await Promise.all([serverPid(client), serverPid(client)]);
        assert.strictEqual(first, second);
    });

    it('rejects a failing tool with its error, and stays connected', async () => {
        client = fake();
        await assert.rejects(
            client.callTool('fail', {}),
            (error: Error) => error instanceof McpError && error.message === 'it failed',
        );
        assert.ok(client.connected);
        assert.deepStrictEqual(await client.callTool('echo', {}), {});
    });

    it('loses the calls of a server that exits, and starts another for the next', async () => {
        client = fake();
        const first = await serverPid(client);
        await assert.rejects(client.callTool('exit', {}), McpConnectionError);
        assert.strictEqual(client.connected, false);
        const second = await serverPid(client);
        assert.notStrictEqual(second, first);
    });

    it("answers the server's ping, and refuses the requests it has no capability for", async () => {
        client = fake();
        const pong = (await client.callTool('ping', {})) as { result?: unknown };
        assert.deepStrictEqual(pong.result, {});
        const refused = (await client.callTool('sample', {})) as { error?: { code: number } };
        assert.strictEqual(refused.error?.code, -32601);
    });

    it('skips output that is not JSON-RPC', async () => {
        client = fake(['--noise']);
        assert.deepStrictEqual(await client.callTool('echo', { a: 1 }), { a: 1 });
    });

    it('closes the connection, and opens another when called again', async () => {
        client = fake();
        const first = await serverPid(client);
        client.close();
        assert.strictEqual(client.connected, false);
        assert.notStrictEqual(await serverPid(client), first);
    });

    it('cannot connect to a command that does not run', async () => {
        client = new McpClient(
            { command: path.join(__dirname, 'no-such-devscriptor'), args: ['mcp'], cwd: __dirname },
            '1.2.3',
        );
        await assert.rejects(client.connect(), McpConnectionError);
        assert.strictEqual(client.connected, false);
    });

    it('gives up on a server that does not answer initialize', async () => {
        client = fake(['--silent'], 500);
        await assert.rejects(
            client.connect(),
            (error: Error) => error instanceof McpConnectionError && /did not answer initialize/.test(error.message),
        );
    });
});

describe('CLI fallback', () => {
    it('runs the CLI when the server cannot be reached, and says why', async () => {
        const reasons: string[] = [];
        const result = await withCliFallback(
            async () => {
                throw new McpConnectionError('devscriptor mcp exited (code 1)');
            },
            async () => 'from the CLI',
            (reason) => reasons.push(reason.message),
        );
        assert.strictEqual(result, 'from the CLI');
        assert.deepStrictEqual(reasons, ['devscriptor mcp exited (code 1)']);
    });

    it("passes a tool's own error on", async () => {
        await assert.rejects(
            withCliFallback(
                async () => {
                    throw new McpError('Unknown tool: x', -32602);
                },
                async () => 'from the CLI',
                () => assert.fail('the CLI must not run'),
            ),
            McpError,
        );
    });

    it('runs no CLI when the server answers', async () => {
        const result = await withCliFallback(
            async () => 'from the server',
            async () => assert.fail('the CLI must not run'),
            () => assert.fail('no fallback'),
        );
        assert.strictEqual(result, 'from the server');
    });
});
