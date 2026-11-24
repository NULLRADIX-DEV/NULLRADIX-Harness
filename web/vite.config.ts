import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

const daemon = 'http://127.0.0.1:4711';

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': { target: daemon, changeOrigin: false },
    },
  },
  build: {
    outDir: 'dist',
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    restoreMocks: true,
    unstubGlobals: true,
  },
});
