import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
// @ts-expect-error type error without @types/node package
import process from 'node:process';

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  // Vite options tailored for Tauri development, applied only in `tauri dev` or `tauri build`:
  // 1. don't obscure Rust errors
  clearScreen: false,
  // 2. Tauri expects a fixed port; fail if it is taken
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    // 3. don't watch the Rust side
    watch: { ignored: ['**/src-tauri/**'] },
  },
}));
