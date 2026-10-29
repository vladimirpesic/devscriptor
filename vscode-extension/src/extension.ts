import * as vscode from 'vscode';
import { CallGraph, DevscriptorManager, SearchResult, Reference as ReferenceResult, DeadCodeResult, ComplexityResult } from './devscriptorManager';
import { AnalysisProvider } from './analysisProvider';
import { registerMcpServerProvider } from './mcpServerProvider';
import { checkForUpdate, DOWNLOAD_PAGE_URL, fetchLatestRelease } from './releases';

let manager: DevscriptorManager;
let statusBarItem: vscode.StatusBarItem;

export function activate(context: vscode.ExtensionContext) {
    console.log('Devscriptor extension is now active');

    // Initialize manager
    manager = new DevscriptorManager(context);

    // Create status bar item
    statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    statusBarItem.command = 'devscriptor.showStatus';
    context.subscriptions.push(statusBarItem);

    // Register commands
    context.subscriptions.push(
        vscode.commands.registerCommand('devscriptor.scanWorkspace', scanWorkspace),
        vscode.commands.registerCommand('devscriptor.semanticSearch', search),
        vscode.commands.registerCommand('devscriptor.findDeadCode', findDeadCode),
        vscode.commands.registerCommand('devscriptor.analyzeComplexity', analyzeComplexity),
        vscode.commands.registerCommand('devscriptor.showCallGraph', showCallGraph),
        vscode.commands.registerCommand('devscriptor.gotoDefinition', gotoDefinition),
        vscode.commands.registerCommand('devscriptor.findReferences', findReferences),
        vscode.commands.registerCommand('devscriptor.startMcpServer', startMcpServer),
        vscode.commands.registerCommand('devscriptor.stopMcpServer', stopMcpServer),
        vscode.commands.registerCommand('devscriptor.updateBinary', updateBinary),
        vscode.commands.registerCommand('devscriptor.showStatus', showStatus)
    );

    // The Devscriptor Analysis view in the Explorer: its rows come from the
    // provider, and it shows only once the `devscriptor.enabled` context key
    // its `when` clause names is set.
    context.subscriptions.push(
        vscode.window.registerTreeDataProvider('devscriptor.analysis', new AnalysisProvider())
    );
    void vscode.commands.executeCommand('setContext', 'devscriptor.enabled', true);

    registerMcpServerProvider(context, manager);

    if (setting('autoScan', true)) {
        scanWorkspace();
    }

    updateStatusBar();
}

export function deactivate() {
    if (manager) {
        manager.dispose();
    }
}

async function scanWorkspace(): Promise<void> {
    const workspaceFolders = vscode.workspace.workspaceFolders;
    if (!workspaceFolders) {
        vscode.window.showWarningMessage('No workspace folder open');
        return;
    }

    const rootPath = workspaceFolders[0].uri.fsPath;

    await vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: 'Scanning workspace with Devscriptor...',
        cancellable: false
    }, async (progress) => {
        try {
            await manager.scanWorkspace(rootPath, (message: string) => {
                progress.report({ message });
            });
            vscode.window.showInformationMessage('Workspace scan complete!');
            updateStatusBar();
        } catch (error) {
            vscode.window.showErrorMessage(`Scan failed: ${error}`);
        }
    });
}

/** A setting of the extension, as the user set it or its default. */
function setting<T>(name: string, otherwise: T): T {
    return vscode.workspace.getConfiguration('devscriptor').get<T>(name, otherwise);
}

/**
 * Search the workspace: by meaning, or by name while
 * `devscriptor.enableSemanticSearch` is off.
 */
