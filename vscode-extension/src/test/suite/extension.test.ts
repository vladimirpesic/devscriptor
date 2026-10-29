import * as assert from 'assert';
import * as vscode from 'vscode';
import { mcpSpawnSpec } from '../../mcpClient';

const EXT_ID = 'vladimirpesic.devscriptor';

const EXPECTED_COMMANDS = [
    'devscriptor.scanWorkspace',
    'devscriptor.semanticSearch',
    'devscriptor.findDeadCode',
    'devscriptor.analyzeComplexity',
    'devscriptor.showCallGraph',
    'devscriptor.gotoDefinition',
    'devscriptor.findReferences',
    'devscriptor.startMcpServer',
    'devscriptor.stopMcpServer',
    'devscriptor.updateBinary',
    'devscriptor.showStatus',
];

describe('Devscriptor Extension Smoke Tests', function () {
    this.timeout(30000);

    let ext: vscode.Extension<unknown>;

    before(async () => {
        const found = vscode.extensions.getExtension(EXT_ID);
        assert.ok(found, `Extension ${EXT_ID} must be installed in the test instance`);
        ext = found;
        if (!ext.isActive) {
            await ext.activate();
        }
    });

    it('extension is present and activates', () => {
        assert.ok(ext, 'Extension should be registered');
        assert.ok(ext.isActive, 'Extension should be active after activate()');
    });

    it('all expected commands are registered', async () => {
        const all = await vscode.commands.getCommands(true);
        const missing = EXPECTED_COMMANDS.filter(cmd => !all.includes(cmd));
        assert.deepStrictEqual(
            missing,
            [],
            `Missing commands: ${missing.join(', ')}`
        );
    });

    it('stopMcpServer does not throw when server was never started', async () => {
        // Verify the stop path is safe to call even with no running MCP process.
        await assert.doesNotReject(async () => {
            await vscode.commands.executeCommand('devscriptor.stopMcpServer');
        });
    });

    it('startMcpServer runs the main binary with `mcp` and the database on every platform', () => {
        // One binary name on every platform: a name derived with
        //   mcpBinary = mainBinary.replace(/devscriptor$/, 'devscriptor-mcp')
        // would not match `devscriptor.exe`, and on Windows the extension would
        // spawn the CLI with no arguments instead of the MCP server.
        const binaries = [
            '/usr/local/bin/devscriptor',
            '/home/user/.cargo/bin/devscriptor',
            '/home/user/.local/bin/devscriptor',
            'C:\\Program Files\\devscriptor\\devscriptor.exe',
            'C:\\Users\\u\\AppData\\Local\\Programs\\devscriptor\\devscriptor.exe',
        ];

        for (const binary of binaries) {
            const spec = mcpSpawnSpec(binary, '/w/devscriptor.db');
            assert.strictEqual(spec.command, binary, `must spawn the binary itself: ${binary}`);
            assert.deepStrictEqual(
                spec.args,
                ['mcp', '--database', '/w/devscriptor.db'],
                `must pass the mcp subcommand and the database: ${binary}`
            );
            assert.ok(
                !spec.command.includes('devscriptor-mcp'),
                `must not derive a devscriptor-mcp name: ${binary}`
            );
        }
    });
});
