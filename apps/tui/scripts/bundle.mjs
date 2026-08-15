/**
 * Bundle the TUI into a single self-contained JS file for releases.
 * Everything (React, Ink, Zod) is inlined; react-devtools-core is stubbed
 * because a release build never connects to devtools.
 */
import { build } from 'esbuild';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

await build({
  entryPoints: [path.join(here, '../src/cli.tsx')],
  bundle: true,
  platform: 'node',
  format: 'esm',
  outfile: path.join(here, '../dist/mix2.bundle.mjs'),
  alias: { 'react-devtools-core': path.join(here, 'devtools-stub.mjs') },
  banner: {
    js: "import { createRequire } from 'node:module'; const require = createRequire(import.meta.url);",
  },
  minify: false,
  logLevel: 'warning',
});
console.log('bundled dist/mix2.bundle.mjs');
