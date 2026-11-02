/**
 * Readers of the JSON Devscriptor's MCP tools answer with, into the results the
 * CLI parsers of `cliOutput.ts` produce: a command shows the same whether the
 * shared server or the CLI ran it.
 *
 * Kept free of the `vscode` API, like `cliOutput.ts`.
 */

import {
    CallGraph,
    ComplexityResult,
    DeadCodeResult,
    SearchResult,
    complexityRisk,
} from './cliOutput';

/** A field of `answer`, an object, or undefined. */
function field(answer: unknown, name: string): unknown {
    return typeof answer === 'object' && answer !== null
        ? (answer as Record<string, unknown>)[name]
        : undefined;
}

/** The array `name` of `answer`, or none. */
function list(answer: unknown, name: string): Record<string, unknown>[] {
    const value = field(answer, name);
    return Array.isArray(value) ? (value as Record<string, unknown>[]) : [];
}

function text(value: unknown): string {
    return typeof value === 'string' ? value : '';
}

function number(value: unknown): number {
    return typeof value === 'number' ? value : 0;
}

/**
 * `semantic_search`: `{results: [{name, entity_type, file_path, start_line,
 * similarity_score, …}]}`. Lines count from 1, as the CLI's do.
 */
export function searchResults(answer: unknown): SearchResult[] {
    return list(answer, 'results').map((hit) => ({
        name: text(hit.name),
        type: text(hit.entity_type),
        file: text(hit.file_path),
        line: number(hit.start_line),
        score: typeof hit.similarity_score === 'number' ? hit.similarity_score : undefined,
    }));
}

/**
 * `search_entities`: `{entities: [{name, entity_type, file_path, line_number}],
 * total_count}`, best match first. Lines count from 1; a search by name has no
 * score.
 */
export function entitySearchResults(answer: unknown): SearchResult[] {
    return list(answer, 'entities').map((hit) => ({
        name: text(hit.name),
        type: text(hit.entity_type),
        file: text(hit.file_path),
        line: number(hit.line_number),
    }));
}

/**
 * `find_dead_code`: `{unused_functions: [...], unused_classes: [...]}`, each
 * `{name, file_path, line, reason, confidence}`.
 */
export function deadCode(answer: unknown): DeadCodeResult[] {
    return [...list(answer, 'unused_functions'), ...list(answer, 'unused_classes')].map((item) => ({
        name: text(item.name),
        file: text(item.file_path),
        line: number(item.line),
        reason: text(item.reason),
    }));
}

/**
 * `analyze_complexity`: `{most_complex_functions: [{name, file_path, line,
 * complexity_score, risk_level}]}`, most complex first.
 */
export function complexity(answer: unknown): ComplexityResult[] {
    return list(answer, 'most_complex_functions').map((item) => {
        const score = number(item.complexity_score);
        return {
            name: text(item.name),
            file: text(item.file_path),
            line: number(item.line),
            complexity: score,
            risk: complexityRisk(score),
        };
    });
}

/** `get_statistics`: `{total_entities, total_files, total_relationships}`. */
export function entityCount(answer: unknown): number {
    return number(field(answer, 'total_entities'));
}

/**
 * `get_relationships`: `{entity, relationships: [{source, source_type, target,
 * target_type, type}]}`, whose ends are names.
 */
export function relationships(answer: unknown): CallGraph {
    return {
        entity: text(field(answer, 'entity')),
        edges: list(answer, 'relationships').map((edge) => ({
            source: text(edge.source),
            type: text(edge.type),
            target: text(edge.target),
        })),
    };
}

/** `scan_codebase`: `{files_scanned, entities_found, duration_seconds, scanned_path}`. */
export function scanSummary(answer: unknown): string {
    const files = number(field(answer, 'files_scanned'));
    const entities = number(field(answer, 'entities_found'));
    const seconds = number(field(answer, 'duration_seconds'));
    return `${files} files, ${entities} entities in ${seconds.toFixed(1)} s`;
}
