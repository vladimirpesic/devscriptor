import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { spawn } from 'child_process';
import * as which from 'which';
import {
    CallGraph,
    ComplexityResult,
    DeadCodeResult,
    Reference,
    SearchResult,
    parseCallGraph,
    parseComplexity,
    parseDeadCode,
    parseEntityCount,
    parseLocations,
    parseSearchResults,
    toReferences,
} from './cliOutput';
import { McpClient, McpConnectionError, mcpSpawnSpec, withCliFallback } from './mcpClient';
import * as mcpOutput from './mcpOutput';
import { DOWNLOAD_PAGE_URL, parseInstalledVersion, versionOutput } from './releases';

export type { CallGraph, ComplexityResult, DeadCodeResult, Reference, SearchResult } from './cliOutput';
export { mcpSpawnSpec } from './mcpClient';

/**
 * The database unless `devscriptor.databasePath` names another: the one
 * `devscriptor scan` writes.
 */
export const DEFAULT_DATABASE = 'devscriptor.db';

/** How many hits a search returns: `devscriptor search`'s default. */
const SEARCH_LIMIT = 20;

/** More findings than any workspace has: list them all, as the CLI's `analyze` does. */
const ALL_FINDINGS = 1_000_000;

/** How an MCP client starts the workspace's Devscriptor. */
export interface McpServerSpec {
    command: string;
    args: string[];
    /** The workspace folder: `devscriptor mcp` relays to the daemon of its working directory. */
    cwd: string;
    /** The installed Devscriptor's version, if it tells. */
    version?: string;
}

export interface DevscriptorStatus {
    binaryVersion?: string;
    databaseExists: boolean;
    entityCount?: number;
    /** Whether the extension is connected to the workspace's Devscriptor. */
    mcpRunning: boolean;
}

/**
 * Runs the extension's commands through one MCP connection to the workspace's
 * Devscriptor (`devscriptor mcp`, which relays to the shared daemon), and through
 * the CLI when that connection cannot be made.
 */
export class DevscriptorManager {
    private binaryPath: string | undefined;
    private client: McpClient | undefined;
    private outputChannel: vscode.OutputChannel | undefined;
    private context: vscode.ExtensionContext;

    constructor(context: vscode.ExtensionContext) {
        this.context = context;
        this.findBinary();
        // A binary named in the settings takes effect at the next command.
        context.subscriptions.push(vscode.workspace.onDidChangeConfiguration((event) => {
            if (event.affectsConfiguration('devscriptor.binaryPath')) {
                this.binaryPath = undefined;
            }
        }));
    }

    private async findBinary(): Promise<void> {
        const config = vscode.workspace.getConfiguration('devscriptor');
        const configuredPath = config.get<string>('binaryPath');

        if (configuredPath && fs.existsSync(configuredPath)) {
            this.binaryPath = configuredPath;
            return;
        }

        // Try to find in PATH
        try {
            this.binaryPath = await which('devscriptor');
        } catch {
            // Not in PATH
        }

        // Check common locations
        const commonPaths = [
            path.join(process.env.HOME || '', '.local', 'bin', 'devscriptor'),
            path.join(process.env.HOME || '', '.cargo', 'bin', 'devscriptor'),
            '/usr/local/bin/devscriptor',
            '/usr/bin/devscriptor',
        ];

        for (const p of commonPaths) {
            if (fs.existsSync(p)) {
                this.binaryPath = p;
                break;
            }
        }
    }

    public async ensureBinary(): Promise<string> {
        if (!this.binaryPath) {
            await this.findBinary();
        }

        if (!this.binaryPath || !fs.existsSync(this.binaryPath)) {
            const action = await vscode.window.showErrorMessage(
                'Devscriptor binary not found. Would you like to install it?',
                'Install',
                'Configure Path'
            );

            if (action === 'Install') {
                await this.installBinary();
            } else if (action === 'Configure Path') {
                const result = await vscode.window.showInputBox({
                    prompt: 'Enter path to devscriptor binary',
                    validateInput: (value) => {
                        if (!value || !fs.existsSync(value)) {
                            return 'File does not exist';
                        }
                        return null;
                    }
                });

                if (result) {
                    const config = vscode.workspace.getConfiguration('devscriptor');
                    await config.update('binaryPath', result, true);
                    this.binaryPath = result;
                }
            }
        }

        if (!this.binaryPath) {
            throw new Error('Devscriptor binary not found');
        }

        return this.binaryPath;
    }

    private async installBinary(): Promise<void> {
        // Direct the user to the download page; the extension does not
        // perform unattended downloads to keep the install auditable and secure.
        const action = await vscode.window.showInformationMessage(
            'Download Devscriptor for your platform and place the devscriptor binary on your PATH.',
            'Open Download Page'
        );
        if (action === 'Open Download Page') {
            await vscode.env.openExternal(vscode.Uri.parse(DOWNLOAD_PAGE_URL));
        }
        await this.findBinary();
    }

