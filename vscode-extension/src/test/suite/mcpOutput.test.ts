import * as assert from 'assert';
import { toReferences } from '../../cliOutput';
import {
    complexity,
    deadCode,
    entityCount,
    entitySearchResults,
    relationships,
    scanSummary,
    searchResults,
} from '../../mcpOutput';

// Shapes copied from the answers of Devscriptor's MCP tools, shortened.
describe('MCP tool answers', () => {
    it('reads semantic search hits as the CLI search reports them', () => {
        const answer = {
            entity_type: '',
            query: 'tax on a total',
            results: [{
                body_preview: '10\tdef add_tax(amount):\n11\t    return amount * 1.2',
                end_line: 14,
                entity_type: 'function',
                fallback: false,
                file_path: '/w/pkg/billing.py',
                language: 'python',
                name: 'add_tax',
                similarity_score: 1.0,
                start_line: 10,
            }],
            service_available: true,
        };
        assert.deepStrictEqual(searchResults(answer), [
            { name: 'add_tax', type: 'function', file: '/w/pkg/billing.py', line: 10, score: 1.0 },
        ]);
    });

    it('reads search-by-name hits as the CLI symbol search reports them', () => {
        const answer = {
            entities: [
                { entity_type: 'function', file_path: '/w/pkg/billing.py', line_number: 10, name: 'add_tax' },
                { entity_type: 'class', file_path: '/w/pkg/tax.py', line_number: 3, name: 'TaxTable' },
            ],
            total_count: 2,
        };
        assert.deepStrictEqual(entitySearchResults(answer), [
            { name: 'add_tax', type: 'function', file: '/w/pkg/billing.py', line: 10 },
            { name: 'TaxTable', type: 'class', file: '/w/pkg/tax.py', line: 3 },
        ]);
        assert.deepStrictEqual(entitySearchResults({ error: 'x' }), []);
    });

    it('reads unused functions and classes', () => {
        const answer = {
            total_unused_classes: 1,
            total_unused_functions: 1,
            unused_classes: [{
                confidence: 0.5, file_path: '/w/a.py', line: 3, name: 'Old', reason: 'No incoming references',
            }],
            unused_functions: [{
                confidence: 0.9, file_path: '/w/pkg/billing.py', line: 17, name: 'unused_helper',
                reason: 'No incoming calls or references',
            }],
        };
        assert.deepStrictEqual(deadCode(answer), [
            { name: 'unused_helper', file: '/w/pkg/billing.py', line: 17, reason: 'No incoming calls or references' },
            { name: 'Old', file: '/w/a.py', line: 3, reason: 'No incoming references' },
        ]);
    });

    it('reads the most complex functions with their risk', () => {
        const answer = {
            average_complexity: 9.5,
            most_complex_functions: [{
                complexity_score: 18, file_path: '/w/utility.rs', line: 20, name: 'validate_file_edit',
                risk_level: 'medium',
            }],
            total_functions_analyzed: 4,
        };
        assert.deepStrictEqual(complexity(answer), [
            { name: 'validate_file_edit', file: '/w/utility.rs', line: 20, complexity: 18, risk: 'medium' },
        ]);
    });

    it('reads the entity count from the statistics', () => {
        assert.strictEqual(entityCount({ total_entities: 7, total_files: 3, total_relationships: 16 }), 7);
        assert.strictEqual(entityCount({}), 0);
    });

    it("reads an entity's relationships by name", () => {
        const answer = {
            entity: 'compute_total',
            entity_type: 'any',
            relationships: [
                { source: 'compute_total', source_type: 'function', target: 'add_tax', target_type: 'function', type: 'calls' },
                { source: 'main', source_type: 'function', target: 'compute_total', target_type: 'function', type: 'calls' },
            ],
            total_relationships: 2,
        };
        assert.deepStrictEqual(relationships(answer), {
            entity: 'compute_total',
            edges: [
                { source: 'compute_total', type: 'calls', target: 'add_tax' },
                { source: 'main', type: 'calls', target: 'compute_total' },
            ],
        });
    });

    it('reads definition and reference locations, counted from 0', () => {
        const answer = {
            locations: [{
                file_path: '/w/main.py',
                range: { start: { line: 4, character: 10 }, end: { line: 4, character: 23 } },
                cite: '/w/main.py:5:11',
            }],
        };
        assert.deepStrictEqual(toReferences(answer), [{ file: '/w/main.py', line: 4, column: 10 }]);
        assert.deepStrictEqual(toReferences({ locations: [] }), []);
    });

    it('sums up a scan', () => {
        const answer = { duration_seconds: 0.020482439, entities_found: 7, files_scanned: 3, scanned_path: '/w' };
        assert.strictEqual(scanSummary(answer), '3 files, 7 entities in 0.0 s');
    });

    it('reads an unexpected answer as no results', () => {
        assert.deepStrictEqual(searchResults(null), []);
        assert.deepStrictEqual(deadCode('text'), []);
        assert.deepStrictEqual(complexity({ most_complex_functions: 'none' }), []);
        assert.deepStrictEqual(relationships(undefined), { entity: '', edges: [] });
    });
});
