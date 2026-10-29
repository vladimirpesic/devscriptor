import { spawn } from 'child_process';
import * as https from 'https';

/**
 * Where Devscriptor's releases are announced and downloaded: its GitHub
 * releases. Each publishes `latest.json` beside the archives of every
 * platform and their checksums (docs/building.md, "After the builds"); GitHub
 * answers the latest one's with a redirect, which `fetchText` follows.
 */
export const LATEST_RELEASE_URL = 'https://github.com/vladimirpesic/devscriptor/releases/latest/download/latest.json';
export const DOWNLOAD_PAGE_URL = 'https://github.com/vladimirpesic/devscriptor/releases/latest';

/** The newest release, as `latest.json` names it. */
export interface LatestRelease {
    /** Without the leading `v` of its tag: `1.0.0`. */
    version: string;
    /** When it was published (ISO 8601), if `latest.json` says. */
    publishedAt?: string;
}

/**
 * Read `latest.json`: `{"tag_name": "v1.0.0", "published_at": "…", "assets": […]}`.
 *
 * @throws if the text is no such object.
 */
export function parseLatestRelease(text: string): LatestRelease {
    const parsed: unknown = JSON.parse(text);
    const tag = (parsed as { tag_name?: unknown })?.tag_name;
    if (typeof tag !== 'string' || !/^v?\d+(\.\d+)*$/.test(tag.trim())) {
        throw new Error('latest.json names no release version');
    }
    const publishedAt = (parsed as { published_at?: unknown }).published_at;
    return {
        version: tag.trim().replace(/^v/, ''),
        publishedAt: typeof publishedAt === 'string' ? publishedAt : undefined,
    };
}

/**
 * What `<binary> --version` prints, trimmed. Rejects when the binary cannot be
 * run (missing, not executable, not a program) or exits with an error.
 */
export function versionOutput(binary: string): Promise<string> {
    return new Promise((resolve, reject) => {
        const child = spawn(binary, ['--version']);
        let output = '';
        child.stdout.on('data', (data: Buffer) => {
            output += data.toString();
        });
        // Without it, a binary that cannot be run would throw in the
        // extension host, and the promise would never settle.
        child.on('error', reject);
        child.on('close', (code) => {
            if (code === 0) {
                resolve(output.trim());
            } else {
                reject(new Error(`${binary} --version exited with code ${code}`));
            }
        });
    });
}

/** The version `devscriptor --version` prints (`devscriptor 1.0.0`), if any. */
export function parseInstalledVersion(output: string): string | undefined {
    return /(\d+(?:\.\d+)+)/.exec(output)?.[1];
}

/**
 * Compare two dotted versions, numerically part by part (`1.10.0` is newer
 * than `1.9.2`); a missing part counts as 0. Negative when `a` is older.
 */
export function compareVersions(a: string, b: string): number {
    const parts = (version: string) => version.split('.').map((part) => parseInt(part, 10) || 0);
    const [left, right] = [parts(a), parts(b)];
    for (let i = 0; i < Math.max(left.length, right.length); i++) {
        const difference = (left[i] ?? 0) - (right[i] ?? 0);
        if (difference !== 0) {
            return difference;
        }
    }
    return 0;
}

/** What a check for a newer release found. */
export type UpdateCheck =
    | { kind: 'newer'; installed: string | undefined; latest: LatestRelease }
    | { kind: 'current'; installed: string; latest: LatestRelease }
    | { kind: 'unknown'; installed: string | undefined; reason: string };

/** Compare the installed version (if Devscriptor is installed) with the newest release. */
export function checkForUpdate(
    installed: string | undefined,
    latest: LatestRelease | Error,
): UpdateCheck {
    if (latest instanceof Error) {
        return { kind: 'unknown', installed, reason: latest.message };
    }
    if (installed !== undefined && compareVersions(installed, latest.version) >= 0) {
        return { kind: 'current', installed, latest };
    }
    return { kind: 'newer', installed, latest };
}

/**
 * Fetch `url` over HTTPS as text, following redirects, within `timeoutMs`.
 *
 * Node's `https` rather than `fetch`: the extension runs on VS Code 1.74,
 * whose Node has no `fetch`.
 */
export function fetchText(url: string, timeoutMs = 10_000, redirects = 3): Promise<string> {
    return new Promise((resolve, reject) => {
        const request = https.get(url, { timeout: timeoutMs }, (response) => {
            const status = response.statusCode ?? 0;
            const location = response.headers.location;
            if (status >= 300 && status < 400 && location && redirects > 0) {
                response.resume();
                resolve(fetchText(new URL(location, url).toString(), timeoutMs, redirects - 1));
                return;
            }
            if (status !== 200) {
                response.resume();
                reject(new Error(`${url} answered HTTP ${status}`));
                return;
            }
            let body = '';
            response.setEncoding('utf8');
            response.on('data', (chunk: string) => {
                body += chunk;
            });
            response.on('end', () => resolve(body));
            response.on('error', reject);
        });
        request.on('timeout', () => request.destroy(new Error(`${url} did not answer within ${timeoutMs} ms`)));
        request.on('error', reject);
    });
}

/** The newest release, or why it could not be told. */
export async function fetchLatestRelease(): Promise<LatestRelease | Error> {
    try {
        return parseLatestRelease(await fetchText(LATEST_RELEASE_URL));
    } catch (error) {
        return error instanceof Error ? error : new Error(String(error));
    }
}
