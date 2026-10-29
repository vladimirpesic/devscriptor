/**
 * A client of Devscriptor's MCP server, through `devscriptor mcp`.
 *
 * `devscriptor mcp` relays to the workspace's shared daemon (`devscriptor serve`),
 * so the extension and the agents working in one folder with the same database
 * share one Devscriptor process: one index, watcher, embedding model and set of
 * language servers. MCP over stdio is JSON-RPC, one message per line.
 *
 * Kept free of the `vscode` API so that it can be tested in plain Node against a
 * fake server.
 */

import { ChildProcess, spawn } from 'child_process';
import * as readline from 'readline';

/** The MCP revision this client speaks (Devscriptor accepts 2024-11-05 to 2025-11-25). */
export const PROTOCOL_VERSION = '2025-06-18';

/**
 * How long the server may take to answer `initialize`. `devscriptor mcp` waits up
 * to 10 s for the workspace's daemon before it serves in its own process.
 */
export const INITIALIZE_TIMEOUT_MS = 60_000;

/**
 * How to start the Devscriptor MCP server for `database`.
 *
 * Always the main binary plus the `mcp` subcommand — never a derived
 * `devscriptor-mcp` name. Devscriptor ships one executable answering to one name:
 * another name would need a symlink, which dangles when the binary is installed
 * alone, or on Windows a `.cmd` shim, which Node refuses to spawn without
 * `shell: true`.
 *
 * `devscriptor mcp` relays to the daemon of its working directory and database,
 * so started in the workspace folder with the database an agent's
 * `devscriptor mcp --database` names too, the extension shares its process.
 *
 * Exported so the test can assert it works for `devscriptor.exe` too, where a
 * name derived from `devscriptor` would not match.
 */
export function mcpSpawnSpec(binary: string, database: string): { command: string; args: string[] } {
    return { command: binary, args: ['mcp', '--database', database] };
}

/** How to start the server. */
export interface ServerCommand {
    command: string;
    args: string[];
    /** The workspace folder: `devscriptor mcp` relays to the daemon of its working directory. */
    cwd: string;
    env?: NodeJS.ProcessEnv;
}

/** The server could not be started or reached, or went away: the CLI can do the work instead. */
export class McpConnectionError extends Error {
    constructor(message: string) {
        super(message);
        this.name = 'McpConnectionError';
    }
}

/** The server answered a request with an error, such as a tool that failed. */
export class McpError extends Error {
    constructor(message: string, readonly code: number) {
        super(message);
        this.name = 'McpError';
    }
}

/** A JSON-RPC message, in either direction. */
interface Message {
    jsonrpc: '2.0';
    id?: number | string | null;
    method?: string;
    params?: unknown;
    result?: unknown;
    error?: { code: number; message: string };
}

/** What `tools/call` answers. */
interface ToolResult {
    content?: { type: string; text?: string }[];
    isError?: boolean;
}

interface Pending {
    resolve(result: unknown): void;
    reject(error: Error): void;
}

export class McpClient {
    private child: ChildProcess | undefined;
    private connecting: Promise<void> | undefined;
    private ready = false;
    private readonly pending = new Map<number, Pending>();
    private nextId = 0;

    /**
     * @param server How to start the server.
     * @param clientVersion The version this client gives in `initialize`.
     * @param log Receives the server's stderr, and why a connection ended.
     * @param initializeTimeoutMs How long `initialize` may take.
     */
    constructor(
        readonly server: ServerCommand,
        private readonly clientVersion: string,
        private readonly log: (text: string) => void = () => undefined,
        private readonly initializeTimeoutMs = INITIALIZE_TIMEOUT_MS,
    ) {}

    /** Whether a connection is open and past its handshake. */
    get connected(): boolean {
        return this.ready;
    }

    /**
     * Start the server and complete the MCP handshake, unless a connection is
     * open or opening. A connection that ended is opened again.
     *
     * @throws McpConnectionError if the server cannot be started or does not
     * complete the handshake.
     */
    connect(): Promise<void> {
        if (!this.connecting) {
            const opening = this.open();
            this.connecting = opening;
            opening.catch(() => {
                if (this.connecting === opening) {
                    this.close();
                }
            });
        }
        return this.connecting;
    }

    /**
     * Call tool `name` with `args`, connecting first if needed. Devscriptor's
     * tools answer with one JSON text block, which is returned parsed.
     *
     * @throws McpConnectionError if the server cannot be reached or goes away.
     * @throws McpError if the tool fails.
     */
    async callTool(name: string, args: Record<string, unknown>): Promise<unknown> {
        await this.connect();
        const result = (await this.request('tools/call', { name, arguments: args })) as ToolResult;
        const text = result.content?.find((block) => block.type === 'text')?.text;
        if (result.isError) {
            throw new McpError(text ?? `${name} failed`, 0);
        }
        if (text === undefined) {
            throw new McpError(`${name} answered without text`, 0);
        }
        return JSON.parse(text);
    }