async function search(): Promise<void> {
    const semantic = setting('enableSemanticSearch', true);
    const query = await vscode.window.showInputBox(semantic
        ? {
            prompt: 'Search by meaning (natural language supported)',
            placeHolder: 'e.g., "authentication logic" or "find all API endpoints"'
        }
        : {
            prompt: 'Search by name (semantic search is off: devscriptor.enableSemanticSearch)',
            placeHolder: 'e.g., "parse_config" or "UserService"'
        });

    if (!query) {
        return;
    }

    let results: SearchResult[];
    try {
        results = await manager.search(query, semantic);
    } catch (error) {
        vscode.window.showErrorMessage(`Search failed: ${error}`);
        return;
    }

    if (results.length === 0) {
        vscode.window.showInformationMessage('No results found');
        return;
    }

    // Show results in quick pick
    interface QuickPickItemWithResult extends vscode.QuickPickItem {
        result: SearchResult;
    }

    const items: QuickPickItemWithResult[] = results.map((r: SearchResult) => ({
        label: r.name,
        description: `${r.type} in ${r.file}:${r.line}`,
        detail: r.score ? `Relevance: ${(r.score * 100).toFixed(1)}%` : undefined,
        result: r
    }));

    const selected = await vscode.window.showQuickPick(items, {
        placeHolder: `Found ${results.length} results for "${query}"`
    });

    if (selected) {
        const doc = await vscode.workspace.openTextDocument(selected.result.file);
        const editor = await vscode.window.showTextDocument(doc);
        const position = new vscode.Position(selected.result.line - 1, 0);
        editor.selection = new vscode.Selection(position, position);
        editor.revealRange(new vscode.Range(position, position));
    }
}

async function findDeadCode(): Promise<void> {
    await vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: 'Finding dead code...',
        cancellable: false
    }, async () => {
        try {
            const results = await manager.findDeadCode();
            if (results.length === 0) {
                vscode.window.showInformationMessage('No dead code found!');
                return;
            }

            interface QuickPickItemWithResult extends vscode.QuickPickItem {
                result: DeadCodeResult;
            }

            // Show results
            const items: QuickPickItemWithResult[] = results.map((r: DeadCodeResult) => ({
                label: r.name,
                description: r.file,
                detail: r.reason,
                result: r
            }));

            const selected = await vscode.window.showQuickPick(items, {
                placeHolder: `Found ${results.length} potentially unused items`
            });

            if (selected) {
                const doc = await vscode.workspace.openTextDocument(selected.result.file);
                const editor = await vscode.window.showTextDocument(doc);
                const position = new vscode.Position(selected.result.line - 1, 0);
                editor.selection = new vscode.Selection(position, position);
                editor.revealRange(new vscode.Range(position, position));
            }
        } catch (error) {
            vscode.window.showErrorMessage(`Analysis failed: ${error}`);
        }
    });
}

async function analyzeComplexity(): Promise<void> {
    try {
        const results = await manager.analyzeComplexity();

        // Show results in a webview panel
        const panel = vscode.window.createWebviewPanel(
            'devscriptorComplexity',
            'Complexity Analysis',
            vscode.ViewColumn.One,
            {}
        );

        panel.webview.html = generateComplexityHtml(results);
    } catch (error) {
        vscode.window.showErrorMessage(`Complexity analysis failed: ${error}`);
    }
}

