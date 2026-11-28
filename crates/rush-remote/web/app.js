'use strict';
// Rush's web app: your sessions on this machine (and its peers), from a
// phone or a browser, in rush's own look. Plain JS and CSS, no build:
// md.js (the theme, markdown), session.js (one session), and this, the
// rest, which starts it.
//
// Routes are the hash, and the hash alone says what's on screen: #/ the
// sessions, #/s/<machine>/<id> one (…/all with its whole history), #/new,
// #/devices (settings, and the devices paired). Each screen is a scope:
// leaving it ends its streams and timers, and anything still in flight
// from it is dropped, so nothing late ever draws over the screen you're
// on. The list of sessions is a scope of its own: on a wide window it
// stays beside every screen, on a narrow one it is the #/ screen and ends
// when you leave it. Lists update rows in place.

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const el = html => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstElementChild; };
const enc = encodeURIComponent;

let machines = [], machine = '', base = '', me = '', lastList = [];

// api: GET reads are safe to repeat; writes are sent once and never retried
// (a send that was delivered but unanswered would be sent twice).
const api = (p, o) => fetch(base + '/api' + p, o).then(async r => {
  if (r.status === 401) throw 401;
  if (!r.ok) throw ((await r.json().catch(() => ({}))).error || r.statusText || 'HTTP ' + r.status);
  return r;
}, () => { throw "can't reach rush"; });
const post = (p, b) => api(p, {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(b)});
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js').catch(() => {});

