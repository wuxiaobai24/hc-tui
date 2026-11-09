import { defineConfig } from 'astro/config';
import tailwindcss from '@tailwindcss/vite';

// 站点既要在本地根路径预览（base = "/"），也要能部署到 GitHub Pages 的项目子路径
// （base = "/hc-tui/"）。两者都由构建环境给出，避免把路径写死进产物。
const site = process.env.SITE_URL ?? 'http://localhost:4321';
const base = process.env.SITE_BASE ?? '/';

export default defineConfig({
  site,
  base,
  cacheDir: './node_modules/.astro',
  build: { format: 'directory', inlineStylesheets: 'always' },
  vite: { plugins: [tailwindcss()] },
});
