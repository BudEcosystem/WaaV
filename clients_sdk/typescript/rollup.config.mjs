import resolve from '@rollup/plugin-node-resolve';
import typescript from '@rollup/plugin-typescript';
import dts from 'rollup-plugin-dts';

// Node built-ins (and the `ws` peer) must NOT be bundled into the browser
// build. `events` in particular is a Node core module pulled in by the
// realtime pipeline's EventEmitter; bundling it bloats the browser output and
// can break in environments that provide their own. Externalising it also
// silences rollup's "treating module as external dependency" warning.
const external = [
  'events',
  'node:events',
  'ws',
  'crypto',
  'node:crypto',
  'http',
  'https',
  'url',
];

// Shared TypeScript plugin options for the JS builds. We force
// `declaration`/`declarationMap` off here (the .d.ts bundle is produced by the
// dts plugin in a separate pass); tsconfig has declarationMap:true for editor
// use, so without this override rollup emits TS5069 on every build.
const tsPluginOptions = {
  tsconfig: './tsconfig.json',
  declaration: false,
  declarationMap: false,
};

export default [
  // ESM build
  {
    input: 'src/index.ts',
    output: {
      file: 'dist/index.mjs',
      format: 'esm',
      sourcemap: true,
    },
    external,
    plugins: [resolve(), typescript(tsPluginOptions)],
  },
  // CJS build
  {
    input: 'src/index.ts',
    output: {
      file: 'dist/index.cjs',
      format: 'cjs',
      sourcemap: true,
      exports: 'named',
    },
    external,
    plugins: [resolve(), typescript(tsPluginOptions)],
  },
  // Type declarations
  {
    input: 'src/index.ts',
    output: {
      file: 'dist/index.d.ts',
      format: 'esm',
    },
    external,
    plugins: [dts()],
  },
  // D3: standalone AudioWorklet capture module. It runs in its OWN global scope
  // (AudioWorkletGlobalScope) and must NOT be bundled into the main entry — it
  // is loaded at runtime via `audioWorklet.addModule('.../capture.worklet.js')`.
  // Emitted as an ESM module sibling to the main bundle (recorder.ts derives the
  // URL from import.meta.url, or accepts an explicit `workletUrl`).
  {
    input: 'src/audio/capture.worklet.ts',
    output: {
      file: 'dist/capture.worklet.js',
      format: 'esm',
      sourcemap: true,
    },
    plugins: [resolve(), typescript(tsPluginOptions)],
  },
];
