/**
 * Parsers for the JSON the `devscriptor` CLI prints on stdout, and the results
 * the extension shows, whether the CLI or an MCP tool (`mcpOutput.ts`) found them.
 *
 * Kept free of the `vscode` API so they can be checked against real CLI output
 * outside the editor. Each takes the command's whole stdout and throws on
 * anything that is not JSON; callers turn that into an empty result.
 */

export interface SearchResult {
    name: string;
    type: string;
    file: string;
    line: number;
    score?: number;
}

export interface DeadCodeResult {
    name: string;
    file: string;
    line: number;
    reason: string;
}

export interface ComplexityResult {
    name: string;
    file: string;
    line: number;
    complexity: number;
    /** `high` above a cyclomatic complexity of 20, `medium` above 10, else `low`. */
    risk: 'high' | 'medium' | 'low';
}

/** A place in a file; `line` counts from 0, as the language server does. */
export interface Reference {
    file: string;
    line: number;
    column?: number;
    context?: string;
}

/** The relationships of one entity: `source`, `type` and `target` of each. */
export interface CallGraph {
    entity: string;
    edges: { source: string; type: string; target: string }[];
}

/** One finding of `analyze --output json`. */
interface Finding {
    title: string;
    description: string;
    file_path?: string | null;
    line_number?: number | null;
    metadata?: Record<string, unknown> | null;
}

/** `search --output json`: an array of `{name, type, file, line, score?}`. */
export function parseSearchResults(stdout: string): SearchResult[] {
    const hits: unknown = JSON.parse(stdout);
    if (!Array.isArray(hits)) {
        return [];
    }
    return hits.map((hit: SearchResult) => ({
        name: hit.name,
        type: hit.type,
        file: hit.file,
        line: hit.line,
        score: hit.score,
    }));
}

/** `analyze --output json` prints `{job_id, findings: [...], summary}`. */
function findings(stdout: string): Finding[] {
    const results = JSON.parse(stdout) as { findings?: unknown };
    return Array.isArray(results?.findings) ? (results.findings as Finding[]) : [];
}

/** The analyzed entity's name, which every analyzer records in `metadata`. */
function entityName(finding: Finding): string {
    const name = finding.metadata?.['entity_name'];
    return typeof name === 'string' ? name : finding.title;
}

/** `analyze --kind dead-code --output json`. */
export function parseDeadCode(stdout: string): DeadCodeResult[] {
    return findings(stdout).map((finding) => ({
        name: entityName(finding),
        file: finding.file_path ?? '',
        line: finding.line_number ?? 0,
        reason: finding.description,
    }));
}

/** The risk the MCP tool `analyze_complexity` gives a cyclomatic complexity. */
export function complexityRisk(complexity: number): ComplexityResult['risk'] {
    if (complexity > 20) {
        return 'high';
    }
    return complexity > 10 ? 'medium' : 'low';
}

/** `analyze --kind complexity --output json`; `complexity` is cyclomatic. */
export function parseComplexity(stdout: string): ComplexityResult[] {
    return findings(stdout).map((finding) => {
        const complexity = Number(finding.metadata?.['cyclomatic'] ?? 0);
        return {
            name: entityName(finding),
            file: finding.file_path ?? '',
            line: finding.line_number ?? 0,
            complexity,
            risk: complexityRisk(complexity),
        };
    });
}

/** `stats` prints `{"files": N, "entities": N, "relationships": N}`. */
export function parseEntityCount(stdout: string): number {
    const stats = JSON.parse(stdout) as { entities?: unknown };
    return typeof stats?.entities === 'number' ? stats.entities : 0;
}

/**
 * `{locations: [{file_path, range: {start: {line, character}}}]}`: what both the
 * CLI's `definition` and `references` and the MCP tools `get_definition` and
 * `get_references` answer.
 */
export function toReferences(answer: unknown): Reference[] {
    const locations = (answer as { locations?: unknown } | null)?.locations;
    if (!Array.isArray(locations)) {
        return [];
    }
    return locations.map((location: LspLocation) => ({
        file: location.file_path,
        line: location.range?.start?.line ?? 0,
        column: location.range?.start?.character,
    }));
}

/** `definition --line … --character …` and `references …`, on stdout. */
export function parseLocations(stdout: string): Reference[] {
    return toReferences(JSON.parse(stdout));
}

/**
 * `call-graph --entity <name> --format json`: `{entity, relationships: [{source,
 * source_name, target, target_name, type}]}`. The ends are read by name; a CLI
 * older than the names gives their ids only.
 */
export function parseCallGraph(stdout: string): CallGraph {
    const graph = JSON.parse(stdout) as {
        entity?: unknown;
        relationships?: {
            source: string;
            source_name?: string;
            target: string;
            target_name?: string;
            type: string;
        }[];
    };
    return {
        entity: typeof graph?.entity === 'string' ? graph.entity : '',
        edges: Array.isArray(graph?.relationships)
            ? graph.relationships.map((edge) => ({
                source: edge.source_name ?? edge.source,
                type: edge.type,
                target: edge.target_name ?? edge.target,
            }))
            : [],
    };
}

/** A location as the language server gives it. */
interface LspLocation {
    file_path: string;
    range?: { start?: { line?: number; character?: number } };
}
