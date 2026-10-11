// Downloads CryptoJS, CodeMirror and the Google fonts into ./vendor so the desktop
// app works fully offline. Needs Node 18+ and internet access (run once, commit the result).
//   node scripts/vendor-deps.mjs src/vendor
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';

const OUT = process.argv[2] || 'src/vendor';
const CDN = 'https://cdnjs.cloudflare.com/ajax/libs/';
const CM = 'codemirror/5.65.16/';

const FILES = [
  ['crypto-js/4.2.0/crypto-js.min.js', 'crypto-js.min.js'],
  [CM + 'codemirror.min.css', 'codemirror/codemirror.min.css'],
  [CM + 'theme/material-darker.min.css', 'codemirror/theme/material-darker.min.css'],
  [CM + 'codemirror.min.js', 'codemirror/codemirror.min.js'],
  ...['mode/javascript/javascript', 'mode/xml/xml', 'addon/edit/matchbrackets', 'addon/edit/closebrackets',
      'addon/selection/active-line', 'addon/comment/comment']
    .map(p => [CM + p + '.min.js', 'codemirror/' + p + '.min.js']),
];

const FONTS_CSS = 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700' +
  '&family=Inter+Tight:wght@500;600;700&family=JetBrains+Mono:wght@400;500&display=swap';

async function get(url, headers) {
  const r = await fetch(url, { headers });
  if (!r.ok) throw new Error(`${r.status} ${url}`);
  return Buffer.from(await r.arrayBuffer());
}
async function save(rel, buf) {
  const p = join(OUT, rel);
  await mkdir(dirname(p), { recursive: true });
  await writeFile(p, buf);
  console.log('  ✓', rel);
}

for (const [remote, local] of FILES) await save(local, await get(CDN + remote));

// Fonts: fetch the CSS with a modern UA (-> woff2), download each file, point the CSS at the local copies.
let css = (await get(FONTS_CSS, { 'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/124 Safari/537.36' })).toString();
let i = 0;
for (const url of [...new Set([...css.matchAll(/url\((https:[^)]+)\)/g)].map(m => m[1]))]) {
  const name = `f${String(++i).padStart(2, '0')}.woff2`;
  await save('fonts/' + name, await get(url));
  css = css.split(url).join('./' + name);
}
await save('fonts/fonts.css', Buffer.from(css));
console.log('Done ->', OUT);
