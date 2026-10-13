import type { APIRoute } from 'astro';
import { abs } from '../lib/site';

const pages = [
  { ru: '/', en: '/en/' },
  { ru: '/privacy/', en: '/en/privacy/' },
];

export const GET: APIRoute = () => {
  const alt = (p: { ru: string; en: string }) =>
    `    <xhtml:link rel="alternate" hreflang="ru" href="${abs(p.ru)}"/>\n` +
    `    <xhtml:link rel="alternate" hreflang="en" href="${abs(p.en)}"/>\n` +
    `    <xhtml:link rel="alternate" hreflang="x-default" href="${abs(p.ru)}"/>\n`;
  const urls = pages
    .flatMap((p) => [p.ru, p.en].map((u) => `  <url>\n    <loc>${abs(u)}</loc>\n${alt(p)}  </url>\n`))
    .join('');
  const xml =
    `<?xml version="1.0" encoding="UTF-8"?>\n` +
    `<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n${urls}</urlset>\n`;
  return new Response(xml, { headers: { 'Content-Type': 'application/xml; charset=utf-8' } });
};
