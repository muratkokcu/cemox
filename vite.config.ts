import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      '/api': { target: 'http://localhost:4100', changeOrigin: true },
      '/health': { target: 'http://localhost:4100', changeOrigin: true },
      '/assets': { target: 'http://localhost:4100', changeOrigin: true },
      '/galery': { target: 'http://localhost:4100', changeOrigin: true }
    }
  }
});
