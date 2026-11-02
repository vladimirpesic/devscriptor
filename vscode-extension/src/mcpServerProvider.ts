import * as vscode from 'vscode';
import { DevscriptorManager } from './devscriptorManager';

/** The provider `contributes.mcpServerDefinitionProviders` declares in package.json. */
const PROVIDER_ID = 'devscriptor.mcp';

/** The setting that offers the server. */
const SETTING = 'devscriptor.mcpServer.enabled';

/** The settings the offered server is made of. */
const SERVER_SETTINGS = [SETTING, 'devscriptor.binaryPath', 'devscriptor.databasePath'];

function enabled(): boolean {
    return vscode.workspace.getConfiguration('devscriptor').get<boolean>('mcpServer.enabled', false);
}

/**
 * Offer the workspace's Devscriptor to VS Code's AI assistants (agent mode in
 * chat) as an MCP server while `devscriptor.mcpServer.enabled` is on:
 * `devscriptor mcp` in the first workspace folder with the extension's
 * database, which relays to the workspace's shared Devscriptor as the
 * extension's own connection does.
 *
 * VS Code has the MCP API from 1.101 on. Turned on where it cannot be offered,
 * on an older VS Code or without an installed Devscriptor, the setting says
 * why.
 */
export function registerMcpServerProvider(context: vscode.ExtensionContext, manager: DevscriptorManager): void {
    // Neither `vscode.lm` nor its MCP API exist before VS Code 1.101.
    const lm = vscode.lm as typeof vscode.lm | undefined;
    if (typeof lm?.registerMcpServerDefinitionProvider !== 'function') {
        const warn = () => {
            if (enabled()) {
                void vscode.window.showWarningMessage(
                    `${SETTING} needs VS Code 1.101 or newer, which can offer MCP servers to its AI ` +
                    `assistants; this is VS Code ${vscode.version}.`
                );
            }
        };
        warn();
        context.subscriptions.push(vscode.workspace.onDidChangeConfiguration((event) => {
            if (event.affectsConfiguration(SETTING)) {
                warn();
            }
        }));
        return;
    }

    const changed = new vscode.EventEmitter<void>();
    context.subscriptions.push(
        changed,
        lm.registerMcpServerDefinitionProvider(PROVIDER_ID, {
            onDidChangeMcpServerDefinitions: changed.event,
            provideMcpServerDefinitions: async () => {
                if (!enabled()) {
                    return [];
                }
                try {
                    const server = await manager.mcpServer();
                    const definition = new vscode.McpStdioServerDefinition(
                        'Devscriptor',
                        server.command,
                        server.args,
                        {},
                        // A new version makes VS Code read the tools again.
                        server.version,
                    );
                    definition.cwd = vscode.Uri.file(server.cwd);
                    return [definition];
                } catch {
                    // Said when the setting was turned on.
                    return [];
                }
            },
        }),
        vscode.workspace.onDidChangeConfiguration(async (event) => {
            if (!SERVER_SETTINGS.some((setting) => event.affectsConfiguration(setting))) {
                return;
            }
            changed.fire();
            if (event.affectsConfiguration(SETTING) && enabled()) {
                try {
                    await manager.mcpServer();
                } catch (error) {
                    void vscode.window.showWarningMessage(
                        `Devscriptor cannot be offered to AI assistants: ${error instanceof Error ? error.message : error}.`
                    );
                }
            }
        }),
        vscode.workspace.onDidChangeWorkspaceFolders(() => changed.fire()),
    );
}