function escapeHtml(value: unknown): string {
    return String(value)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

function generateComplexityHtml(results: ComplexityResult[]): string {
    const rows = results.map(r => `
        <tr>
            <td>${escapeHtml(r.name)}</td>
            <td>${escapeHtml(r.file)}:${escapeHtml(r.line)}</td>
            <td>${escapeHtml(r.complexity)}</td>
            <td>${escapeHtml(r.risk)}</td>
        </tr>
    `).join('');

    return `
        <!DOCTYPE html>
        <html>
        <head>
            <style>
                table { width: 100%; border-collapse: collapse; }
                th, td { padding: 8px; text-align: left; border-bottom: 1px solid #ddd; }
                th { background-color: #4CAF50; color: white; }
                tr:hover { background-color: #f5f5f5; }
                .high { color: red; }
                .medium { color: orange; }
                .low { color: green; }
            </style>
        </head>
        <body>
            <h1>Code Complexity Analysis</h1>
            <table>
                <tr>
                    <th>Function/Method</th>
                    <th>Location</th>
                    <th>Complexity</th>
                    <th>Risk</th>
                </tr>
                ${rows}
            </table>
        </body>
        </html>
    `;
}

async function showCallGraph(): Promise<void> {
    const symbol = await vscode.window.showInputBox({
        prompt: 'Enter function name to visualize call graph',
        placeHolder: 'e.g., "main" or "processRequest"'
    });

    if (!symbol) {
        return;
    }

    try {
        const graph = await manager.getCallGraph(symbol);

        const panel = vscode.window.createWebviewPanel(
            'devscriptorCallGraph',
            `Call Graph: ${symbol}`,
            vscode.ViewColumn.One,
            {}
        );

        panel.webview.html = generateCallGraphHtml(graph, symbol);
    } catch (error) {
        vscode.window.showErrorMessage(`Failed to generate call graph: ${error}`);
    }
}

/** The relationships of the entity, one row each: its source, kind and target. */
function generateCallGraphHtml(graph: CallGraph, symbol: string): string {
    const rows = graph.edges.map(edge => `
        <tr>
            <td>${escapeHtml(edge.source)}</td>
            <td>${escapeHtml(edge.type)}</td>
            <td>${escapeHtml(edge.target)}</td>
        </tr>
    `).join('');
    const body = graph.edges.length === 0
        ? `<p>No relationships found for ${escapeHtml(symbol)}.</p>`
        : `<table>
                <tr>
                    <th>Source</th>
                    <th>Relationship</th>
                    <th>Target</th>
                </tr>
                ${rows}
            </table>`;

    return `
        <!DOCTYPE html>
        <html>
        <head>
            <style>
                table { width: 100%; border-collapse: collapse; }
                th, td { padding: 8px; text-align: left; border-bottom: 1px solid #ddd; }
                th { background-color: #4CAF50; color: white; }
            </style>
        </head>
        <body>
            <h1>Call Graph: ${escapeHtml(graph.entity || symbol)}</h1>
            ${body}
        </body>
        </html>
    `;
}

/**
 * Whether the enhanced navigation is on (`devscriptor.lsp.enabled`); says how
 * to turn it on when it is off. Its commands are hidden then, but a
 * keybinding or another extension can still run them.
 */
async function enhancedNavigation(): Promise<boolean> {
    if (setting('lsp.enabled', true)) {
        return true;
    }
    const open = 'Open Settings';
    const action = await vscode.window.showInformationMessage(
        'Enhanced navigation is off (devscriptor.lsp.enabled).',
        open
    );
    if (action === open) {
        await vscode.commands.executeCommand('workbench.action.openSettings', 'devscriptor.lsp.enabled');
    }
    return false;
}

async function gotoDefinition(): Promise<void> {
    if (!await enhancedNavigation()) {
        return;
    }
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
        vscode.window.showWarningMessage('No active editor');
        return;
    }

    // The file path and the cursor position: the `definition` subcommand asks
    // the live LSP server.
    const position = editor.selection.active;
    const filePath = editor.document.uri.fsPath;

    try {
        const location = await manager.getDefinition(filePath, position.line, position.character);

        if (location) {
            const doc = await vscode.workspace.openTextDocument(location.file);
            const targetEditor = await vscode.window.showTextDocument(doc);
            const pos = new vscode.Position(Math.max(0, location.line), location.column ?? 0);
            targetEditor.selection = new vscode.Selection(pos, pos);
            targetEditor.revealRange(new vscode.Range(pos, pos));
        } else {
            vscode.window.showInformationMessage('No definition found at cursor position');
        }
    } catch (error) {
        vscode.window.showErrorMessage(`Failed to find definition: ${error}`);
    }
}

async function findReferences(): Promise<void> {
    if (!await enhancedNavigation()) {
        return;
    }
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
        vscode.window.showWarningMessage('No active editor');
        return;
    }

    // The file path and the cursor position.
    const position = editor.selection.active;
    const filePath = editor.document.uri.fsPath;

    try {
        const references = await manager.getReferences(filePath, position.line, position.character);

        if (references.length === 0) {
            vscode.window.showInformationMessage('No references found at cursor position');
            return;
        }

        interface QuickPickItemWithRef extends vscode.QuickPickItem {
            reference: ReferenceResult;
        }

        const items: QuickPickItemWithRef[] = references.map((r: ReferenceResult) => ({
            label: r.file,
            description: `Line ${r.line}`,
            detail: r.context,
            reference: r
        }));

        const selected = await vscode.window.showQuickPick(items, {
            placeHolder: `Found ${references.length} references`
        });

        if (selected) {
            const doc = await vscode.workspace.openTextDocument(selected.reference.file);
            const targetEditor = await vscode.window.showTextDocument(doc);
            const pos = new vscode.Position(Math.max(0, selected.reference.line), selected.reference.column ?? 0);
            targetEditor.selection = new vscode.Selection(pos, pos);
            targetEditor.revealRange(new vscode.Range(pos, pos));
        }
    } catch (error) {
        vscode.window.showErrorMessage(`Failed to find references: ${error}`);
    }
}

