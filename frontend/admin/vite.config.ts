/// <reference types="vitest/config" />

import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: 'dist',
    // Keep the checked-in directory marker so a Cargo-only build still has a
    // valid RustEmbed folder before the frontend has been compiled.
    emptyOutDir: false,
  },
  test: {
    environment: 'jsdom',
    globals: true,
  },
});
