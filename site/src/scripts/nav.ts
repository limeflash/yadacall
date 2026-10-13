// Header turns solid after 8 px of scroll (design: navCheck()). Shared by every page.
const nav = document.getElementById('yl-nav');
let solid = false;

function navCheck(): void {
  if (!nav) return;
  const next = (window.scrollY || document.documentElement.scrollTop || 0) > 8;
  if (next === solid) return;
  solid = next;
  nav.style.background = solid ? 'color-mix(in srgb, var(--bg) 76%, transparent)' : 'transparent';
  nav.style.borderBottomColor = solid ? 'var(--border)' : 'transparent';
  const blur = solid ? 'blur(18px) saturate(1.4)' : 'none';
  nav.style.setProperty('-webkit-backdrop-filter', blur);
  nav.style.setProperty('backdrop-filter', blur);
}

window.addEventListener('scroll', navCheck, { passive: true });
window.addEventListener('resize', navCheck);
navCheck();
