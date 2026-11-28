'use strict';
// First in the page, before anything draws: the theme you chose (Settings),
// so a light page never flashes dark. Then md: the markdown agents write, as
// HTML. All of it is escaped before a tag is made, and a link opens only
// http(s) and mailto.

try { const t = localStorage.getItem('rush.theme'); if (t === 'light' || t === 'dark') document.documentElement.dataset.theme = t; } catch {}

const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));

// hl colours a code block a little: comments, strings, numbers and the
// words most languages keep. Not a parser; a language it doesn't know is
// left plain.
const HL_WORDS = new Set(('as async await break case catch class const continue crate def default defer do dyn elif else enum export extends false finally fn for from func function go if impl ' +
  'import in interface is let loop match mod mut new nil None null of package pub return self static struct super switch then this throw trait true True False try type ' +
  'undefined use var void where while with yield fi done esac elif echo end private public protected readonly').split(' '));
const HL_HASH = /^(sh|bash|zsh|shell|console|py|python|rb|ruby|yaml|yml|toml|dockerfile|make|makefile|ini|conf|r|pl|perl|nix|tf|hcl)$/;
const HL_SLASH = /^(js|jsx|mjs|ts|tsx|javascript|typescript|go|rs|rust|c|h|cpp|cc|hpp|java|kt|kotlin|swift|cs|csharp|php|scala|dart|zig|json|jsonc|json5|css|scss|proto|sql)$/;
function hl(code, lang) {
  lang = (lang || '').toLowerCase();
  if (lang === 'diff' || lang === 'patch') return diffLines(code);
  const hash = HL_HASH.test(lang), slash = HL_SLASH.test(lang);
  if (!hash && !slash) return esc(code);
  const quotes = /^(rs|rust)$/.test(lang) ? '"(?:\\\\.|[^"\\\\\\n])*"' : '"(?:\\\\.|[^"\\\\\\n])*"|\'(?:\\\\.|[^\'\\\\\\n])*\'|`(?:\\\\.|[^`\\\\])*`';
  const re = new RegExp((slash ? '(\\/\\/[^\\n]*|\\/\\*[\\s\\S]*?\\*\\/)' : '((?:^|(?<=\\s))#[^\\n]*)') + `|(${quotes})|\\b(\\d[\\w.]*)\\b|\\b([A-Za-z_]\\w*)\\b`, 'gm');
  let out = '', at = 0, m;
  while ((m = re.exec(code))) {
    out += esc(code.slice(at, m.index)); at = re.lastIndex;
    const t = esc(m[0]);
    out += m[1] ? `<i class=c>${t}</i>` : m[2] ? `<i class=s>${t}</i>` : m[3] ? `<i class=n>${t}</i>` : HL_WORDS.has(m[0]) ? `<i class=k>${t}</i>` : t;
    if (!m[0]) re.lastIndex++;
  }
  return out + esc(code.slice(at));
}
// a diff's lines, coloured by what they do
const diffLines = d => d.split('\n').map(l => {
  const c = l.startsWith('+++') || l.startsWith('---') || l.startsWith('diff ') || l.startsWith('index ') || l.startsWith('@@') ? 'hunk' : l[0] === '+' ? 'add' : l[0] === '-' ? 'del' : '';
  return c ? `<span class=${c}>${esc(l) || ' '}</span>` : esc(l) + '\n';
}).join('').replace(/\n$/, '');
const codeBlock = (code, lang) => `<div class=code><div class=code-head><span>${esc(lang || 'text')}</span><button type=button data-copy>copy</button></div><pre><code>${hl(code, lang)}</code></pre></div>`;

function mdInline(s) {
  // code spans come out first, so nothing in them reads as markup
  const codes = [];
  s = s.replace(/(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/g, (_, t, c) => { codes.push(c); return `\u0001${codes.length - 1}\u0001`; });
  s = esc(s)
    .replace(/\[([^\]\n]+)\]\(([^)\s]+)\)|\bhttps?:\/\/[^\s<]*[^\s<.,;:!?)\]'"*_]/g, (m, t, u) => {
      const url = u || m;
      if (!/^(https?:|mailto:)/i.test(url)) return m;
      return `<a href="${url}" target=_blank rel="noopener noreferrer">${t || m}</a>`;
    })
    .replace(/\*\*([^*\n](?:[^\n]*?[^*\n])?)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^\w*])\*([^*\s](?:[^*\n]*?[^*\s])?)\*(?![\w*])/g, '$1<em>$2</em>')
    .replace(/(^|[^\w_])_([^_\s](?:[^_\n]*?[^_\s])?)_(?![\w_])/g, '$1<em>$2</em>')
    .replace(/~~([^~\n]+)~~/g, '<del>$1</del>');
  return s.replace(/\u0001(\d+)\u0001/g, (_, i) => `<code>${esc(codes[i])}</code>`);
}

