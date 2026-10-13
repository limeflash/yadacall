import { defineConfig } from 'astro/config';
import { SITE, BASE } from './src/lib/site';

export default defineConfig({
  site: SITE,
  base: BASE,
  output: 'static',
  trailingSlash: 'always',
  build: { format: 'directory' },
  devToolbar: { enabled: false },
});
