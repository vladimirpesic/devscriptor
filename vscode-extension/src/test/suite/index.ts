import * as path from 'path';
import * as fs from 'fs';
import Mocha = require('mocha');

export function run(): Promise<void> {
    const mocha = new Mocha({ ui: 'bdd', color: true, timeout: 30000 });
    const testsRoot = path.resolve(__dirname, '.');

    return new Promise((resolve, reject) => {
        const files = fs.readdirSync(testsRoot).filter(f => f.endsWith('.test.js'));
        files.sort().forEach(f => mocha.addFile(path.resolve(testsRoot, f)));
        mocha.run(failures => {
            if (failures > 0) {
                reject(new Error(`${failures} test(s) failed.`));
            } else {
                resolve();
            }
        });
    });
}