const MD_LIST = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const MD_FENCE = /^(\s*)(```+|~~~+)\s*([\w+#.-]*)[^\n]*$/;
const mdCells = l => l.trim().replace(/^\||\|$/g, '').split(/(?<!\\)\|/).map(c => c.trim().replace(/\\\|/g, '|'));
function md(src) {
  const lines = String(src ?? '').replace(/\r\n?/g, '\n').split('\n');
  let out = '', i = 0, m;
  const blank = l => !l.trim();
  while (i < lines.length) {
    const l = lines[i];
    if (blank(l)) { i++; continue; }
    if ((m = l.match(MD_FENCE))) {
      // a block not yet closed (it's still being written) runs to the end
      const end = new RegExp('^\\s*' + m[2][0] + '{' + m[2].length + ',}\\s*$'), body = [];
      for (i++; i < lines.length && !end.test(lines[i]); i++) body.push(lines[i].startsWith(m[1]) ? lines[i].slice(m[1].length) : lines[i]);
      i++;
      out += codeBlock(body.join('\n'), m[3]);
      continue;
    }
    if ((m = l.match(/^(#{1,6})\s+(.*?)\s*#*\s*$/))) { const n = Math.min(4, m[1].length + 1); out += `<h${n}>${mdInline(m[2])}</h${n}>`; i++; continue; }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(l)) { out += '<hr>'; i++; continue; }
    if (/^\s*>/.test(l)) {
      const body = [];
      for (; i < lines.length && /^\s*>/.test(lines[i]); i++) body.push(lines[i].replace(/^\s*> ?/, ''));
      out += `<blockquote class=md>${md(body.join('\n'))}</blockquote>`;
      continue;
    }
    if (l.includes('|') && i + 1 < lines.length && /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/.test(lines[i + 1]) && lines[i + 1].includes('-')) {
      const head = mdCells(l), rows = [];
      for (i += 2; i < lines.length && lines[i].includes('|') && !blank(lines[i]); i++) rows.push(mdCells(lines[i]));
      out += `<div class=tablebox><table><thead><tr>${head.map(c => `<th>${mdInline(c)}</th>`).join('')}</tr></thead><tbody>${
        rows.map(r => `<tr>${head.map((_, k) => `<td>${mdInline(r[k] || '')}</td>`).join('')}</tr>`).join('')}</tbody></table></div>`;
      continue;
    }
    if ((m = l.match(MD_LIST))) {
      // items at this indent; what sits deeper under one is its own markdown
      const indent = m[1].length, ordered = /\d/.test(m[2]), items = [];
      for (; i < lines.length; i++) {
        const x = lines[i], k = x.match(MD_LIST);
        if (blank(x)) {
          // a gap carries on into another item of this list, or something set under the last
          let n = i + 1;
          while (n < lines.length && blank(lines[n])) n++;
          const nk = n < lines.length && lines[n].match(MD_LIST);
          if (!(nk ? nk[1].length > indent + 1 || (nk[1].length >= indent && /\d/.test(nk[2]) === ordered) : n < lines.length && /^\s{2,}\S/.test(lines[n]))) break;
          items[items.length - 1].rest.push('');
        } else if (k && k[1].length <= indent + 1) {
          if (/\d/.test(k[2]) !== ordered || k[1].length < indent) break;
          items.push({first: k[3], n: parseInt(k[2], 10), rest: []});
        } else if (k || /^\s{2,}\S/.test(x)) items[items.length - 1].rest.push(x);
        else if (items[items.length - 1].rest.length || MD_FENCE.test(x) || /^#{1,6}\s/.test(x)) break;
        else items[items.length - 1].first += '\n' + x.trim(); // a lazy continuation of its line
      }
      const tag = ordered ? 'ol' : 'ul';
      out += `<${tag}${ordered && items[0].n > 1 ? ` start=${items[0].n}` : ''}>${items.map(it => {
        const cut = Math.min(...it.rest.filter(r => r.trim()).map(r => r.match(/^\s*/)[0].length), 99);
        const sub = it.rest.length ? md(it.rest.map(r => r.slice(cut)).join('\n')) : '';
        const task = it.first.match(/^\[([ xX])\]\s+(.*)$/s);
        return task ? `<li class="task${task[1] === ' ' ? '' : ' done'}"><span>${task[1] === ' ' ? '☐' : '☑'}</span>${mdInline(task[2])}${sub}</li>` : `<li>${mdInline(it.first)}${sub}</li>`;
      }).join('')}</${tag}>`;
      continue;
    }
    const para = [];
    for (; i < lines.length && !blank(lines[i]) && !MD_FENCE.test(lines[i]) && !/^#{1,6}\s/.test(lines[i]) && !/^\s*>/.test(lines[i]) && !(para.length && MD_LIST.test(lines[i])); i++) para.push(lines[i]);
    if (!para.length) para.push(lines[i++]);
    out += `<p>${mdInline(para.join('\n'))}</p>`;
  }
  return out;
}
