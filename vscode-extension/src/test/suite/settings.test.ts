import * as assert from 'assert';
import * as fs from 'fs';
import * as path from 'path';

/** The extension's root, from `out/test/suite`. */
const ROOT = path.join(__dirname, '..', '..', '..');

const manifest = JSON.parse(fs.readFileSync(path.join(ROOT, 'package.json'), 'utf8'));

/** The extension's sources, all in one. */
const sources = fs.readdirSync(path.join(ROOT, 'src'))
    .filter((file) => file.endsWith('.ts'))
    .map((file) => fs.readFileSync(path.join(ROOT, 'src', file), 'utf8'))
    .join('\n');

/**
 * Every setting the extension declares acts: the code reads it, or a `when`
 * clause of the manifest does.
 */
describe('Settings', () => {
    const whenClauses = JSON.stringify([manifest.contributes.menus, manifest.contributes.keybindings]);

    for (const key of Object.keys(manifest.contributes.configuration.properties)) {
        it(`reads ${key}`, () => {
            const name = key.replace(/^devscriptor\./, '');
            assert.ok(
                sources.includes(`'${name}'`) || sources.includes(`'${key}'`) || whenClauses.includes(`config.${key}`),
                `${key} is declared, but neither the code nor a when clause reads it`
            );
        });
    }

    it('registers the MCP server provider it declares', () => {
        const providers = manifest.contributes.mcpServerDefinitionProviders as { id: string }[];
        assert.deepStrictEqual(providers.map((provider) => provider.id), ['devscriptor.mcp']);
        assert.ok(sources.includes(`'devscriptor.mcp'`));
    });
});
