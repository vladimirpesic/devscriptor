import * as assert from 'assert';
import {
    complexityRisk,
    parseCallGraph,
    parseComplexity,
    parseDeadCode,
    parseEntityCount,
    parseLocations,
    parseSearchResults,
} from '../../cliOutput';

// Shapes copied from the CLI's real output (`search -o json`, `analyze -o json`,
// `stats`, `definition`, `call-graph -f json`), shortened.
describe('CLI output parsers', () => {
    it('reads search hits', () => {
        const stdout = JSON.stringify([
            {
                name: 'call_tool', type: 'function', file: '/p/unified_server.rs',
                line: 419, end_line: 450, language: 'rust', score: 1,
            },
        ]);
        assert.deepStrictEqual(parseSearchResults(stdout), [
            { name: 'call_tool', type: 'function', file: '/p/unified_server.rs', line: 419, score: 1 },
        ]);
    });

    it('reads dead-code findings out of the analysis object', () => {
        const stdout = JSON.stringify({
            job_id: 'j',
            findings: [{
                title: 'Unused function: shutdown_signal',
                description: 'No incoming calls or references',
                file_path: '/p/server.rs',
                line_number: 139,
                metadata: { entity_name: 'shutdown_signal' },
            }],
            summary: {},
        });
        assert.deepStrictEqual(parseDeadCode(stdout), [{
            name: 'shutdown_signal', file: '/p/server.rs', line: 139,
            reason: 'No incoming calls or references',
        }]);
    });

    it('reads cyclomatic complexity from finding metadata', () => {
        const stdout = JSON.stringify({
            findings: [{
                title: 'Complex function: validate_file_edit',
                description: 'Cyclomatic complexity: 18, Lines: 80',
                file_path: '/p/utility.rs',
                line_number: 20,
                metadata: { entity_name: 'validate_file_edit', cyclomatic: 18, cognitive: 18, lines: 80 },
            }],
        });
        assert.deepStrictEqual(parseComplexity(stdout), [{
            name: 'validate_file_edit', file: '/p/utility.rs', line: 20, complexity: 18, risk: 'medium',
        }]);
    });

    it('rates complexity as analyze_complexity does', () => {
        assert.deepStrictEqual([5, 10, 11, 20, 21].map(complexityRisk), ['low', 'low', 'medium', 'medium', 'high']);
    });

    it('reads definition and reference locations, counted from 0', () => {
        const stdout = JSON.stringify({
            locations: [{
                file_path: '/p/billing.py',
                range: { start: { line: 3, character: 4 }, end: { line: 3, character: 17 } },
                cite: '/p/billing.py:4:5',
            }],
        });
        assert.deepStrictEqual(parseLocations(stdout), [{ file: '/p/billing.py', line: 3, column: 4 }]);
    });

    it('reads the call graph by the names of its ends', () => {
        const stdout = JSON.stringify({
            direction: 'both', entity: 'compute_total', entity_id: 'id-1', entity_type: 'function', max_depth: 3,
            relationships: [{
                source: 'id-1', source_name: 'compute_total', source_type: 'function',
                target: 'id-2', target_name: 'add_tax', target_type: 'function', type: 'calls',
            }],
        });
        assert.deepStrictEqual(parseCallGraph(stdout), {
            entity: 'compute_total',
            edges: [{ source: 'compute_total', type: 'calls', target: 'add_tax' }],
        });
    });

    it('reads an older call graph by its ids', () => {
        const stdout = JSON.stringify({
            entity: 'compute_total',
            relationships: [{ source: 'id-1', target: 'id-2', type: 'calls' }],
        });
        assert.deepStrictEqual(parseCallGraph(stdout).edges, [{ source: 'id-1', type: 'calls', target: 'id-2' }]);
    });

    it('reads the entity count from stats', () => {
        assert.strictEqual(parseEntityCount('{"files":18,"entities":685,"relationships":2078}'), 685);
    });

    it('rejects output that is not JSON', () => {
        assert.throws(() => parseEntityCount('DEVSCRIPTOR v1.3.3\n{"entities":1}'));
    });
});