// what this browser keeps: drafts, folds, the last folder
const store = {
  get(k, d) { try { const v = localStorage.getItem('rush.' + k); return v === null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { v === undefined || v === '' ? localStorage.removeItem('rush.' + k) : localStorage.setItem('rush.' + k, JSON.stringify(v)); } catch {} },
};

// the rush bottle, as the header draws it, a pixel to a cell
const bottle = (cls = 'bottle') => `<svg class="${cls}" viewBox="0 0 6 10" shape-rendering="crispEdges" aria-hidden="true">
  <rect width="6" height="1" fill="#707074"/><rect y="1" width="6" height="1" fill="#404044"/><rect y="2" width="6" height="1" fill="#58585c"/>
  <rect x="1" y="3" width="4" height="1" fill="#9a4c14"/><rect y="4" width="6" height="1" fill="#9a4c14"/><rect y="5" width="6" height="3" fill="#fad61e"/>
  <rect x="1" y="6" width="4" height="1" fill="#d61c18"/><rect y="8" width="6" height="1" fill="#9a4c14"/><rect y="9" width="6" height="1" fill="#542606"/></svg>`;

// ── words ──
const zero = t => !t || t.startsWith('0001');
const ago = t => {
  if (zero(t)) return '';
  const s = (Date.now() - new Date(t)) / 1000;
  if (s < 45) return 'now';
  if (s < 3600) return Math.round(s / 60) + 'm';
  if (s < 86400) return Math.round(s / 3600) + 'h';
  if (s < 7 * 86400) return Math.round(s / 86400) + 'd';
  return new Date(t).toLocaleDateString([], {day: 'numeric', month: 'short'});
};
const clock = t => new Date(t).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'});
const day = t => new Date(t).toLocaleDateString([], {day: 'numeric', month: 'short'});
// dur is seconds as rush says them: 42s, 3m 05s, 1h 04m
const dur = s => { s = Math.max(0, Math.round(s)); return s < 60 ? s + 's' : s < 3600 ? `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s` : `${Math.floor(s / 3600)}h ${String(Math.floor(s % 3600 / 60)).padStart(2, '0')}m`; };
const home = p => String(p || '').replace(/^\/(Users|home)\/[^/]+/, '~');
const money = n => n ? '$' + n.toFixed(2) : '';
const tokens = n => n >= 1e6 ? (n / 1e6).toFixed(1) + 'M' : n >= 1e3 ? Math.round(n / 1e3) + 'k' : String(n || 0);
const plural = (n, w) => `${n} ${w}${n === 1 ? '' : 's'}`;
const agentName = k => ({'': 'Claude', claude: 'Claude', codex: 'Codex', gemini: 'Gemini', kimi: 'Kimi', antigravity: 'Antigravity'}[k || ''] || k);
const stateOf = i => i.alive === false ? (i.sleeping ? 'asleep' : 'stopped') : (i.state || 'idle');
const marks = {blocked: '●', working: '✻', starting: '✻', idle: '◆', asleep: '◇', stopped: '·'};
const stateWords = (st, needs) => st === 'blocked' ? 'needs ' + (needs || 'you') : st;
const state = (st, needs) => `<span class="st-${esc(st)}">${marks[st] || '·'} ${esc(stateWords(st, needs))}</span>`;
// the project a folder is, and the worktree of it when it's one
const project = cwd => {
  const m = String(cwd || '').match(/^(.*?)\/\.claude\/worktrees\/([^/]+)/), p = (m ? m[1] : String(cwd || '')).replace(/\/+$/, '');
  return {name: p.split('/').pop() || '/', tree: m ? m[2] : ''};
};
const fine = matchMedia('(pointer: fine)'), wideQ = matchMedia('(min-width: 900px)');
const wide = () => wideQ.matches;
const mac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
const buzz = ms => { try { navigator.vibrate && navigator.userActivation?.hasBeenActive && navigator.vibrate(ms); } catch {} };
const copy = async (text, what = 'copied') => {
  try { await navigator.clipboard.writeText(text); toast(what); } catch { toast("This browser won't let the page copy.", 'bad'); }
};

// ── screens ──
function mkScope() {
  const ends = [];
  const s = {live: true, on: f => ends.push(f), end() { s.live = false; ends.splice(0).forEach(f => { try { f(); } catch {} }); }};
  return s;
}
let scope = null;
// what the screen offers the keyboard and the palette: {keys: {c: fn}, acts: [{name, hint, run}]}
let view = {keys: {}, acts: []};
function newScope() {
  if (scope) scope.end();
  view = {keys: {}, acts: []};
  return scope = mkScope();
}
let needs = 0, barTitle = 'Rush';
const titled = () => { document.title = (needs ? `(${needs}) ` : '') + (barTitle && barTitle !== 'Rush' ? barTitle + ' · Rush' : 'Rush'); };
function bar({title = 'Rush', sub = '', actions, plain} = {}) {
  set($('#title'), '', title, true);
  set($('#sub'), '', sub, true);
  if (actions) $('#actions').replaceChildren(...actions);
  barTitle = plain ?? $('#title').textContent; titled();
}
const action = (id, words, onclick, label) => { const b = el(`<button class="key key-plain" id="${id}"${label ? ` aria-label="${esc(label)}" title="${esc(label)}"` : ''}>${words}</button>`); b.onclick = onclick; return b; };
const go = h => { if (location.hash === h) route(); else location.hash = h; };
const sessionHash = (id, m = machine) => `#/s/${enc(m)}/${enc(id)}`;
$('#back').onclick = () => go('#/');

function route() {
  if (!machines.length) return;
  const h = location.hash, app = $('#app'), was = machine;
  let m, name = 'home';
  if ((m = h.match(/^#\/s\/([^/]+)\/([^/]+)(\/all)?$/))) { name = 'session'; setMachine(decodeURIComponent(m[1])); }
  else if (h === '#/new') name = 'new';
  else if (h === '#/devices') name = 'devices';
  app.dataset.route = name;
  $$('#tabbar a').forEach(a => a.dataset.tab === name ? a.setAttribute('aria-current', 'page') : a.removeAttribute('aria-current'));
  $('#actions').replaceChildren();
  syncList(was !== machine);
  if (name === 'session') sessionScreen(decodeURIComponent(m[2]), !!m[3]);
  else if (name === 'new') newScreen();
  else if (name === 'devices') devicesScreen();
  else homeScreen();
  markCurrent();
}
addEventListener('hashchange', route);
wideQ.addEventListener('change', () => machines.length && route());

function setMachine(name) {
  const m = machines.find(x => x.name === name) || machines[0];
  machine = m.name; base = m.self ? '' : '/m/' + enc(m.name);
}

// the page is as tall as what's left above an on-screen keyboard
if (window.visualViewport) {
  const fit = () => { if (visualViewport.scale === 1) document.documentElement.style.setProperty('--vvh', visualViewport.height + 'px'); scrollTo(0, 0); };
  visualViewport.addEventListener('resize', fit);
  visualViewport.addEventListener('scroll', () => scrollTo(0, 0));
}

// ── toasts, sheets ──
function toast(text, kind, act) {
  const t = el(`<div class=toast data-kind="${kind || ''}" role=${kind === 'bad' ? 'alert' : 'status'}><span>${esc(text)}</span></div>`);
  if (act) { const b = el(`<button>${esc(act.name)}</button>`); b.onclick = () => { t.remove(); act.run(); }; t.append(b); }
  t.onclick = e => { if (!e.target.closest('button')) t.remove(); };
  $('#toasts').append(t);
  setTimeout(() => t.remove(), kind === 'bad' ? 8000 : act ? 6000 : 3000);
  return t;
}
function sheet(html, {title = '', big = false, sc = scope} = {}) {
  const d = el(`<dialog class="sheet${big ? ' big' : ''}" aria-label="${esc(title)}"><div class=sheet-body>${
    title ? `<div class=sheet-head><h2>${esc(title)}</h2><button class="key key-plain" data-close aria-label=Close>✕</button></div>` : ''}${html}</div></dialog>`);
  document.body.append(d);
  d.addEventListener('close', () => d.remove());
  d.addEventListener('click', e => { if (e.target === d || e.target.closest('[data-close]')) d.close(); });
  d.showModal();
  sc && sc.on(() => d.open && d.close());
  return d;
}
function confirmSheet(title, text, ok, danger, sc = scope) {
  return new Promise(res => {
    const d = sheet(`<p class=c-sub>${esc(text)}</p><div class="line end"><button class=key data-close>cancel</button><button class="key ${danger ? 'key-bad' : 'key-on'}" id=ok>${esc(ok)}</button></div>`, {title, sc});
    let v = false;
    $('#ok', d).onclick = () => { v = true; d.close(); };
    d.addEventListener('close', () => res(v));
  });
}
// anything marked data-copy copies what it names, or the code beside it
document.addEventListener('click', e => {
  const b = e.target.closest('[data-copy]');
  if (!b) return;
  const pre = b.dataset.copy ? null : $('pre', b.closest('.code'));
  copy(b.dataset.copy || (pre ? pre.textContent : ''));
});

// keyed puts one element per item in box, in order, updating those it has
// rather than drawing them again: what you're looking at stays put.
function keyed(box, items, key, make, update) {
  const had = new Map($$(':scope > [data-key]', box).map(e => [e.dataset.key, e]));
  let prev = null;
  for (const it of items) {
    const k = key(it);
    let e = had.get(k);
    if (e) had.delete(k); else { e = make(it); e.dataset.key = k; }
    update(e, it);
    const at = prev ? prev.nextElementSibling : box.firstElementChild;
    if (at !== e) box.insertBefore(e, at);
    prev = e;
  }
  had.forEach(e => e.remove());
}
const set = (e, sel, v, html) => { const t = sel ? $(sel, e) : e; if (html ? t.innerHTML !== v : t.textContent !== v) html ? t.innerHTML = v : t.textContent = v; };

// times say how long ago, kept current
setInterval(() => $$('time[data-t]').forEach(t => { t.textContent = ago(t.dataset.t); }), 30e3);

// ── start ──
async function start() {
  $('#logo').innerHTML = bottle();
  $('#palkey').textContent = mac ? '⌘K' : 'ctrl K';
  // opened from rush's pairing QR code: trade the one-time code for the sign-in
  const pair = location.hash.match(/^#pair=([\w-]+)$/);
  if (pair) {
    history.replaceState(null, '', location.pathname + '#/');
    const r = await fetch('/api/login', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({code: pair[1]})}).catch(() => null);
    if (r && r.ok) toast('paired with ' + (await r.json()).name);
    else toast('That pairing code has been used or has run out. Show a new one in rush: Settings › Remote.', 'bad');
  }
  try {
    const r = await fetch('/api/machines');
    if (r.status === 401) throw 401;
    if (!r.ok) throw r.statusText || 'HTTP ' + r.status;
    machines = await r.json();
  } catch (e) { return e === 401 ? signIn() : offline(e instanceof TypeError ? 'No answer from this machine. Is rush serve running, and are you on its network?' : e); }
  setMachine(machine || store.get('machine') || machines[0].name);
  route();
}
function alone(html) {
  newScope(); endList(); machines = [];
  $('#app').dataset.route = 'signin';
  $('#main').innerHTML = html;
  needs = 0; barTitle = 'Rush'; titled();
}
function offline(e) {
  alone(`<div class=empty>${bottle()}<h2 class=c-red>Can't reach rush</h2><p>${esc(e)}</p><p><button class="key key-on" id=retry>try again</button></p></div>`);
  $('#retry').onclick = start;
  addEventListener('online', start, {once: true});
}

function signIn() {
  alone(`<div class=signin>
    <div class=signin-head>${bottle()}<div><b>rush</b><p class=c-dim>your agents, from anywhere on your tailnet</p></div></div>
    <fieldset class=box><legend>pair this device</legend><p class=c-sub>In rush on the machine: <b>Settings › Remote › Pair a device</b>. Scan its code with a phone's camera, or type the code below.</p></fieldset>
    <form id=signin><fieldset class=box><legend>pairing code, or the machine's token</legend>
      <input id=tok class=field type=password autocomplete=current-password placeholder="K7QM-3XHP" aria-label="Pairing code or token" autocapitalize=characters autocorrect=off spellcheck=false>
      <p class=note>The token is what <span class=c-text>rush remote token</span> prints there.</p>
      <div class=err-line id=sierr role=alert></div>
      <div class="line end"><label class="key key-plain"><input type=checkbox id=show> show</label><button class="key key-on" id=go>sign in</button></div></fieldset></form></div>`);
  $('#show').onchange = e => { $('#tok').type = e.target.checked ? 'text' : 'password'; };
  if (fine.matches) $('#tok').focus();
  $('#signin').onsubmit = async e => {
    e.preventDefault();
    const v = $('#tok').value.trim(), b = $('#go');
    if (!v) return $('#tok').focus();
    // a pairing code is eight of its letters, with or without the dash
    const code = /^[2-9A-HJ-NP-Z]{4}-?[2-9A-HJ-NP-Z]{4}$/i.test(v);
    b.disabled = true; b.classList.add('busy'); $('#sierr').textContent = '';
    const r = await fetch('/api/login', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(code ? {code: v.toUpperCase().replace(/^(.{4})-?/, '$1-')} : {token: v})}).catch(() => null);
    if (r && r.ok) { history.replaceState(null, '', location.pathname + '#/'); return start(); }
    b.disabled = false; b.classList.remove('busy');
    $('#sierr').textContent = !r ? "Can't reach rush. Check you're on its network." : code ? 'That code has been used or has run out. Show a new one in rush.' : "That isn't this machine's token.";
    $('#tok').select();
  };
}
const failed = e => e === 401 ? signIn() : toast(String(e), 'bad');

// ── the sessions: the list beside every screen, or the #/ screen ──
const groups = [['blocked', 'Needs you'], ['working', 'Working'], ['idle', 'Idle'], ['asleep', 'Asleep'], ['stopped', 'Stopped']];
const groupOf = st => st === 'starting' ? 'working' : groups.some(g => g[0] === st) ? st : 'idle';
let listSc = null, listFor = '';
function endList() { if (listSc) { listSc.end(); listSc = null; } }
function syncList(force) {
  if (!wide() && $('#app').dataset.route !== 'home') return endList();
  if (listSc && listSc.live && listFor === machine && !force) return;
  endList();
  listSc = mkScope(); listFor = machine;
  listPane(listSc);
}
function markCurrent() {
  const h = location.hash.replace(/\/all$/, '');
  $$('#list .row-link').forEach(a => a.getAttribute('href') === h ? a.setAttribute('aria-current', 'page') : a.removeAttribute('aria-current'));
}
function listPane(sc) {
  const list = $('#list'), q = $('#q');
  store.set('machine', machine);
  // the machines, when this one shows others
  const tabs = $('#machs');
  tabs.hidden = machines.length < 2;
  tabs.innerHTML = machines.map(m => `<button class=key role=tab aria-selected=${m.name === machine} data-m="${esc(m.name)}">${esc(m.name)}</button>`).join('');
  tabs.onclick = e => { const b = e.target.closest('[data-m]'); if (b && b.dataset.m !== machine) { setMachine(b.dataset.m); syncList(true); if ($('#app').dataset.route === 'home') homeScreen(); } };
  list.innerHTML = '<div class=row-link><div class="skeleton"></div></div>'.repeat(5);
  $('#oth').innerHTML = '';
  $('#conn').hidden = true;

  let all = null, oth = [], more = false, before = null;
  const folded = new Set(store.get('fold', ['stopped', 'others']));
  const draw = () => {
    if (!sc.live || !all) return;
    const words = q.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const hit = (...f) => words.every(w => f.some(x => String(x || '').toLowerCase().includes(w)));
    const ss = all.map(i => ({...i, st: stateOf(i)}));
    const shown = ss.filter(i => hit(i.name, i.cwd, agentName(i.kind), i.model, i.detail, i.st, i.id));
    const projects = new Set(ss.map(i => project(i.cwd).name)).size > 1;
    const rows = [];
    for (const [g, name] of groups) {
      const in_ = shown.filter(i => groupOf(i.st) === g).sort((a, b) => a.updatedAt < b.updatedAt ? 1 : -1);
      if (!in_.length) continue;
      const shut = folded.has(g) && !words.length;
      rows.push({hdr: g, name, n: in_.length, cost: in_.reduce((n, i) => n + (i.costUsd || 0), 0), shut});
      if (shut) continue;
      if (!projects) { rows.push(...in_); continue; }
      // by project, the one touched last first
      const by = new Map();
      for (const i of in_) { const p = project(i.cwd).name; by.has(p) || by.set(p, []); by.get(p).push(i); }
      for (const [p, l] of by) rows.push({proj: p, g}, ...l);
    }
    if (!ss.length) list.innerHTML = `<div class=empty>${bottle()}<h2>No sessions on ${esc(machine)} yet</h2><p>Start an agent in a folder and it shows here, with what it's doing and what it needs from you.</p><p><a class="key key-on" href=#/new>start a session</a></p></div>`;
    else if (!rows.length && !oth.some(o => hit(o.name, o.cwd, agentName(o.agent)))) list.innerHTML = `<div class=empty><p>Nothing matches “${esc(q.value.trim())}”.</p><p><button class=key id=unq>clear the search</button></p></div>`;
    else {
      if (!list.querySelector('[data-key]')) list.innerHTML = '';
      keyed(list, rows, i => i.hdr ? 'h:' + i.hdr : i.proj ? `p:${i.g}:${i.proj}` : i.id,
        i => i.hdr ? el('<button class=rule><b></b><span></span></button>') : i.proj ? el('<div class=proj></div>')
          : el(`<div class=row><a class=row-link><div class=row-top><span class=mark aria-hidden=true></span><b></b><span class=cost></span><time></time></div><div class=row-sub><span class=tree aria-hidden=true>└</span><span class=why></span></div></a><div class=row-acts hidden><button class="key key-on" data-quick=allow>allow</button><button class="key key-bad" data-quick=deny>deny</button></div></div>`),
        (e, i) => {
          if (i.hdr) { e.dataset.hdr = i.hdr; e.setAttribute('aria-expanded', !i.shut); set(e, 'b', (i.shut ? '▸ ' : '▾ ') + i.name); set(e, 'span', `${i.n}${i.cost ? ' · ' + money(i.cost) : ''}`); return; }
          if (i.proj) return set(e, '', i.proj);
          const a = $('a', e);
          a.setAttribute('href', sessionHash(i.id));
          e.dataset.id = i.id; e.dataset.state = i.st;
          set(e, '.mark', marks[i.st] || '·');
          set(e, 'b', i.name || i.id);
          set(e, '.cost', money(i.costUsd));
          const t = $('time', e); t.dataset.t = i.updatedAt; set(t, '', ago(i.updatedAt));
          const p = project(i.cwd), ask = (i.asking || [])[0];
          const where = (projects ? (p.tree ? `<span class=c-green>${esc(p.tree)}</span> · ` : '') : `<span class=c-blue>${esc(home(i.cwd))}</span> · `) + `<span>${esc([agentName(i.kind), i.model].filter(Boolean).join(' · '))}</span>`;
          const queued = (i.queue || []).length ? ` <span class=c-queue>⋯ ${i.queue.length} queued</span>` : '';
          set(e, '.why', (i.st === 'blocked' ? `<span class=c-yellow>${esc(stateWords(i.st, i.needs))}</span>${ask ? ` <span>${esc(ask.call.split('\n')[0])}</span>` : ''}`
            : i.error ? `<span class=c-red>${esc(i.error)}</span>`
            : i.limit ? `<span class=c-yellow>usage limit · resets ${esc(clock(i.limit.resetsAt))}</span>`
            : i.decisions ? `<span class=c-yellow>${esc(i.needs || 'decisions')}</span> ${where}`
            : i.st === 'working' && i.detail ? `<span class=c-sub>${esc(i.detail)}</span>` : where) + queued, true);
          a.setAttribute('aria-label', `${i.name || i.id}, ${stateWords(i.st, i.needs)}`);
          // one approval waiting can be answered from here
          const acts = $('.row-acts', e), one = i.st === 'blocked' && (i.asking || []).length === 1 && !(i.questions || []).length;
          acts.hidden = !one;
          if (one) acts.dataset.ask = ask.id;
        });
    }
    markCurrent();
    const n = ss.filter(i => i.st === 'blocked').length, w = ss.filter(i => groupOf(i.st) === 'working').length;
    set($('#sum'), '', [n && `<span class=c-yellow>● ${n} need${n > 1 ? '' : 's'} you</span>`, w && `<span class=c-orange>✻ ${w} working</span>`].filter(Boolean).join(' ') || `<span>${esc(machine)} · ${plural(ss.length, 'session')}</span>`, true);
    needs = n; titled();
    $('#tabn').hidden = !n; $('#tabn').textContent = n;
    if (navigator.setAppBadge) (n ? navigator.setAppBadge(n) : navigator.clearAppBadge()).catch(() => {});
    drawOthers(hit);
  };
  const take = l => {
    if (!sc.live) return;
    // one that has just come to need you says so, wherever you are
    if (before) for (const i of l) {
      const st = stateOf(i);
      if (st === 'blocked' && before.get(i.id) !== 'blocked' && location.hash.replace(/\/all$/, '') !== sessionHash(i.id)) {
        toast(`● ${i.name || i.id} ${stateWords(st, i.needs)}`, 'ask', {name: 'open', run: () => go(sessionHash(i.id))});
        buzz(60);
      }
    }
    before = new Map(l.map(i => [i.id, stateOf(i)]));
    all = lastList = l; draw();
  };
  q.oninput = draw;
  q.onkeydown = e => {
    if (e.key === 'Escape') { if (q.value) { q.value = ''; draw(); } else q.blur(); e.stopPropagation(); }
    if (e.key === 'Enter' || e.key === 'ArrowDown') { const a = $('#list .row-link'); if (a) { e.preventDefault(); e.key === 'Enter' ? a.click() : a.focus(); } }
  };
  $('#listbox').onclick = async e => {
    const h = e.target.closest('.rule');
    if (h) { const g = h.dataset.hdr; folded.has(g) ? folded.delete(g) : folded.add(g); store.set('fold', [...folded]); return draw(); }
    if (e.target.closest('#unq')) { q.value = ''; return draw(); }
    if (e.target.closest('#more')) { more = true; return draw(); }
    const k = e.target.closest('[data-quick]');
    if (k) {
      const row = k.closest('.row'), ask = k.parentNode.dataset.ask, allow = k.dataset.quick === 'allow';
      $$('button', k.parentNode).forEach(b => b.disabled = true);
      try { await post(`/sessions/${enc(row.dataset.id)}/op`, allow ? {op: 'allow', id: ask} : {op: 'deny', id: ask}); buzz(20); toast(allow ? 'allowed' : 'denied'); }
      catch (err) { failed(err); }
      $$('button', k.parentNode).forEach(b => b.disabled = false);
      return;
    }
    const b = e.target.closest('[data-r]');
    if (!b) return;
    if (!b.dataset.r) return toast('It runs in a terminal there. It can be carried on in Rush once it ends.');
    if (!await confirmSheet('Carry it on in Rush', `“${$('b', b).textContent}” carries on as a Rush session on ${machine}, with its own agent and folder.`, 'carry on', false, sc)) return;
    try { const v = await (await post('/sessions', {resume: b.dataset.r})).json(); go(sessionHash(v.id)); }
    catch (err) { failed(err); }
  };

  // the list as serve streams it; a peer whose serve can't stream is asked every 4s
  let got = false, timer = 0;
  const lost = on => { const c = $('#conn'); c.hidden = !on; if (on) c.textContent = navigator.onLine === false ? 'You are offline. The list is as it was.' : `Lost ${machine}. Trying again…`; };
  const poll = () => api('/sessions').then(r => r.json()).then(l => { lost(false); take(l); }, e => { if (!sc.live) return; e === 401 ? signIn() : lost(true); });
  const es = new EventSource(base + '/api/sessions/stream');
  es.onopen = () => lost(false);
  es.onmessage = m => { got = true; lost(false); const v = JSON.parse(m.data); if (v.me) me = v.me; if (v.sessions) take(v.sessions); };
  es.onerror = () => {
    if (!sc.live) return;
    if (!got && !timer) { es.close(); poll(); timer = setInterval(poll, 4000); }
    else if (got) { lost(true); if (es.readyState === 2) api('/machines').catch(e => e === 401 && signIn()); }
  };
  sc.on(() => { es.close(); clearInterval(timer); q.oninput = null; });

  // the machine's other sessions, its agents' own that Rush didn't start: a
  // past one carries on in Rush when picked
  const others = () => api('/others').then(r => r.json()).then(v => { if (sc.live) { oth = v || []; draw(); } }, () => {});
  const drawOthers = hit => {
    const box = $('#oth'), l = oth.filter(o => hit(o.name, o.cwd, agentName(o.agent), o.account));
    if (!l.length) { box.innerHTML = ''; return; }
    const shut = folded.has('others') && !q.value.trim();
    if (!$('#othlist', box)) box.innerHTML = `<button class=rule data-hdr=others><b></b><span id=othn></span></button><div id=othlist></div><div id=othmore class=row-link></div>`;
    $('.rule', box).setAttribute('aria-expanded', !shut);
    set(box, '.rule b', (shut ? '▸ ' : '▾ ') + 'Not started by Rush');
    set($('#othn'), '', String(l.length));
    keyed($('#othlist'), shut ? [] : more ? l : l.slice(0, 20), o => o.sessionId,
      () => el(`<button class=row data-state=stopped><div class=row-link><div class=row-top><span class=mark aria-hidden=true>·</span><b></b><time></time></div><div class=row-sub><span class=tree aria-hidden=true>└</span><span class=why></span></div></div></button>`),
      (e, o) => {
        e.dataset.r = o.live ? '' : o.sessionId;
        set(e, 'b', o.name || o.sessionId);
        const t = $('time', e); t.dataset.t = o.updatedAt || ''; set(t, '', ago(o.updatedAt));
        set(e, '.why', `<span class=c-blue>${esc(home(o.cwd))}</span> · <span>${esc([agentName(o.agent), o.account].filter(Boolean).join(' · '))}${o.live ? ' · in a terminal' : ''}</span>`, true);
      });
    const left = !shut && !more && l.length > 20;
    $('#othmore').hidden = !left;
    $('#othmore').innerHTML = left ? `<button class="key key-plain" id=more>↓ ${l.length - 20} more</button>` : '';
  };
  others();
  const ot = setInterval(others, 60e3);
  sc.on(() => clearInterval(ot));
}
$('#sidenew').onclick = () => go('#/new');
// moves through the list's rows: -1 up, 1 down
function stepRow(d) {
  const rows = $$('#list .row-link[href], #oth button.row'), at = rows.indexOf(document.activeElement);
  const cur = at < 0 ? rows.findIndex(a => a.getAttribute('aria-current')) : at;
  const to = rows[Math.max(0, Math.min(rows.length - 1, (cur < 0 && d < 0 ? rows.length : cur) + d))];
  if (to) { to.focus(); to.scrollIntoView({block: 'nearest'}); }
}
// the session before or after the one on screen, as the list has them
function stepSession(d) {
  const rows = $$('#list .row-link[href]'), at = rows.findIndex(a => a.getAttribute('aria-current'));
  const to = rows[at < 0 ? 0 : at + d];
  if (to) go(to.getAttribute('href'));
}

// #/ on a wide window, where the list is beside it: what there is to do
function homeScreen() {
  newScope();
  bar({title: 'Rush'});
  $('#main').innerHTML = `<div class=welcome>${bottle()}<p>Pick a session, or start one.</p>
    <dl class=keys-list><dt><kbd>${mac ? '⌘' : 'ctrl'} K</kbd></dt><dd>go to anything</dd><dt><kbd>/</kbd></dt><dd>search the sessions</dd><dt><kbd>n</kbd></dt><dd>a new session</dd><dt><kbd>?</kbd></dt><dd>every shortcut</dd></dl></div>`;
}

// ── a new session ──
const AGENTS = ['', 'claude', 'codex', 'gemini', 'kimi'];
let newIn = '';
function newScreen(again) {
  const sc = newScope();
  bar({title: 'New session', sub: `<span>on ${esc(machine)}</span>`});
  const dirs = [...new Set([newIn, store.get('cwd.' + machine), ...lastList.slice().sort((a, b) => a.updatedAt < b.updatedAt ? 1 : -1).map(i => i.cwd)].filter(Boolean))].slice(0, 8);
  $('#main').innerHTML = `<form id=mkform class=page>
    <fieldset class=box><legend>folder on ${esc(machine)}</legend>
      <input id=cwd class=field placeholder="~/src/project" aria-label=Folder autocapitalize=off autocorrect=off spellcheck=false value="${esc(home(dirs[0] || ''))}" required>
      ${dirs.length ? `<div class=seg id=dirs aria-label="Recent folders">${dirs.map(d => `<button type=button class=key data-d="${esc(home(d))}">${esc(project(d).name)}${project(d).tree ? ` <span class=c-green>${esc(project(d).tree)}</span>` : ''}</button>`).join('')}</div>` : ''}</fieldset>
    <fieldset class=box><legend>agent</legend><div class=seg id=ags role=radiogroup aria-label=Agent>${AGENTS.map((a, n) => `<button type=button class=key role=radio aria-checked=${!n} data-a="${a}">${a ? agentName(a) : 'the default'}</button>`).join('')}</div></fieldset>
    <fieldset class=box><legend>first message</legend><textarea id=pr class=field rows=5 placeholder="What should it do?" aria-label="First message"></textarea></fieldset>
    <details><summary class="key key-plain">▸ name, model and effort</summary><div class=box>
      <input id=nm class=field placeholder="a name (it names itself otherwise)" aria-label=Name>
      <input id=mdl class=field placeholder="model (the agent's default)" aria-label=Model autocapitalize=off autocorrect=off spellcheck=false>
      <input id=eff class=field placeholder="effort (the agent's default)" aria-label=Effort autocapitalize=off autocorrect=off spellcheck=false></div></details>
    <div class=err-line id=mkerr role=alert></div>
    <div class="line end"><span class="note grow">${fine.matches ? `${mac ? '⌘' : 'ctrl'} enter starts it` : ''}</span><a class=key href=#/>cancel</a><button class="key key-on" id=mk>start</button></div></form>`;
  // opened straight onto this screen, the folders aren't known yet
  if (!lastList.length && !again) api('/sessions').then(r => r.json()).then(l => { if (sc.live && l.length && !$('#pr').value && !$('#cwd').value) { lastList = l; newScreen(true); } }, () => {});
  let agent = '';
  $('#ags').onclick = e => { const b = e.target.closest('[data-a]'); if (!b) return; agent = b.dataset.a; $$('#ags button').forEach(x => x.setAttribute('aria-checked', x === b)); };
  if ($('#dirs')) $('#dirs').onclick = e => { const b = e.target.closest('[data-d]'); if (b) { $('#cwd').value = b.dataset.d; $('#pr').focus(); } };
  if (fine.matches) (dirs.length ? $('#pr') : $('#cwd')).focus();
  $('#mkform').onkeydown = e => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); $('#mkform').requestSubmit(); } };
  $('#mkform').onsubmit = async e => {
    e.preventDefault();
    const b = $('#mk'), cwd = $('#cwd').value.trim();
    b.disabled = true; b.classList.add('busy'); $('#mkerr').textContent = '';
    try {
      const v = await (await post('/sessions', {cwd, agent, prompt: $('#pr').value, name: $('#nm').value.trim(), model: $('#mdl').value.trim(), effort: $('#eff').value.trim()})).json();
      store.set('cwd.' + machine, v.cwd || cwd); newIn = '';
      go(sessionHash(v.id));
    } catch (err) {
      if (err === 401) return signIn();
      b.disabled = false; b.classList.remove('busy');
      $('#mkerr').textContent = String(err);
    }
  };
}

