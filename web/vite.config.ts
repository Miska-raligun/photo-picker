import { defineConfig } from 'vite'
import path from 'node:path'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') },
  },
  build: {
    rollupOptions: {
      output: {
        // Split heavy third-party libs into their own cacheable chunks so the
        // main app bundle stays small and a dependency bump doesn't bust the
        // whole cache. `motion` (~130KB) is the biggest single offender.
        manualChunks(id: string) {
          if (!id.includes('node_modules')) return undefined
          if (id.includes('/motion/') || id.includes('/framer-motion/')) return 'motion'
          if (id.includes('/radix-ui/') || id.includes('/@radix-ui/')) return 'radix'
          if (
            id.includes('/react/') ||
            id.includes('/react-dom/') ||
            id.includes('/scheduler/')
          )
            return 'react'
          return 'vendor'
        },
      },
    },
  },
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:7777',
    },
  },
})