    public async scanWorkspace(rootPath: string, onProgress?: (message: string) => void): Promise<void> {
        await this.viaServer(
            'scan_codebase',
            { path: rootPath },
            (answer) => onProgress?.(mcpOutput.scanSummary(answer)),
            async () => {
                await this.cli(['scan', rootPath], 'Scan failed', (text) => onProgress?.(text.trim()));
            },
        );
    }

    /**
     * The code that matches `query`: by meaning (`semantic_search`, with the
     * embedding model) when `semantic`, else by name (`search_entities`).
     */
    public async search(query: string, semantic: boolean): Promise<SearchResult[]> {
        const cliArgs = ['search', query, '--limit', String(SEARCH_LIMIT), '--output', 'json'];
        const viaCli = async (searchType: string) => readOr(
            await this.cli([...cliArgs, '--search-type', searchType], 'Search failed'),
            parseSearchResults,
            [],
        );
        return semantic
            ? this.viaServer(
                'semantic_search',
                { query, limit: SEARCH_LIMIT },
                mcpOutput.searchResults,
                () => viaCli('semantic'),
            )
            : this.viaServer(
                'search_entities',
                { query, limit: SEARCH_LIMIT },
                mcpOutput.entitySearchResults,
                () => viaCli('symbol'),
            );
    }

    public async findDeadCode(): Promise<DeadCodeResult[]> {
        return this.viaServer(
            'find_dead_code',
            { path: this.workspaceFolder() ?? '.', limit: ALL_FINDINGS },
            mcpOutput.deadCode,
            async () => readOr(
                await this.cli(['analyze', '--kind', 'dead-code', '--output', 'json'], 'Analysis failed'),
                parseDeadCode,
                [],
            ),
        );
    }

    public async analyzeComplexity(): Promise<ComplexityResult[]> {
        return this.viaServer(
            'analyze_complexity',
            { path: this.workspaceFolder() ?? '.', top_n: ALL_FINDINGS },
            mcpOutput.complexity,
            async () => readOr(
                await this.cli(['analyze', '--kind', 'complexity', '--output', 'json'], 'Analysis failed'),
                parseComplexity,
                [],
            ),
        );
    }

    /**
     * The relationships of `symbol`, by the names of their ends: through the
     * server (`get_relationships`) or the CLI (`call-graph`).
     */
    public async getCallGraph(symbol: string): Promise<CallGraph> {
        return this.viaServer(
            'get_relationships',
            { entity_name: symbol },
            mcpOutput.relationships,
            async () => readOr(
                await this.cli(['call-graph', '--entity', symbol, '--format', 'json'], 'Call graph failed'),
                parseCallGraph,
                { entity: symbol, edges: [] },
            ),
        );
    }

    /**
     * The definition of the symbol at `line` and `character` of `filePath`, both
     * from 0, from the live language server.
     */
    public async getDefinition(filePath: string, line: number, character: number): Promise<Reference | undefined> {
        const position = [filePath, '--line', String(line), '--character', String(character)];
        return this.viaServer(
            'get_definition',
            { file_path: filePath, line, character },
            (answer) => toReferences(answer)[0],
            async () => readOr(
                await this.cli(['definition', ...position], 'Definition lookup failed'),
                (stdout) => parseLocations(stdout)[0],
                undefined,
            ),
        );
    }

    /** The references to the symbol at `line` and `character` of `filePath`, both from 0. */
    public async getReferences(filePath: string, line: number, character: number): Promise<Reference[]> {
        const position = [filePath, '--line', String(line), '--character', String(character)];
        return this.viaServer(
            'get_references',
            { file_path: filePath, line, character },
            toReferences,
            async () => readOr(
                await this.cli(['references', ...position], 'Reference lookup failed'),
                parseLocations,
                [],
            ),
        );
    }

    /**
     * Connect to the workspace's Devscriptor, starting its daemon if none runs.
     *
     * @throws McpConnectionError if it cannot be reached.
     */
    public async startMcpServer(): Promise<void> {
        await this.mcp();
    }

    /**
     * Close the extension's connection. The workspace's Devscriptor goes on
     * serving its other clients, and stops once none has been connected for 10
     * minutes.
     */
    public async stopMcpServer(): Promise<void> {
        this.client?.close();
        this.client = undefined;
    }

    /**
     * How an AI assistant starts the workspace's Devscriptor: `devscriptor mcp`
     * with the extension's database, in the first workspace folder, as the
     * extension's own connection does. No prompt: VS Code asks for it
     * unbidden.
     *
     * @throws Error saying why when no workspace folder is open or no
     * Devscriptor is installed.
     */
    public async mcpServer(): Promise<McpServerSpec> {
        const folder = this.workspaceFolder();
        if (!folder) {
            throw new Error('no workspace folder is open');
        }
        const version = await this.installedVersion();
        if (!this.binaryPath || !fs.existsSync(this.binaryPath)) {
            throw new Error('no devscriptor binary was found; set devscriptor.binaryPath');
        }
        return { ...mcpSpawnSpec(this.binaryPath, this.getDatabasePath()), cwd: folder, version };
    }

