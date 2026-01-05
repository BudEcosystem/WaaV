import typescript from '@rollup/plugin-typescript';
import resolve from '@rollup/plugin-node-resolve';
import terser from '@rollup/plugin-terser';

export default {
  input: 'src/index.ts',
  output: [
    {
      file: 'dist/bud-widget.js',
      format: 'iife',
      name: 'BudWidget',
      sourcemap: true,
    },
    {
      file: 'dist/bud-widget.min.js',
      format: 'iife',
      name: 'BudWidget',
      plugins: [terser()],
    },
    {
      file: 'dist/bud-widget.esm.js',
      format: 'es',
      sourcemap: true,
    },
  ],
  plugins: [
    resolve(),
    typescript({
      tsconfig: './tsconfig.json',
      declaration: true,
      declarationDir: './dist',
    }),
  ],
};
