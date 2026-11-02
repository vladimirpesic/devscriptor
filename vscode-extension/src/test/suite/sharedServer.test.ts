import * as assert from 'assert';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { McpClient, mcpSpawnSpec } from '../../mcpClient';
import { entityCount } from '../../mcpOutput';

/**
 * The extension's client against a real `devscriptor`, when `DEVSCRIPTOR_BIN`
 * names one that has the shared process: the extension and another client in one
 * workspace are served by one Devscriptor process. The test works in a temporary
 * workspace, with runtime, data and configuration directories of its own, and
 * stops the daemon it started.
 */
describe('The shared Devscriptor', function () {
    this.timeout(120000);
    // Resolved where the tests start: the clients run in the temporary workspace.
    const binary = process.env.DEVSCRIPTOR_BIN && path.resolve(process.env.DEVSCRIPTOR_BIN);
    let dir = '';
    let workspace = '';
    let env: NodeJS.ProcessEnv = {};
    const clients: McpClient[] = [];
    let daemon: number | undefined;

    before(function () {
        if (!binary) {
            this.skip();
        }
        dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dsx-'));
        workspace = path.join(dir, 'ws');
        fs.mkdirSync(workspace);
        fs.writeFileSync(
            path.join(workspace, 'billing.py'),
            'def compute_total(items):\n    return add_tax(sum(items))\n\n\ndef add_tax(amount):\n    return amount * 1.2\n',
        );
        env = {
            ...process.env,
            // A socket path has room for about 100 bytes: the temporary
            // directory keeps the runtime directory's path short.
            XDG_RUNTIME_DIR: path.join(dir, 'run'),
            XDG_DATA_HOME: path.join(dir, 'data'),
            XDG_CONFIG_HOME: path.join(dir, 'config'),
            XDG_CACHE_HOME: path.join(dir, 'cache'),
            // Gone soon even if the test cannot stop it.
            DEVSCRIPTOR_DAEMON_IDLE_SECS: '30',
        };
    });

    after(async () => {
        for (const client of clients) {
            client.close();
        }
        if (daemon !== undefined) {
            await stop(daemon);
        }
        if (dir) {
            fs.rmSync(dir, { recursive: true, force: true });
        }
    });

    /** A client of the workspace's Devscriptor, as the extension connects. */
    function connect(): McpClient {
        const { command, args } = mcpSpawnSpec(binary as string, path.join(workspace, 'devscriptor.db'));
        const client = new McpClient({ command, args, cwd: workspace, env }, '0.0.0');
        clients.push(client);
        return client;
    }

    it('serves the extension and another client from one process', async () => {
        const extension = connect();
        await extension.callTool('scan_codebase', { path: workspace });
        const served = (await extension.callTool('health_check', {})) as { daemon_pid?: number };
        assert.ok(served.daemon_pid, 'a daemon serves the extension');
        daemon = served.daemon_pid;

        const agent = connect();
        const alsoServed = (await agent.callTool('health_check', {})) as { daemon_pid?: number };
        assert.strictEqual(alsoServed.daemon_pid, daemon, 'the same daemon serves the other client');
        // The other client sees the index the extension's scan built.
        assert.ok(entityCount(await agent.callTool('get_statistics', {})) > 0);
    });
});

/** Stop process `pid`, and wait until it is gone. */
async function stop(pid: number): Promise<void> {
    try {
        process.kill(pid);
    } catch {
        return;
    }
    for (let attempt = 0; attempt < 100; attempt++) {
        try {
            process.kill(pid, 0);
        } catch {
            return;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
    }
}
