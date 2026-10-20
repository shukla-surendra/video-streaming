// Small helpers shared by every page.
const $ = (sel, root = document) => root.querySelector(sel);

// Build DOM without innerHTML, so titles typed by uploaders can't inject markup.
function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === 'class') node.className = v;
    else if (k.startsWith('on')) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v);
  }
  node.append(...children.flat().filter(c => c != null));
  return node;
}

function fmtDuration(secs) {
  secs = Math.round(secs || 0);
  const h = Math.floor(secs / 3600), m = Math.floor(secs / 60) % 60, s = secs % 60;
  const mm = h ? String(m).padStart(2, '0') : m;
  return (h ? h + ':' : '') + mm + ':' + String(s).padStart(2, '0');
}

function fmtAgo(unix) {
  const s = Date.now() / 1000 - unix;
  for (const [n, unit] of [[31536000, 'year'], [2592000, 'month'], [86400, 'day'], [3600, 'hour'], [60, 'minute']]) {
    if (s >= n) { const v = Math.floor(s / n); return `${v} ${unit}${v > 1 ? 's' : ''} ago`; }
  }
  return 'just now';
}

function fmtCount(n) {
  n = n || 0;
  if (n < 1000) return String(n);
  if (n < 1e6) return (n / 1e3).toFixed(n < 1e4 ? 1 : 0).replace(/\.0$/, '') + 'K';
  return (n / 1e6).toFixed(n < 1e7 ? 1 : 0).replace(/\.0$/, '') + 'M';
}

function fmtBytes(b) {
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return `${b.toFixed(i ? 1 : 0)} ${u[i]}`;
}

const siteUrl = path => (window.VSTREAM?.siteUrl || '') + path;
const adminUrl = path => (window.VSTREAM?.adminUrl || '') + path;

function header(active) {
  return el('header', {},
    el('a', { href: siteUrl('/'), class: 'logo' }, 'v', el('span', {}, 'stream')),
    el('nav', {},
      el('a', { href: siteUrl('/'), style: active === 'home' ? 'color:var(--text)' : null }, 'Browse'),
      el('a', { href: adminUrl('/admin'), style: active === 'admin' ? 'color:var(--text)' : null }, 'Admin')));
}