// ── settings, and the devices paired ──
function setTheme(t) {
  try { t ? localStorage.setItem('rush.theme', t) : localStorage.removeItem('rush.theme'); } catch {}
  t ? document.documentElement.dataset.theme = t : delete document.documentElement.dataset.theme;
  themeColor();
}
const themeNow = () => document.documentElement.dataset.theme || '';
// the browser's own chrome takes the page's ground
function themeColor() { const g = getComputedStyle(document.documentElement).getPropertyValue('--ground').trim().split(/\s+/); $('meta[name=theme-color]').content = `rgb(${g.join(',')})`; }
matchMedia('(prefers-color-scheme: light)').addEventListener('change', themeColor);
let installer = null;
addEventListener('beforeinstallprompt', e => { e.preventDefault(); installer = e; });
const standalone = () => matchMedia('(display-mode: standalone)').matches || navigator.standalone === true;

function devicesScreen() {
  const sc = newScope();
  bar({title: 'Settings', sub: `<span>this device, and ${esc(machine)}</span>`});
  const ios = /iPhone|iPad/.test(navigator.userAgent);
  $('#main').innerHTML = `<div class=page>
    <fieldset class=box><legend>appearance</legend><div class=seg id=theme role=radiogroup aria-label=Theme>${[['', 'as the system'], ['dark', 'dark'], ['light', 'light']].map(([v, w]) => `<button class=key role=radio aria-checked=${themeNow() === v} data-th="${v}">${w}</button>`).join('')}</div></fieldset>
    <fieldset class=box><legend>notifications</legend><p class=c-sub id=pushsay>…</p><div class=line><button class=key id=pushon hidden>turn them on</button></div></fieldset>
    ${standalone() ? '' : `<fieldset class=box><legend>on your home screen</legend><p class=c-sub>${ios ? 'In Safari: Share, then <b>Add to Home Screen</b>. Rush opens full screen from there, and can notify you.' : 'Rush installs as an app of its own, with no browser around it: Install, or Add to Home screen, in the browser\'s menu.'}</p>${ios ? '' : '<div class=line><button class=key id=install hidden>install rush</button></div>'}</fieldset>`}
    <fieldset class=box><legend>devices paired with ${esc(machine)} <span id=devn></span></legend><div id=devlist><div class=skeleton></div></div>
      <div class=line><button class=key id=pairnew><span class=c-orange>+</span> pair another device</button></div></fieldset>
    <fieldset class=box><legend>this device</legend><div class=line><button class="key key-bad" id=signout>sign out</button><button class="key wide-only" id=keys2>keyboard shortcuts</button></div></fieldset></div>`;
  $('#theme').onclick = e => { const b = e.target.closest('[data-th]'); if (!b) return; setTheme(b.dataset.th); $$('#theme button').forEach(x => x.setAttribute('aria-checked', x === b)); };
  $('#keys2').onclick = shortcuts;
  if ($('#install') && installer) { $('#install').hidden = false; $('#install').onclick = async () => { installer.prompt(); await installer.userChoice; installer = null; $('#install').hidden = true; }; }

  // notifications: what stands between you and them, said plainly
  const push = async () => {
    const say = $('#pushsay'), on = $('#pushon');
    if (!sc.live) return;
    on.hidden = true;
    if (!('PushManager' in window) || !('Notification' in window)) return say.textContent = ios && !standalone() ? 'Add Rush to your Home Screen first; iOS notifies only from there.' : "This browser can't show notifications.";
    if (Notification.permission === 'denied') return say.textContent = "They're blocked for this site in the browser's settings.";
    const reg = await navigator.serviceWorker.getRegistration(), sub = reg && await reg.pushManager.getSubscription();
    if (!sc.live) return;
    if (sub && Notification.permission === 'granted') { say.innerHTML = '<span class=c-green>● on</span> for this device: when an agent needs an approval or an answer, or fails.'; on.hidden = false; on.textContent = 'send them here again'; }
    else { say.textContent = 'Off. Rush can tell you when an agent needs an approval or an answer, or fails.'; on.hidden = false; on.textContent = 'turn them on'; }
  };
  $('#pushon').onclick = async () => { await subscribe(); push(); };
  push();

  const load = () => api('/devices').then(r => r.json()).then(l => {
    if (!sc.live) return;
    l = l || [];
    const box = $('#devlist');
    if (!box.querySelector('[data-key]')) box.innerHTML = '';
    set($('#devn'), '', String(l.length));
    keyed(box, l, d => d.id,
      d => el(`<div class=row data-state=${d.live ? 'idle' : 'stopped'}><div class=row-link><div class=row-top><span class=mark aria-hidden=true>${d.kind === 'ssh' ? '❯' : '◰'}</span><b></b><span class=me></span><button class="key key-plain key-bad" data-revoke>unpair</button></div><div class=row-sub><span class=tree aria-hidden=true>└</span><span class=why></span></div></div></div>`),
      (e, d) => {
        set(e, 'b', d.name);
        set(e, '.me', d.id === me ? '<span class=c-orange>this device</span>' : '', true);
        set(e, '.why', (d.live ? '<span class=c-green>● connected</span>' : `<span>seen ${esc(ago(d.lastSeen) || 'never')}${['now', ''].includes(ago(d.lastSeen)) ? '' : ' ago'}</span>`) +
          ` · <span>${d.kind === 'ssh' ? 'SSH key' : 'browser'} · paired ${esc(day(d.created))}</span>`, true);
        $('[data-revoke]', e).dataset.revoke = d.id;
      });
  }, e => { if (sc.live) e === 401 ? signIn() : $('#devlist').innerHTML = `<div class=err-line>${esc(e)}</div>`; });
  load();
  const t = setInterval(load, 10e3);
  sc.on(() => clearInterval(t));
  $('#devlist').onclick = async e => {
    const b = e.target.closest('[data-revoke]');
    if (!b) return;
    const name = $('b', b.closest('.row')).textContent, self = b.dataset.revoke === me;
    if (!await confirmSheet('Unpair ' + name, self ? 'This device is signed out at once.' : 'It is signed out at once, and anything it has open ends.', 'unpair', true)) return;
    try { await post(`/devices/${enc(b.dataset.revoke)}/revoke`, {}); if (self) return location.replace(location.pathname); load(); toast('unpaired ' + name); }
    catch (err) { failed(err); }
  };
  // a code for another device, as rush's own Pair a device shows one
  $('#pairnew').onclick = async () => {
    let p;
    try { p = await (await post('/pair', {})).json(); } catch (err) { return failed(err); }
    const d = sheet(`<p class=c-sub>Type this on the other device's sign-in${p.url ? ', or open the link there' : ''}. It works once${zero(p.expires) ? '' : `, until ${esc(clock(p.expires))}`}.</p>
      <div class=pair-code>${esc(p.code)}</div>
      ${p.url ? `<div class=line><span class="grow cut c-blue">${esc(p.url)}</span></div>` : ''}
      <div class="line end"><button class=key data-copy="${esc(p.code)}">copy the code</button>${p.url ? `<button class=key data-copy="${esc(p.url)}">copy the link</button>${navigator.share ? '<button class="key key-on" id=share>share</button>' : ''}` : ''}</div>
      ${p.ssh ? `<fieldset class=box><legend>or over SSH</legend><div class=code><div class=code-head><span>sh</span><button type=button data-copy>copy</button></div><pre>${esc(p.ssh)}</pre></div>${p.hostKey ? `<p class=note>host key ${esc(p.hostKey)}</p>` : ''}</fieldset>` : ''}`, {title: 'Pair another device'});
    if ($('#share', d)) $('#share', d).onclick = () => navigator.share({title: 'Rush', url: p.url}).catch(() => {});
    d.addEventListener('close', load);
  };
  $('#signout').onclick = async () => {
    if (!await confirmSheet('Sign out', "You'll need a new pairing code, or the token, to sign in again.", 'sign out', true)) return;
    if (me) await post(`/devices/${enc(me)}/revoke`, {}).catch(() => {});
    await fetch('/api/logout', {method: 'POST'}).catch(() => {});
    location.replace(location.pathname); // the app afresh, from its start: the sign-in
  };
}