    /**
     * The version of the installed Devscriptor, or `undefined` when none is
     * found (no prompt: checking for an update must not ask for a path).
     */
    public async installedVersion(): Promise<string | undefined> {
        if (!this.binaryPath) {
            await this.findBinary();
        }
        if (!this.binaryPath || !fs.existsSync(this.binaryPath)) {
            return undefined;
        }
        try {
            return parseInstalledVersion(await this.getVersion());
        } catch {
            return undefined;
        }
    }

    public async getStatus(): Promise<DevscriptorStatus> {
        const status: DevscriptorStatus = {
            databaseExists: false,
            mcpRunning: this.client?.connected ?? false,
        };

        if (this.binaryPath) {
            try {
                const version = await this.getVersion();
                status.binaryVersion = version;
            } catch {
                // Ignore errors
            }
        }

        const databasePath = this.getDatabasePath();
        status.databaseExists = fs.existsSync(databasePath);

        if (status.databaseExists) {
            try {
                status.entityCount = await this.getEntityCount();
            } catch {
                // Ignore errors
            }
        }

        return status;
    }

    private async getVersion(): Promise<string> {
        if (!this.binaryPath) {
            throw new Error('Binary path not set');
        }
        return versionOutput(this.binaryPath);
    }

    private async getEntityCount(): Promise<number> {
        return this.viaServer(
            'get_statistics',
            {},
            mcpOutput.entityCount,
            // `stats` prints JSON directly.
            () => this.cli(['stats'], 'Statistics failed').then(
                (stdout) => readOr(stdout, parseEntityCount, 0),
                () => 0,
            ),
        );
    }

    /** The first workspace folder: the workspace whose Devscriptor the extension uses. */
    private workspaceFolder(): string | undefined {
        return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    }

    private getDatabasePath(): string {
        const config = vscode.workspace.getConfiguration('devscriptor');
        const configuredPath = config.get<string>('databasePath', DEFAULT_DATABASE);

        if (path.isAbsolute(configuredPath)) {
            return configuredPath;
        }

        const folder = this.workspaceFolder();
        return folder ? path.join(folder, configuredPath) : configuredPath;
    }

    /**
     * The connection to the workspace's Devscriptor, opened on first use, and
     * again when the binary, the database or the workspace folder changed.
     *
     * @throws McpConnectionError if the server cannot be reached.
     */
    private async mcp(): Promise<McpClient> {
        const binary = await this.ensureBinary();
        const folder = this.workspaceFolder();
        if (!folder) {
            throw new McpConnectionError('no workspace folder is open');
        }
        const { command, args } = mcpSpawnSpec(binary, this.getDatabasePath());
        const server = this.client?.server;
        if (!this.client || server?.command !== command || server.cwd !== folder
            || server.args.join('\0') !== args.join('\0')) {
            this.client?.close();
            const version = String(this.context.extension.packageJSON.version ?? '0.0.0');
            this.client = new McpClient({ command, args, cwd: folder }, version, (text) => this.output().append(text));
        }
        await this.client.connect();
        return this.client;
    }

    /**
     * What tool `name` answers to `args`, read by `read`; what `viaCli` finds
     * when the workspace's Devscriptor cannot be reached. A tool's own error is
     * thrown.
     */
    private viaServer<T>(
        name: string,
        args: Record<string, unknown>,
        read: (answer: unknown) => T,
        viaCli: () => Promise<T>,
    ): Promise<T> {
        return withCliFallback(
            async () => read(await (await this.mcp()).callTool(name, args)),
            viaCli,
            (reason) => this.output().appendLine(`[${name}: running the CLI instead: ${reason.message}]`),
        );
    }

    /**
     * Run the CLI with `args` and the database, in the workspace folder, and
     * return its stdout; `onOutput` sees it as it comes. Rejects with `failure`
     * and the exit code if the command fails.
     */
    private async cli(args: string[], failure: string, onOutput?: (text: string) => void): Promise<string> {
        const binary = await this.ensureBinary();
        const command = [...args, '--database', this.getDatabasePath()];
        return new Promise((resolve, reject) => {
            const child = spawn(binary, command, { cwd: this.workspaceFolder() });
            let stdout = '';
            child.stdout.on('data', (data: Buffer) => {
                stdout += data.toString();
                onOutput?.(data.toString());
            });
            // Drained so that the command never blocks on a full pipe.
            child.stderr.on('data', (data: Buffer) => this.output().append(data.toString()));
            child.on('error', reject);
            child.on('close', (code) => {
                if (code === 0) {
                    resolve(stdout);
                } else {
                    reject(new Error(`${failure} with exit code ${code}`));
                }
            });
        });
    }

    /** The extension's output channel: the stderr of `devscriptor mcp` and of the CLI. */
    private output(): vscode.OutputChannel {
        if (!this.outputChannel) {
            this.outputChannel = vscode.window.createOutputChannel('Devscriptor');
            this.context.subscriptions.push(this.outputChannel);
        }
        return this.outputChannel;
    }

    public dispose(): void {
        this.client?.close();
        this.client = undefined;
    }
}

/** `read(stdout)`, or `otherwise` when the output cannot be read. */
function readOr<T>(stdout: string, read: (stdout: string) => T, otherwise: T): T {
    try {
        return read(stdout);
    } catch {
        return otherwise;
    }
}