    /**
     * Close the connection. `devscriptor mcp` exits; the shared daemon goes on
     * serving its other clients, and stops once none has been connected for a
     * while.
     */
    close(): void {
        const child = this.child;
        this.child = undefined;
        this.connecting = undefined;
        this.ready = false;
        this.rejectAll(new McpConnectionError('the connection was closed'));
        if (child && child.exitCode === null && child.signalCode === null) {
            child.stdin?.end();
            child.kill();
        }
    }

    private async open(): Promise<void> {
        const { command, args, cwd, env } = this.server;
        const child = spawn(command, args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'] });
        this.child = child;
        child.stderr?.on('data', (chunk: Buffer) => this.log(chunk.toString()));
        if (child.stdout) {
            readline.createInterface({ input: child.stdout }).on('line', (line) => this.receive(line));
        }
        // A write to a server that exited fails with EPIPE; `exit` reports why.
        child.stdin?.on('error', () => undefined);
        child.on('error', (error) => this.lost(child, `cannot run ${command}: ${error.message}`));
        child.on('exit', (code, signal) =>
            this.lost(child, `devscriptor mcp exited (${signal ?? `code ${code}`})`));

        let timer: NodeJS.Timeout | undefined;
        const timeout = new Promise<never>((_, reject) => {
            timer = setTimeout(() => reject(new McpConnectionError(
                `devscriptor mcp did not answer initialize within ${this.initializeTimeoutMs / 1000} s`)),
            this.initializeTimeoutMs);
        });
        try {
            await Promise.race([
                this.request('initialize', {
                    protocolVersion: PROTOCOL_VERSION,
                    capabilities: {},
                    clientInfo: { name: 'devscriptor-vscode', version: this.clientVersion },
                }),
                timeout,
            ]);
        } catch (error) {
            throw error instanceof McpError
                ? new McpConnectionError(`devscriptor mcp refused initialize: ${error.message}`)
                : error;
        } finally {
            clearTimeout(timer);
        }
        this.send({ jsonrpc: '2.0', method: 'notifications/initialized' });
        this.ready = this.child === child;
    }

    /** `child`, the current server or one replaced already, has gone. */
    private lost(child: ChildProcess, reason: string): void {
        if (this.child !== child) {
            return;
        }
        this.child = undefined;
        this.connecting = undefined;
        this.ready = false;
        this.log(`[${reason}]\n`);
        this.rejectAll(new McpConnectionError(reason));
    }

    private rejectAll(error: Error): void {
        const pending = [...this.pending.values()];
        this.pending.clear();
        for (const request of pending) {
            request.reject(error);
        }
    }

    private receive(line: string): void {
        if (!line.trim()) {
            return;
        }
        let message: Message;
        try {
            message = JSON.parse(line) as Message;
        } catch {
            this.log(`[not JSON-RPC: ${line}]\n`);
            return;
        }
        if (message.method !== undefined) {
            // A request or notification of the server's own. This client
            // declares no capabilities, so it only answers `ping`.
            if (message.id !== undefined && message.id !== null) {
                this.send(message.method === 'ping'
                    ? { jsonrpc: '2.0', id: message.id, result: {} }
                    : {
                        jsonrpc: '2.0',
                        id: message.id,
                        error: { code: -32601, message: `${message.method} is not supported` },
                    });
            }
            return;
        }
        const request = typeof message.id === 'number' ? this.pending.get(message.id) : undefined;
        if (!request) {
            return;
        }
        this.pending.delete(message.id as number);
        if (message.error) {
            request.reject(new McpError(message.error.message, message.error.code));
        } else {
            request.resolve(message.result);
        }
    }

    private request(method: string, params: unknown): Promise<unknown> {
        const id = ++this.nextId;
        return new Promise((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
            if (!this.send({ jsonrpc: '2.0', id, method, params })) {
                this.pending.delete(id);
                reject(new McpConnectionError('the connection is closed'));
            }
        });
    }

    /** Write `message`; false when there is no server to write to. */
    private send(message: Message): boolean {
        const stdin = this.child?.stdin;
        if (!stdin || !stdin.writable) {
            return false;
        }
        stdin.write(`${JSON.stringify(message)}\n`);
        return true;
    }
}

/**
 * The result of `viaMcp`, unless the server cannot be reached: then that of
 * `viaCli`, after `onFallback` is told why. A tool's own error is no reason to
 * run the CLI, and is thrown.
 */
export async function withCliFallback<T>(
    viaMcp: () => Promise<T>,
    viaCli: () => Promise<T>,
    onFallback: (reason: McpConnectionError) => void,
): Promise<T> {
    try {
        return await viaMcp();
    } catch (error) {
        if (!(error instanceof McpConnectionError)) {
            throw error;
        }
        onFallback(error);
        return viaCli();
    }
}