async function subscribe() {
  try {
    const k = await (await api('/push')).json();
    if (await Notification.requestPermission() !== 'granted') return;
    const reg = await navigator.serviceWorker.ready;
    const sub = await reg.pushManager.subscribe({userVisibleOnly: true,
      applicationServerKey: Uint8Array.from(atob(k.key.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0))});
    await post('/push', sub.toJSON()); toast('notifications on');
  } catch (e) { toast('Notifications: ' + (e === 401 ? 'sign in again' : e.message || e), 'bad'); }
}

// ── the palette, and the keys ──
function palette() {
  if ($('dialog.palette')) return;
  const d = el(`<dialog class=palette aria-label="Go to anything"><input type=text placeholder="Go to a session, or do something" aria-label=Search autocapitalize=off autocorrect=off spellcheck=false role=combobox aria-expanded=true aria-controls=palist><ul id=palist role=listbox></ul></dialog>`);
  const items = [
    ...view.acts,
    {name: 'New session', hint: 'n', run: () => go('#/new')},
    {name: 'Settings and devices', run: () => go('#/devices')},
    {name: themeNow() === 'light' ? 'Dark theme' : 'Light theme', run: () => setTheme(themeNow() === 'light' ? 'dark' : 'light')},
    {name: 'Keyboard shortcuts', hint: '?', run: shortcuts},
    ...machines.filter(m => m.name !== machine).map(m => ({name: 'Switch to ' + m.name, run: () => { setMachine(m.name); go('#/'); syncList(true); }})),
    ...lastList.map(i => ({...i, st: stateOf(i)})).sort((a, b) => groups.findIndex(g => g[0] === groupOf(a.st)) - groups.findIndex(g => g[0] === groupOf(b.st)) || (a.updatedAt < b.updatedAt ? 1 : -1))
      .map(i => ({name: i.name || i.id, mark: `<span class="st-${i.st}">${marks[i.st] || '·'}</span>`, hint: project(i.cwd).name, more: `${i.cwd} ${agentName(i.kind)} ${i.st}`, run: () => go(sessionHash(i.id))})),
  ];
  let shown = [], at = 0;
  const draw = () => {
    const words = $('input', d).value.toLowerCase().split(/\s+/).filter(Boolean);
    shown = items.filter(i => words.every(w => `${i.name} ${i.hint || ''} ${i.more || ''}`.toLowerCase().includes(w))).slice(0, 60);
    at = Math.min(at, Math.max(0, shown.length - 1));
    $('ul', d).innerHTML = shown.map((i, n) => `<li role=option aria-selected=${n === at} data-n=${n}>${i.mark || '<span class=c-orange>❯</span>'}<span class=cut>${esc(i.name)}</span><small>${esc(i.hint || '')}</small></li>`).join('') || '<li class=none>Nothing by that name.</li>';
    $('[aria-selected=true]', d)?.scrollIntoView({block: 'nearest'});
  };
  const run = n => { const i = shown[n]; d.close(); if (i) i.run(); };
  $('input', d).oninput = () => { at = 0; draw(); };
  d.onkeydown = e => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); at = (at + (e.key === 'ArrowDown' ? 1 : shown.length - 1)) % Math.max(1, shown.length); draw(); }
    if (e.key === 'Enter') { e.preventDefault(); run(at); }
  };
  d.onclick = e => { const li = e.target.closest('li[data-n]'); if (li) run(+li.dataset.n); else if (e.target === d) d.close(); };
  d.addEventListener('close', () => d.remove());
  document.body.append(d); d.showModal(); draw();
}
$('#pal').onclick = palette;
function shortcuts() {
  const mod = mac ? '⌘' : 'ctrl';
  sheet(`<dl class=keys-list>${[
    [`${mod} K`, 'go to a session, or do something'], ['/', 'search the sessions'], ['n', 'a new session'], ['j  k', 'down and up the list; enter opens'],
    ['alt ↓  alt ↑', 'the next session, the one before'], ['esc', 'back to the sessions; out of a box'], ['enter', 'send (shift enter for a new line)'], [`${mod} enter`, 'steer: into the turn under way'],
    ['ctrl C', 'stop the turn, when the box is empty'], ['alt A  alt D', 'allow, deny the approval waiting'], ['c', 'changes'], ['i', 'details'], ['e', 'into the message box'], ['?', 'these'],
  ].map(([k, w]) => `<dt>${k.split('  ').map(x => `<kbd>${esc(x)}</kbd>`).join(' ')}</dt><dd>${esc(w)}</dd>`).join('')}</dl>`, {title: 'Keyboard shortcuts', sc: null});
}
$('#keys').onclick = shortcuts;
addEventListener('keydown', e => {
  if (!machines.length || e.defaultPrevented) return;
  const mod = e.metaKey || e.ctrlKey, t = e.target, typing = t.matches('input, textarea, select, [contenteditable]');
  if (mod && e.key.toLowerCase() === 'k') { e.preventDefault(); return palette(); }
  if ($('dialog[open]')) return;
  if (e.altKey && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) { e.preventDefault(); return stepSession(e.key === 'ArrowDown' ? 1 : -1); }
  if (e.altKey && view.keys['alt+' + e.code]) { e.preventDefault(); return view.keys['alt+' + e.code](); }
  if (e.key === 'Escape') { if (typing) return t.blur(); if ($('#app').dataset.route !== 'home') return go('#/'); }
  if (typing || mod || e.altKey) return;
  const k = e.key;
  if (k === '/') { e.preventDefault(); if (!wide() && $('#app').dataset.route !== 'home') go('#/'); return $('#q').focus(); }
  if (k === '?') return shortcuts();
  if (k === 'n') { e.preventDefault(); return go('#/new'); }
  if (k === 'j' || (k === 'ArrowDown' && $('#side').contains(t))) { e.preventDefault(); return stepRow(1); }
  if (k === 'k' || (k === 'ArrowUp' && $('#side').contains(t))) { e.preventDefault(); return stepRow(-1); }
  if (view.keys[k]) { e.preventDefault(); view.keys[k](); }
});

// picture is an image file as a send carries it. A type an agent can't take
// (a phone's HEIC, say) or a big one is redrawn as a JPEG at most 2000px.
const takes = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
async function picture(f) {
  let blob = f;
  if (!takes.includes(f.type) || f.size > 4.5e6) {
    const bm = await createImageBitmap(f), k = Math.min(1, 2000 / Math.max(bm.width, bm.height));
    const c = document.createElement('canvas'); c.width = Math.round(bm.width * k); c.height = Math.round(bm.height * k);
    c.getContext('2d').drawImage(bm, 0, 0, c.width, c.height);
    blob = await new Promise((ok, no) => c.toBlob(b => b ? ok(b) : no('can\'t be read'), 'image/jpeg', 0.85));
  }
  const url = await new Promise((ok, no) => { const r = new FileReader(); r.onload = () => ok(r.result); r.onerror = () => no(r.error); r.readAsDataURL(blob); });
  return {type: blob.type, data: url.slice(url.indexOf(',') + 1)};
}

themeColor();
start();