async function startMcpServer(): Promise<void> {
    try {
        await manager.startMcpServer();
        vscode.window.showInformationMessage("Connected to the workspace's Devscriptor");
        updateStatusBar();
    } catch (error) {
        vscode.window.showErrorMessage(`Failed to connect to the workspace's Devscriptor: ${error}`);
    }
}

async function stopMcpServer(): Promise<void> {
    try {
        await manager.stopMcpServer();
        vscode.window.showInformationMessage("Disconnected from the workspace's Devscriptor");
        updateStatusBar();
    } catch (error) {
        vscode.window.showErrorMessage(`Failed to disconnect: ${error}`);
    }
}

/**
 * Compare the installed Devscriptor with the newest release, and offer its
 * download page. Devscriptor does not update itself: a release is installed
 * over the old one, and the next connection replaces a daemon of the old
 * version.
 */
async function updateBinary(): Promise<void> {
    const [installed, latest] = await vscode.window.withProgress({
        location: vscode.ProgressLocation.Notification,
        title: 'Checking for a Devscriptor update...',
        cancellable: false
    }, () => Promise.all([manager.installedVersion(), fetchLatestRelease()]));
    const check = checkForUpdate(installed, latest);
    const open = 'Open Download Page';
    let action: string | undefined;
    switch (check.kind) {
        case 'current':
            action = await vscode.window.showInformationMessage(
                `Devscriptor ${check.installed} is the latest release.`
            );
            break;
        case 'newer':
            action = await vscode.window.showInformationMessage(
                check.installed === undefined
                    ? `Devscriptor ${check.latest.version} is the latest release; no Devscriptor is installed here.`
                    : `Devscriptor ${check.latest.version} is out (installed: ${check.installed}). ` +
                      'Install it over this one from the download page.',
                open
            );
            break;
        case 'unknown':
            action = await vscode.window.showWarningMessage(
                `Could not tell the latest Devscriptor release (${check.reason}).`,
                open
            );
            break;
    }
    if (action === open) {
        await vscode.env.openExternal(vscode.Uri.parse(DOWNLOAD_PAGE_URL));
    }
}

async function showStatus(): Promise<void> {
    const status = await manager.getStatus();

    const items: vscode.QuickPickItem[] = [
        {
            label: `$(info) Binary Version`,
            description: status.binaryVersion || 'Not available'
        },
        {
            label: `$(database) Database`,
            description: status.databaseExists ? 'Exists' : 'Not created'
        },
        {
            label: `$(symbol-misc) Entities`,
            description: status.entityCount ? status.entityCount.toString() : '0'
        },
        {
            label: `$(server) Shared Devscriptor`,
            description: status.mcpRunning ? 'Connected' : 'Not connected'
        }
    ];

    await vscode.window.showQuickPick(items, {
        placeHolder: 'Devscriptor Status'
    });

    // Item selected - could add actions here
}

function updateStatusBar(): void {
    manager.getStatus().then((status: { mcpRunning: boolean; databaseExists: boolean; entityCount?: number }) => {
        if (status.mcpRunning) {
            statusBarItem.text = '$(server) Devscriptor';
            statusBarItem.tooltip = "Devscriptor: connected to the workspace's shared server";
        } else if (status.databaseExists) {
            statusBarItem.text = '$(database) Devscriptor';
            statusBarItem.tooltip = `Devscriptor: ${status.entityCount} entities indexed`;
        } else {
            statusBarItem.text = '$(symbol-misc) Devscriptor';
            statusBarItem.tooltip = 'Devscriptor: No database';
        }
        statusBarItem.show();
    });
}
