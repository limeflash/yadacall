# YadaCall site

Astro static site for GitHub Pages: RU landing (`/`), EN landing (`/en/`), privacy policy (`/privacy/`, `/en/privacy/`), 404.

```sh
pnpm install
pnpm dev        # http://localhost:4321/yadacall/
pnpm build      # -> dist/
pnpm check      # astro check
```

- Address: `src/lib/site.ts` (`SITE`, `BASE`). A custom domain later = change those two constants.
- Copy lives in `src/lib/i18n.ts` (verbatim from the approved design) and `src/lib/strings.ts` (SEO, privacy text).
- Release version, download links and installer sizes are fetched from the GitHub API at build time (`GITHUB_TOKEN` is used if set; any failure falls back to `releases/latest`) and refreshed in the browser (`src/scripts/downloads.ts`).
- Landing motion (orb canvas, dot grid, typing loop, tilt, reveals, count-up) is `src/scripts/landing.ts`, a 1:1 port of the design's logic. Do not retune constants without checking against the design.
