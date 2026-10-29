import * as assert from 'assert';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import {
    checkForUpdate,
    compareVersions,
    parseInstalledVersion,
    parseLatestRelease,
    versionOutput,
} from '../../releases';

// `latest.json` as docs/building.md describes it, shortened.
const LATEST = JSON.stringify({
    tag_name: 'v1.1.0',
    published_at: '2026-10-20T10:00:00Z',
    assets: [{ name: 'devscriptor-1.1.0-x86_64-unknown-linux-gnu.tar.gz', platform: 'linux-x64' }],
});

describe('Release check', () => {
    it('reads the version latest.json names', () => {
        assert.deepStrictEqual(parseLatestRelease(LATEST), {
            version: '1.1.0',
            publishedAt: '2026-10-20T10:00:00Z',
        });
        assert.deepStrictEqual(parseLatestRelease('{"tag_name": "2.0.0"}'), {
            version: '2.0.0',
            publishedAt: undefined,
        });
    });

    it('rejects a latest.json without a version', () => {
        assert.throws(() => parseLatestRelease('{"assets": []}'), /no release version/);
        assert.throws(() => parseLatestRelease('{"tag_name": "nightly"}'), /no release version/);
        assert.throws(() => parseLatestRelease('null'), /no release version/);
        // An HTML error page in place of the file.
        assert.throws(() => parseLatestRelease('<html>Not Found</html>'), SyntaxError);
    });

    it('runs --version', async () => {
        // This process's own binary, which prints its version for --version as
        // Devscriptor does (Node's, or VS Code's under the test host).
        assert.match(await versionOutput(process.execPath), /\d+\.\d+\.\d+/);
    });

    it('rejects a binary that cannot be run', async () => {
        const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dsx-'));
        try {
            await assert.rejects(versionOutput(path.join(dir, 'missing')), { code: 'ENOENT' });
            if (process.platform !== 'win32') {
                const file = path.join(dir, 'devscriptor');
                fs.writeFileSync(file, 'not a program\n', { mode: 0o644 });
                await assert.rejects(versionOutput(file), { code: 'EACCES' });
            }
        } finally {
            fs.rmSync(dir, { recursive: true, force: true });
        }
    });

    it('reads the installed version from --version', () => {
        assert.strictEqual(parseInstalledVersion('devscriptor 1.0.0'), '1.0.0');
        assert.strictEqual(parseInstalledVersion('devscriptor 1.10.0\n'), '1.10.0');
        assert.strictEqual(parseInstalledVersion('devscriptor'), undefined);
    });

    it('compares versions part by part', () => {
        assert.ok(compareVersions('1.10.0', '1.9.2') > 0);
        assert.ok(compareVersions('1.0.0', '1.1.0') < 0);
        assert.strictEqual(compareVersions('1.1', '1.1.0'), 0);
        assert.ok(compareVersions('2.0.0', '1.99.99') > 0);
    });

    it('tells a newer release from the installed one', () => {
        const latest = parseLatestRelease(LATEST);
        assert.deepStrictEqual(checkForUpdate('1.0.0', latest), { kind: 'newer', installed: '1.0.0', latest });
        assert.deepStrictEqual(checkForUpdate('1.1.0', latest), { kind: 'current', installed: '1.1.0', latest });
        // A build newer than the release, such as one from source.
        assert.strictEqual(checkForUpdate('2.0.0', latest).kind, 'current');
        // Nothing installed: the release is offered.
        assert.deepStrictEqual(checkForUpdate(undefined, latest), { kind: 'newer', installed: undefined, latest });
    });

    it('says why the latest release is unknown', () => {
        const error = new Error('https://github.com/vladimirpesic/devscriptor/releases/latest/download/latest.json answered HTTP 404');
        assert.deepStrictEqual(checkForUpdate('1.0.0', error), {
            kind: 'unknown',
            installed: '1.0.0',
            reason: error.message,
        });
    });
});
