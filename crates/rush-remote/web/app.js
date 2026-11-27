'use strict';
// Rush's web app: your sessions on this machine (and its peers), from a
// phone or a browser, in rush's own look. Plain JS, no build but the CSS
// (webstyle/app.css).
//
// Routes are the hash, and the hash alone says what's on screen: #/ the
// sessions, #/s/<machine>/<id> one, #/new, #/devices. Each screen is a
// scope: leaving it ends its streams and timers, and anything still in
// flight from it is dropped, so nothing late ever draws over the screen
// you're on. Lists update rows in place.

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));
const el = html => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstElementChild; };

let machines = [], machine = '', base = '', me = '', lastList = [];

// api: GET reads are safe to repeat; writes are sent once and never retried
// (a send that was delivered but unanswered would be sent twice).
const api = (p, o) => fetch(base + '/api' + p, o).then(async r => {
  if (r.status === 401) throw 401;
  if (!r.ok) throw ((await r.json().catch(() => ({}))).error || r.statusText);
  return r;
});
const post = (p, b) => api(p, {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(b)});
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js');

// the rush bottle, as the header draws it, a pixel to a cell
const bottle = (cls = 'bottle') => `<svg class="${cls}" viewBox="0 0 6 10" shape-rendering="crispEdges" aria-hidden="true">
  <rect width="6" height="1" fill="#707074"/><rect y="1" width="6" height="1" fill="#404044"/><rect y="2" width="6" height="1" fill="#58585c"/>
  <rect x="1" y="3" width="4" height="1" fill="#9a4c14"/><rect y="4" width="6" height="1" fill="#9a4c14"/><rect y="5" width="6" height="3" fill="#fad61e"/>
  <rect x="1" y="6" width="4" height="1" fill="#d61c18"/><rect y="8" width="6" height="1" fill="#9a4c14"/><rect y="9" width="6" height="1" fill="#542606"/></svg>`;

// ── words ──
const ago = t => {
  if (!t || t.startsWith('0001')) return '';
  const s = (Date.now() - new Date(t)) / 1000;
  if (s < 45) return 'now';
  if (s < 3600) return Math.round(s / 60) + 'm';
  if (s < 86400) return Math.round(s / 3600) + 'h';
  if (s < 7 * 86400) return Math.round(s / 86400) + 'd';
  return new Date(t).toLocaleDateString([], {day: 'numeric', month: 'short'});
};
const home = p => String(p || '').replace(/^\/(Users|home)\/[^/]+/, '~');
const money = n => n ? '$' + n.toFixed(2) : '';
const agentName = k => ({'': 'Claude', claude: 'Claude', codex: 'Codex', gemini: 'Gemini', kimi: 'Kimi', antigravity: 'Antigravity'}[k || ''] || k);
const stateOf = i => i.alive === false ? (i.sleeping ? 'asleep' : 'stopped') : (i.state || 'idle');
const marks = {blocked: '●', working: '✻', starting: '✻', idle: '◆', asleep: '◇', stopped: '·'};
const stateWords = (st, needs) => st === 'blocked' ? 'needs ' + (needs || 'you') : st;
const state = (st, needs) => `<span class="st-${esc(st)}">${marks[st] || '·'} ${esc(stateWords(st, needs))}</span>`;
// md is the little markdown agents write most: code blocks, code, bold
const md = s => esc(s)
  .replace(/```[\w-]*\n?([\s\S]*?)```/g, (_, c) => `<pre>${c}</pre>`)
  .replace(/`([^`\n]+)`/g, '<code>$1</code>')
  .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>');

// ── screens ──
let scope = null;
function newScope() {
  if (scope) scope.end();
  const ends = [];
  const s = {live: true, on: f => ends.push(f), end() { s.live = false; ends.splice(0).forEach(f => { try { f(); } catch {} }); }};
  return scope = s;
}
function bar({title = 'rush', sub = '', back = false, actions = []}) {
  $('#title').innerHTML = title;
  $('#sub').innerHTML = sub;
  $('#back').innerHTML = back ? '‹' : bottle();
  $('#back').classList.toggle('key-plain', true);
  $('#back').disabled = !back;
  $('#actions').replaceChildren(...actions);
}
const action = (id, words, onclick) => { const b = el(`<button class="key key-plain" id="${id}">${words}</button>`); b.onclick = onclick; return b; };
$('#back').onclick = () => go('#/');
const go = h => { if (location.hash === h) route(); else location.hash = h; };
// the session screen on screen, if its scope is still live, for its tabs
// to switch in place
let curTab = null;

function route() {
  if (!machines.length) return;
  const h = location.hash;
  let m;
  $('#main').style.paddingBottom = '';
  if ((m = h.match(/^#\/s\/([^/]+)\/([^/]+)(?:\/(all|agents|shells|changes|overview))?$/))) {
    setMachine(decodeURIComponent(m[1]));
    sideFor(decodeURIComponent(m[2]));
    const tab = m[3] && m[3] !== 'all' ? m[3] : 'c';
    // the same session's tab is a switch in place, not a rebuild: its
    // stream keeps running under whichever tab is shown
    if (curTab?.id === m[2] && curTab.live) return curTab.set(tab);
    return sessionScreen(m[2], tab);
  }
  sideOff();
  if (h === '#/new') return newScreen();
  if (h === '#/devices') return devicesScreen();
  if (h === '#/fleet') return fleetScreen();
  homeScreen();
}
addEventListener('hashchange', route);

// ── the sidebar: on a wide screen a session keeps the list beside it, the
// one you're in marked, kept current from the machine's list stream ──
const wideQ = matchMedia('(min-width: 1100px)');
const side = {es: null, timer: 0, mach: '', list: [], cur: ''};
// the session on screen hears the list too: its name and state as they change
let onList = null;
function sideFor(id) {
  side.cur = id;
  document.body.classList.toggle('with-side', wideQ.matches);
  if (!wideQ.matches) return;
  if (side.mach !== machine || (!side.es && !side.timer)) {
    if (side.es) side.es.close();
    clearInterval(side.timer);
    side.es = null; side.timer = 0; side.mach = machine; side.list = [];
    let got = false;
    const take = l => { side.list = l; lastList = l; sideDraw(); onList && onList(l); };
    const es = side.es = new EventSource(base + '/api/sessions/stream');
    es.onmessage = m => { got = true; const v = JSON.parse(m.data); if (v.sessions) take(v.sessions); };
    // a peer whose serve can't stream is asked every 4s
    es.onerror = () => {
      if (got || side.timer) return;
      es.close(); side.es = null;
      const poll = () => api('/sessions').then(r => r.json()).then(take, () => {});
      poll(); side.timer = setInterval(poll, 4000);
    };
  }
  sideDraw();
}
function sideOff() { side.cur = ''; document.body.classList.remove('with-side'); }
function sideDraw() {
  const box = $('#sidelist');
  if (!box || !document.body.classList.contains('with-side')) return;
  const ss = side.list.map(i => ({...i, st: stateOf(i), mach: side.mach}));
  keyed(box, sessionRows(ss), r => r.k, rowEl, rowSet);
  $$('a[data-id]', box).forEach(e => e.classList.toggle('row-cur', e.dataset.id === side.cur));
  set($('#sidesub'), '', [machines.length > 1 ? esc(side.mach) : '', ...tally(ss)].filter(Boolean).join(' '), true);
}
wideQ.addEventListener('change', () => { if (side.cur) sideFor(side.cur); });

function setMachine(name) {
  const m = machines.find(x => x.name === name) || machines[0];
  machine = m.name; base = m.self ? '' : '/m/' + encodeURIComponent(m.name);
}

// ── toasts, sheets ──
function toast(text, kind) {
  const t = el(`<div class=toast data-kind="${kind || ''}" role=status>${esc(text)}</div>`);
  $('#toasts').append(t);
  setTimeout(() => t.remove(), kind === 'bad' ? 7000 : 3500);
}
function sheet(html, id) {
  const d = el(`<dialog class=sheet${id ? ` id=${id}` : ''}><div class=sheet-body>${html}</div></dialog>`);
  document.body.append(d);
  d.addEventListener('close', () => d.remove());
  d.addEventListener('click', e => { if (e.target === d) d.close(); });
  d.showModal();
  scope && scope.on(() => d.open && d.close());
  return d;
}
function confirmSheet(title, text, ok, danger) {
  return new Promise(res => {
    const d = sheet(`<fieldset class=box><legend>${esc(title)}</legend><p class="text-sub py-1">${esc(text)}</p>
      <div class="flex gap-1.5 justify-end pt-2"><button class=key id=cancel>cancel</button><button class="key ${danger ? 'key-bad' : 'key-on'}" id=ok>${esc(ok)}</button></div></fieldset>`);
    let v = false;
    $('#ok', d).onclick = () => { v = true; d.close(); };
    $('#cancel', d).onclick = () => d.close();
    d.addEventListener('close', () => res(v));
  });
}

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
  $('#sidehome').innerHTML = bottle() + '<b>rush</b>';
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
    machines = await r.json();
  } catch (e) { return e === 401 ? signIn() : offline(e); }
  setMachine(machine || machines[0].name);
  route();
}
function offline(e) {
  newScope(); bar({});
  $('#main').innerHTML = `<div class=empty><p class=text-red>can't reach rush</p><p>${esc(e)}</p><p><button class=key id=retry>try again</button></p></div>`;
  $('#retry').onclick = start;
}

function signIn() {
  newScope(); machines = []; bar({});
  $('#main').innerHTML = `<div class="px-3 pt-10 pb-10 max-w-md mx-auto space-y-5">
    <div class="flex items-center gap-3">${bottle('h-14 w-[34px]')}<div><b class="text-[18px]">rush</b><p class=text-dim>your agents, anywhere on your tailnet</p></div></div>
    <fieldset class=box><legend>pair this device</legend><p class=text-sub>In rush on your Mac: <b>Settings › Remote › Pair a device</b>, then scan its code with this phone's camera.</p></fieldset>
    <form id=signin><fieldset class="box space-y-2"><legend>or sign in with its token · <span class=text-dim>rush remote token</span></legend>
      <input id=tok class=field type=password autocomplete=current-password placeholder="token">
      <div class="flex justify-end"><button class="key key-on" id=go>sign in</button></div></fieldset></form></div>`;
  $('#signin').onsubmit = async e => {
    e.preventDefault();
    const r = await fetch('/api/login', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({token: $('#tok').value})}).catch(() => null);
    if (r && r.ok) { history.replaceState(null, '', location.pathname + '#/'); start(); } else toast("That isn't this machine's token.", 'bad');
  };
}
const failed = e => e === 401 ? signIn() : toast(String(e), 'bad');

// ── the sessions ──
const groups = [['blocked', 'Needs you'], ['working', 'Working'], ['idle', 'Idle'], ['asleep', 'Asleep'], ['stopped', 'Stopped']];
const groupOf = st => st === 'starting' ? 'working' : groups.some(g => g[0] === st) ? st : 'idle';
const atWork = st => st === 'working' || st === 'starting';
// whose run a session is: the session that spawned it (a subagent rush
// hosts), as the TUI's list nests them
const parentOf = i => (i.meta && (i.meta.spawnedBy || '')) || '';
// sessionRows is a list of sessions as the lists draw it: a rule per state
// (its count and spend), each top-level session newest first, its runs under
// it. ss are sessions with st (stateOf) and mach (their machine) set.
function sessionRows(ss, {byMachine = false} = {}) {
  const ids = new Set(ss.map(i => i.mach + '/' + i.id));
  const kids = new Map();
  for (const i of ss) {
    const p = parentOf(i);
    if (p && ids.has(i.mach + '/' + p)) { const k = i.mach + '/' + p; kids.set(k, [...(kids.get(k) || []), i]); }
  }
  const top = ss.filter(i => !(parentOf(i) && ids.has(i.mach + '/' + parentOf(i))));
  const newest = (a, b) => (b.updatedAt || '') > (a.updatedAt || '') ? 1 : -1;
  const rows = [];
  const add = (i, depth) => {
    const ks = (kids.get(i.mach + '/' + i.id) || []).sort((a, b) => atWork(b.st) - atWork(a.st) || newest(a, b));
    rows.push({k: i.mach + '/' + i.id, i, depth, runs: ks.filter(x => atWork(x.st)).length, byMachine});
    if (depth < 3) ks.forEach((x, n) => add({...x, last: n === ks.length - 1}, depth + 1));
  };
  for (const [g, name] of groups) {
    const in_ = top.filter(i => groupOf(i.st) === g).sort(newest);
    if (!in_.length) continue;
    const cost = in_.reduce((n, i) => n + (i.costUsd || 0), 0);
    rows.push({k: 'h:' + g, hdr: g, name, n: in_.length, cost});
    in_.forEach(i => add(i, 0));
  }
  return rows;
}
const rowEl = r => r.hdr ? el('<div class=rule><b></b><span></span></div>')
  : el(`<a class=row><div class=row-top><span class=tree></span><span class=mark></span><b></b><span class=mach></span><span class=cost></span><time></time></div><div class=row-sub><span class=tree></span><span class=why></span></div></a>`);
const rowSet = (e, r) => {
  if (r.hdr) { set(e, 'b', '▾ ' + r.name); set(e, 'span', `${r.n}${r.cost ? ' · ' + money(r.cost) : ''}`); return; }
  const i = r.i, sub = r.depth > 0;
  e.href = `#/s/${encodeURIComponent(i.mach)}/${encodeURIComponent(i.id)}`;
  e.dataset.id = i.id; e.dataset.state = i.st; e.dataset.mach = i.mach;
  e.classList.toggle('row-run', sub);
  const pad = '\u2003'.repeat(Math.max(0, r.depth - 1));
  set($('.row-top .tree', e), '', sub ? pad + (i.last ? '└' : '├') : '');
  set(e, '.mark', marks[i.st] || '·');
  set(e, 'b', i.name || i.id);
  set(e, '.mach', r.byMachine ? i.mach : '');
  set(e, '.cost', money(i.costUsd));
  const t = $('time', e); t.dataset.t = i.updatedAt || ''; set(t, '', ago(i.updatedAt));
  set($('.row-sub .tree', e), '', sub ? pad + (i.last ? '\u2003' : '│') + '\u2003' : '└');
  const who = esc([agentName(i.kind), i.model].filter(Boolean).join(' · '));
  const runs = r.runs ? ` <span class=text-queue>⇉ ${r.runs} run${r.runs > 1 ? 's' : ''} working</span>` : '';
  let why;
  if (i.st === 'blocked') why = `<span class=text-yellow>${esc(stateWords(i.st, i.needs))}</span>`;
  else if (atWork(i.st)) why = `<span class=text-orange>${esc(i.detail || 'working…')}</span>`;
  else if (sub) why = `<span class=text-faint>${who}</span>`;
  else why = `<span class=text-blue>${esc(home(i.cwd))}</span> <span class=text-faint>· ${who}</span>`;
  if (sub && (i.st === 'blocked' || atWork(i.st))) why += ` <span class=text-faint>· ${who}</span>`;
  set(e, '.why', why + runs, true);
};
const tally = ss => {
  const n = ss.filter(i => i.st === 'blocked').length, w = ss.filter(i => atWork(i.st) && !parentOf(i)).length, r = ss.filter(i => atWork(i.st) && parentOf(i)).length;
  return [n && `<span class=text-yellow>● ${n} need${n > 1 ? '' : 's'} you</span>`, w && `<span class=text-orange>✻ ${w} working</span>`, r && `<span class=text-queue>⇉ ${r} run${r > 1 ? 's' : ''}</span>`].filter(Boolean);
};
function homeScreen() {
  const sc = newScope();
  const acts = [];
  if (machines.length > 1) {
    const s = el(`<select id=mach class="key" aria-label=Machine>${machines.map(m => `<option value="${esc(m.name)}">${esc(m.name)}</option>`).join('')}</select>`);
    s.value = machine; s.onchange = () => { setMachine(s.value); homeScreen(); };
    acts.push(s);
  }
  if ('PushManager' in window) acts.push(action('bell', 'notify', subscribe));
  acts.push(action('fleet', 'fleet', () => go('#/fleet')), action('devs', 'devices', () => go('#/devices')), action('new', '<span class=text-orange>+</span> new', () => go('#/new')));
  bar({title: machines.length > 1 ? esc(machine) : 'rush', actions: acts});
  $('#main').innerHTML = `<div id=list>${'<div class=row><div class="skeleton h-4 w-2/3 my-0.5"></div><div class="skeleton h-3 w-1/3 ml-[2ch] mt-1.5"></div></div>'.repeat(3)}</div><div id=oth></div>`;
  const draw = l => {
    if (!sc.live) return;
    lastList = l;
    const list = $('#list');
    if (!l.length) { list.innerHTML = `<div class=empty><div class="flex justify-center">${bottle('h-12 w-[29px] opacity-60')}</div><p>no sessions yet</p><p class=text-faint>start one with + new, or carry one on from below</p></div>`; set($('#sub'), '', ''); return; }
    if (!list.querySelector('[data-key]')) list.innerHTML = '';
    const ss = l.map(i => ({...i, st: stateOf(i), mach: machine}));
    keyed(list, sessionRows(ss), r => r.k, rowEl, rowSet);
    set($('#sub'), '', tally(ss).join(' '), true);
  };
  // the list as serve streams it; a peer whose serve can't stream is asked every 4s
  let got = false, timer = 0;
  const poll = () => api('/sessions').then(r => r.json()).then(draw, e => sc.live && e === 401 && signIn());
  const es = new EventSource(base + '/api/sessions/stream');
  es.onmessage = m => { got = true; const v = JSON.parse(m.data); if (v.me) me = v.me; if (v.sessions) draw(v.sessions); };
  es.onerror = () => { if (!got && !timer) { es.close(); poll(); timer = setInterval(poll, 4000); } };
  sc.on(() => { es.close(); clearInterval(timer); });

  // the machine's other sessions, its agents' own that Rush didn't start: a
  // past one carries on in Rush when picked
  let oth = [], all = false;
  const others = () => api('/others').then(r => r.json()).then(v => { if (sc.live) { oth = v || []; drawOthers(); } }, () => {});
  const drawOthers = () => {
    const box = $('#oth');
    if (!oth.length) { box.innerHTML = ''; return; }
    if (!$('#othlist', box)) box.innerHTML = `<div class=rule><b>▾ Not started by Rush</b><span id=othn></span></div><div id=othlist></div><div id=othmore class="px-3 py-2"></div>`;
    set($('#othn'), '', String(oth.length));
    keyed($('#othlist'), all ? oth : oth.slice(0, 20), o => o.sessionId,
      () => el(`<button class=row data-state=stopped><div class=row-top><span class=mark>·</span><b></b><time></time></div><div class=row-sub><span class=tree>└</span><span class=why></span></div></button>`),
      (e, o) => {
        e.dataset.r = o.live ? '' : o.sessionId;
        set(e, 'b', o.name || o.sessionId);
        const t = $('time', e); t.dataset.t = o.updatedAt || ''; set(t, '', ago(o.updatedAt));
        set(e, '.why', `<span class=text-blue>${esc(home(o.cwd))}</span> <span class=text-faint>· ${esc([agentName(o.agent), o.account].filter(Boolean).join(' · '))}${o.live ? ' · in a terminal' : ''}</span>`, true);
      });
    $('#othmore').innerHTML = !all && oth.length > 20 ? `<button class="key key-plain text-dim" id=more>↓ ${oth.length - 20} more</button>` : '';
  };
  others();
  const ot = setInterval(others, 60e3);
  sc.on(() => clearInterval(ot));
  $('#oth').onclick = async e => {
    if (e.target.closest('#more')) { all = true; return drawOthers(); }
    const b = e.target.closest('[data-r]');
    if (!b) return;
    if (!b.dataset.r) return toast('It runs in a terminal there; it can be carried on in Rush once it ends.');
    if (!await confirmSheet('carry it on in rush', `${$('b', b).textContent} carries on as a Rush session on ${machine}, with its own agent and folder.`, 'carry on')) return;
    try { const v = await (await post('/sessions', {resume: b.dataset.r})).json(); go(`#/s/${encodeURIComponent(machine)}/${v.id}`); }
    catch (err) { failed(err); }
  };
}

// ── a new session ──
function newScreen() {
  newScope();
  bar({title: 'new session', sub: 'on ' + esc(machine), back: true});
  const dirs = [...new Set(lastList.map(i => i.cwd).filter(Boolean))].slice(0, 20);
  $('#main').innerHTML = `<form id=mkform class="p-3 space-y-3">
    <fieldset class=box><legend>folder on ${esc(machine)}</legend><input id=cwd class=field list=dirs placeholder="~/src/project" autocapitalize=off autocorrect=off spellcheck=false>
      <datalist id=dirs>${dirs.map(d => `<option value="${esc(d)}">`).join('')}</datalist></fieldset>
    <fieldset class=box><legend>agent</legend><input id=ag class=field list=ags placeholder="its default" autocapitalize=off>
      <datalist id=ags>${['claude', 'codex', 'gemini', 'kimi'].map(a => `<option value="${a}">`).join('')}</datalist></fieldset>
    <fieldset class=box><legend>first message</legend><textarea id=pr class="field min-h-32" rows=5 placeholder="what should it do?"></textarea></fieldset>
    <div class="flex justify-end"><button class="key key-on" id=mk>start</button></div></form>`;
  $('#mkform').onsubmit = async e => {
    e.preventDefault();
    $('#mk').disabled = true;
    try { const v = await (await post('/sessions', {cwd: $('#cwd').value, agent: $('#ag').value, prompt: $('#pr').value})).json(); go(`#/s/${encodeURIComponent(machine)}/${v.id}`); }
    catch (err) { failed(err); $('#mk').disabled = false; }
  };
}

// ── devices ──
function devicesScreen() {
  const sc = newScope();
  bar({title: 'devices', sub: 'paired with ' + esc(machine), back: true});
  $('#main').innerHTML = `<div class=rule><b>▾ Paired</b><span id=devn></span></div><div id=devlist></div>
    <p class="px-3 py-3 text-dim">Pair another in rush on that machine: Settings › Remote › Pair a device. A phone scans its code; an SSH app types it when asked.</p>
    <div class="px-3 pb-10"><button class="key key-bad" id=signout>sign out of this device</button></div>`;
  const load = () => api('/devices').then(r => r.json()).then(l => {
    if (!sc.live) return;
    const box = $('#devlist');
    set($('#devn'), '', String(l.length));
    keyed(box, l, d => d.id,
      d => el(`<div class=row><div class=row-top><span class="mark text-dim">${d.kind === 'ssh' ? '❯' : '◰'}</span><b></b><span class=me></span><button class="key key-plain key-bad h-6" data-revoke>unpair</button></div><div class=row-sub><span class=tree>└</span><span class=why></span></div></div>`),
      (e, d) => {
        set(e, 'b', d.name);
        set(e, '.me', d.id === me ? '<span class=text-orange>this device</span>' : '', true);
        set(e, '.why', (d.live ? '<span class=text-green>● connected</span>' : `<span>seen ${esc(ago(d.lastSeen) || 'never')}${['now', ''].includes(ago(d.lastSeen)) ? '' : ' ago'}</span>`) +
          ` <span class=text-faint>· ${d.kind === 'ssh' ? 'SSH key' : 'browser'} · paired ${esc(new Date(d.created).toLocaleDateString([], {day: 'numeric', month: 'short'}))}</span>`, true);
        $('[data-revoke]', e).dataset.revoke = d.id;
      });
  }, e => sc.live && failed(e));
  load();
  const t = setInterval(load, 10e3);
  sc.on(() => clearInterval(t));
  $('#devlist').onclick = async e => {
    const b = e.target.closest('[data-revoke]');
    if (!b) return;
    const name = $('b', b.closest('.row')).textContent, self = b.dataset.revoke === me;
    if (!await confirmSheet('unpair ' + name, self ? 'This device is signed out at once.' : 'It is signed out at once, and anything it has open ends.', 'unpair', true)) return;
    try { await post(`/devices/${encodeURIComponent(b.dataset.revoke)}/revoke`, {}); if (self) return location.replace(location.pathname); load(); toast('unpaired ' + name); }
    catch (err) { failed(err); }
  };
  $('#signout').onclick = async () => {
    if (!await confirmSheet('sign out', "You'll need a new pairing code, or the token, to sign in again.", 'sign out', true)) return;
    if (me) await post(`/devices/${encodeURIComponent(me)}/revoke`, {}).catch(() => {});
    await fetch('/api/logout', {method: 'POST'}).catch(() => {});
    location.replace(location.pathname); // the app afresh, from its start: the sign-in
  };
}

// ── the fleet: every machine's sessions, and the feed ──
// api for a machine that isn't the picked one: the same reads through
// its own proxy prefix
const mapi = (m, p, o) => fetch((m.self ? '' : '/m/' + encodeURIComponent(m.name)) + '/api' + p, o).then(async r => {
  if (r.status === 401) throw 401;
  if (!r.ok) throw ((await r.json().catch(() => ({}))).error || r.statusText);
  return r;
});
function fleetScreen() {
  const sc = newScope();
  bar({title: 'fleet', back: true});
  // wide, the sessions and the feed sit side by side; narrow, a tab each
  const wide = matchMedia('(min-width: 1100px)').matches;
  $('#main').classList.add('main-wide');
  sc.on(() => $('#main').classList.remove('main-wide'));
  $('#main').innerHTML = `<nav class="tabs${wide ? ' hidden' : ''}" id=ftabs><button class="tab tab-on" data-t=sessions>sessions</button><button class=tab data-t=feed>feed</button></nav>
    <div class="${wide ? 'fleet-grid' : ''}"><section><div id=fstats class=stats></div><div id=fsess></div><div id=fgone></div></section>
    <section id=ffeedcol${wide ? '' : ' class=hidden'}>${wide ? '<div class=rule><b>▾ the feed</b><span></span></div>' : ''}<div id=ffeed></div></section></div>`;
  let tab = 'sessions';
  const showing = t => wide || tab === t;

  // every machine's sessions at a glance: what needs you first, then what's
  // at work (each with its runs under it), then the rest
  const draw = got => {
    const ss = [], gone = [];
    for (const {m, l} of got) {
      if (!l) { gone.push(m.name); continue; }
      ss.push(...l.map(i => ({...i, st: stateOf(i), mach: m.name})));
    }
    const multi = got.length > 1;
    keyed($('#fsess'), sessionRows(ss, {byMachine: multi}), r => r.k, rowEl, rowSet);
    if (!ss.length) $('#fsess').innerHTML = `<div class=empty><p>no sessions on ${multi ? 'any machine' : 'this machine'}</p></div>`;
    const top = ss.filter(i => !parentOf(i));
    const stat = (n, label, cls) => `<div class=stat><b class="${n ? cls : 'text-faint'}">${n}</b><span>${label}</span></div>`;
    const spent = ss.reduce((n, i) => n + (i.costUsd || 0), 0);
    set($('#fstats'), '', [
      stat(ss.filter(i => i.st === 'blocked').length, 'need you', 'text-yellow'),
      stat(top.filter(i => atWork(i.st)).length, 'working', 'text-orange'),
      stat(ss.filter(i => parentOf(i) && atWork(i.st)).length, 'runs working', 'text-queue'),
      stat(top.filter(i => i.st === 'idle').length, 'idle', 'text-green'),
      `<div class=stat><b class=text-bright>${money(spent) || '$0'}</b><span>spent</span></div>`].join(''), true);
    set($('#fgone'), '', gone.length ? `<p class="px-3 py-2 text-red">can't reach ${esc(gone.join(', '))}</p>` : '', true);
    set($('#sub'), '', [...tally(ss), `${top.length} on ${got.length - gone.length} machine${got.length - gone.length === 1 ? '' : 's'}`].join(' · '), true);
  };
  const poll = () => Promise.all(machines.map(m =>
    mapi(m, '/sessions').then(r => r.json()).then(l => ({m, l}), () => ({m, l: null}))
  )).then(got => { if (sc.live && showing('sessions')) draw(got); });
  poll();
  const pt = setInterval(poll, 4000);
  sc.on(() => clearInterval(pt));

  // the feed: the picked machine's board (rush feed), read and posted to
  let threads = [], feedOn = false;
  const who = a => esc(a.name || a.handle || (a.sessionId ? '@' + a.sessionId : 'someone'));
  const tags = s => esc(s).replace(/(^|\s)(#[\p{L}\p{N}][\p{L}\p{N}-]*)/gu, '$1<span class=text-orange>$2</span>');
  const proj = t => t.author.project ? ` <span class=text-faint>${esc('#' + t.author.project.split('/').pop().toLowerCase().replace(/[^a-z0-9]+/g, '-'))}</span>` : '';
  const drawFeed = () => {
    $('#ffeed').innerHTML = `${feedOn ? `<fieldset class=box id=fsay><legend>chirp as <b>you</b></legend>
        <textarea id=ftxt class=field rows=2 placeholder="what's happening; #hashtags sort it"></textarea>
        <div class="flex justify-end pt-2"><button class="key key-on" id=fpost>post</button></div></fieldset>`
        : `<p class="px-3 py-2 text-dim">the feed is off; the user turns it on with <b>#feed on</b></p>`}
      ${threads.length ? '' : `<div class=empty><p>no chirps yet</p></div>`}` +
      threads.map(t => `<details class=chirp data-id="${esc(t.id)}"${t.resolved ? ' data-resolved' : ''}>
        <summary><span class=who>${who(t.author)}${t.to ? ` <span class=text-faint>→</span> ${who(t.to)}` : ''}</span><span class=what>${tags(t.title)}${proj(t)}</span><time data-t="${esc(t.updatedAt)}">${ago(t.updatedAt)}</time></summary>
        ${(t.messages || []).map(m => `<div class=msg><span class=who>${who(m.author)}</span><div class=body>${tags(m.text)}</div></div>`).join('')}
        ${feedOn ? `<div class=actions><input class=field placeholder="reply" enterkeyhint=send><button class="key key-on" data-reply>reply</button><button class=key data-resolve>${t.resolved ? 'unresolve' : 'resolve'}</button></div>` : ''}
      </details>`).join('');
  };
  const feed = () => api('/feed').then(r => r.json()).then(v => {
    if (!sc.live || !showing('feed')) return;
    feedOn = !!v.on;
    const open = new Set($$('#ffeed .chirp[open]').map(e => e.dataset.id));
    threads = v.threads || [];
    drawFeed();
    for (const e of $$('#ffeed .chirp')) if (open.has(e.dataset.id)) e.open = true;
  }, () => {});
  const ft = setInterval(feed, 15000);
  if (wide) feed();
  sc.on(() => clearInterval(ft));

  const fbox = $('#ffeed');
  const send = async (o, done) => { try { await post('/feed', o); done && done(); feed(); } catch (e) { failed(e); } };
  fbox.onclick = e => {
    const d = e.target.closest('[data-id]');
    if (e.target.closest('#fpost')) {
      const b = $('#ftxt'), t = b.value.trim();
      if (t) send({op: 'post', text: t}, () => b.value = '');
    } else if (e.target.closest('[data-reply]') && d) {
      const b = $('input', d), t = b.value.trim();
      if (t) send({op: 'reply', id: d.dataset.id, text: t});
    } else if (e.target.closest('[data-resolve]') && d) {
      send({op: 'resolve', id: d.dataset.id, resolve: !d.hasAttribute('data-resolved')});
    }
  };
  fbox.onkeydown = e => {
    if (e.key !== 'Enter' || e.shiftKey) return;
    const d = e.target.closest('[data-id]');
    if (d && e.target.matches('input')) { e.preventDefault(); $('[data-reply]', d).click(); }
  };

  $('#ftabs').onclick = e => {
    const b = e.target.closest('[data-t]');
    if (!b || b.dataset.t === tab) return;
    tab = b.dataset.t;
    $$('#ftabs .tab').forEach(x => x.classList.toggle('tab-on', x === b));
    $('#fsess').parentNode.classList.toggle('hidden', tab !== 'sessions');
    $('#ffeedcol').classList.toggle('hidden', tab !== 'feed');
    scrollTo(0, 0);
    if (tab === 'feed') feed(); else poll();
  };
}

// ── one session ──
const glyphs = {Bash: '$', Edit: '✎', MultiEdit: '✎', Write: '✎', Read: '◧', Grep: '⌕', Glob: '⌕', WebFetch: '⊕', WebSearch: '⊕', Task: '⇉', Agent: '⇉', TodoWrite: '☐'};
function sessionScreen(id, init) {
  const sc = newScope();
  let info = lastList.find(i => i.id === id) || {};
  const head = () => {
    const st = stateOf({...info, alive: info.alive ?? true});
    bar({title: `${esc(info.name || id)}`, sub: `${state(st, info.needs)}<span class="truncate text-dim">${esc(home(info.cwd))}${info.model ? ' · ' + esc(info.model) : ''}</span>`,
      back: true, actions: [action('info', 'details', () => details())]});
    const lg = $('#composer legend');
    if (lg) lg.innerHTML = `to <b>${esc(info.name || id)}</b>${st === 'working' ? ' · <em>working</em>, send queues, steer goes in now' : ''}`;
  };
  $('#main').innerHTML = `<button id=jump class="key key-plain jump hidden" aria-label="to the newest">↓</button>
    <nav class=tabs id=tabs aria-label="this session"><button class=tab data-t=c>conversation</button><button class=tab data-t=agents>agents</button><button class=tab data-t=shells>shells</button><button class=tab data-t=changes>changes</button><button class=tab data-t=overview>overview</button></nav>
    <div id=log class="pt-1"></div><div id=asks></div><div id=tabpane class="px-3 pt-2 hidden"></div>
    <div class=composer id=composer><div id=pics class=chips></div>
      <fieldset class=box><legend></legend><span class=prompt>❯</span><textarea id=txt rows=1 placeholder="message for this agent" enterkeyhint=send></textarea><button class="key key-on h-7" id=send>send</button></fieldset>
      <div class=hints><button id=pic><b>image</b></button><input id=file type=file accept="image/*" multiple class=hidden><span class=sep>·</span>
        <button id=steer title="into the turn under way"><b>steer</b></button><span class=sep>·</span><button id=halt class=text-red>stop</button><span class=sep>·</span><button id=chg><b>changes</b></button></div></div>`;
  head();
  onList = l => {
    const x = l.find(i => i.id === id);
    if (!x || !sc.live) return;
    const k = i => [i.name, i.state, i.alive, i.needs, i.model, i.cwd].join('\u0000');
    if (k(x) !== k(info)) { info = {...info, ...x}; head(); }
  };
  sc.on(() => { onList = null; });
  // the conversation stays clear of the box, whatever its height
  const ro = new ResizeObserver(() => { $('#main').style.paddingBottom = $('#composer').offsetHeight + 8 + 'px'; });
  ro.observe($('#composer'));
  sc.on(() => ro.disconnect());

  const log = $('#log'), asks = new Map(), tasks = new Map(), calls = new Map();
  let stick = true, live = null, plan = null;

  // The history paged back: every event loaded (evts, oldest first), which
  // of them the log renders from (lo; the rest were dropped off the top to
  // keep the DOM a window), and where the server reads on from (cursor,
  // more). Scrolling to the top fills the window in — from evts when it
  // still holds what's missing, else from the server, a page at a time.
  const CAP = 600, PAGE = 300;
  let evts = [], lo = 0, cursor = null, more = false, busy = false;
  const seenIds = new Set();
  const op = o => post(`/sessions/${encodeURIComponent(id)}/op`, o).then(() => true, e => { failed(e); return false; });

  // say sends the draft once. It stays in the box until the server has taken
  // it (serve handed it to the session, not that the agent has read it: the
  // conversation shows that), and a failed or unanswered request keeps it,
  // to be sent again by hand. Typing meanwhile is kept.
  let sending = false, pics = [];
  const box = $('#txt');
  const grow = () => { box.style.height = 'auto'; box.style.height = Math.min(box.scrollHeight, 160) + 'px'; };
  box.oninput = grow;
  box.onkeydown = e => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); say(false); } };
  const drawPics = () => {
    $('#pics').innerHTML = pics.map((p, i) => `<span class=chip><img src="data:${p.type};base64,${p.data}" alt=""><button data-i=${i} aria-label=Remove>×</button></span>`).join('');
  };
  $('#pics').onclick = e => { const i = e.target.dataset.i; if (i !== undefined && !sending) { pics.splice(+i, 1); drawPics(); } };
  const addFiles = async files => {
    for (const f of files) {
      if (!f.type.startsWith('image/')) continue;
      try { pics.push(await picture(f)); } catch (e) { toast(`${f.name || 'image'}: ${e}`, 'bad'); }
    }
    drawPics();
  };
  $('#pic').onclick = () => $('#file').click();
  $('#file').onchange = e => { addFiles([...e.target.files]); e.target.value = ''; };
  box.onpaste = e => { const fs = [...(e.clipboardData?.files || [])]; if (fs.length) { e.preventDefault(); addFiles(fs); } };
  const say = async guide => {
    const sent = box.value, t = sent.trim(), att = pics.slice();
    if ((!t && !att.length) || sending) return;
    sending = true; $$('#send,#steer').forEach(b => b.disabled = true);
    try {
      await post(`/sessions/${encodeURIComponent(id)}/op`, {op: 'send', text: t, guide, pictures: att.length ? att : undefined});
      pics = pics.filter(p => !att.includes(p)); drawPics();
      box.value = box.value === sent ? '' : box.value.startsWith(sent) ? box.value.slice(sent.length).trimStart() : box.value;
      grow(); stick = true; down();
    } catch (e) {
      if (e === 401) return signIn();
      toast(`Not confirmed: ${e}. Your message is still in the box. It may have been delivered; check the conversation before sending again.`, 'bad');
    } finally {
      sending = false; $$('#send,#steer').forEach(b => b.disabled = false);
    }
  };
  $('#send').onclick = () => say(false);
  $('#steer').onclick = () => say(true);
  $('#halt').onclick = () => op({op: 'interrupt'});

  // changes: git's diff of the working folder against HEAD, and what's
  // untracked, read on its machine — a tab, or the sheet the composer's
  // button opens from any of them
  const git = async args => {
    const r = await (await post('/git', {cwd: info.cwd, args})).json();
    return new TextDecoder().decode(Uint8Array.from(atob(r.out || ''), c => c.charCodeAt(0))) + (r.error ? '\n' + r.error : '');
  };
  const fillChanges = async box => {
    box.innerHTML = `<div class="skeleton h-40"></div>`;
    try {
      const [df, u] = await Promise.all([git(['diff', 'HEAD', '--no-color', '--no-ext-diff']), git(['ls-files', '--others', '--exclude-standard', '--full-name', '--', ':/'])]);
      const lines = df.split('\n').map(l => `<span class="${l.startsWith('+') ? 'add' : l.startsWith('-') ? 'del' : l.startsWith('@@') || l.startsWith('diff ') ? 'hunk' : ''}">${esc(l)}</span>`).join('\n');
      box.innerHTML = (u.trim() ? `<p class="text-dim pb-1">untracked: ${esc(u.trim().split('\n').join(', '))}</p>` : '') + `<pre>${df.trim() ? lines : 'no changes against HEAD'}</pre>`;
    } catch (e) { box.innerHTML = `<div class=err-line>${esc(e)}</div>`; }
  };
  $('#chg').onclick = () => {
    const d = sheet(`<fieldset class=box><legend>changes · <span class=text-blue>${esc(home(info.cwd))}</span></legend><div id=chgbody></div></fieldset>`, 'changes');
    fillChanges($('#chgbody', d));
  };

  const down = () => { if (stick) scrollTo(0, document.body.scrollHeight); };
  // Only a scroll up lets go of the bottom. Our own scrollTo moves down, and
  // its event can land after more of the log is drawn, when the page is no
  // longer at the bottom: read as a scroll, that left a load at the top.
  let lastY = scrollY;
  const onScroll = () => {
    const bottom = innerHeight + scrollY >= document.body.scrollHeight - 120;
    if (scrollY < lastY || bottom) stick = bottom;
    lastY = scrollY;
    const j = $('#jump');
    if (j) j.classList.toggle('hidden', stick || tab !== 'c');
    if (scrollY < 80) earlier();
  };
  addEventListener('scroll', onScroll, {passive: true});
  sc.on(() => removeEventListener('scroll', onScroll));
  // the log grows without an event too (an image loads, a card opens)
  const grown = new ResizeObserver(() => { if (tab === 'c') down(); });
  grown.observe(log);
  sc.on(() => grown.disconnect());
  const jb = $('#jump');
  if (jb) jb.onclick = () => { stick = true; down(); };

  // fill the window in above what's drawn: from evts when it still reaches
  // back far enough, else a page from the server. The scroll stays put.
  const earlier = () => {
    if (busy || !sc.live) return;
    while (scrollY < 80 && lo > 0) {
      const from = Math.max(0, lo - PAGE);
      // a plan redrawn from history would only repeat the one drawn: the
      // newest of the range, and only when none is
      const planAt = plan ? -1 : from + evts.slice(from, lo).findLastIndex(e => e.t === 'plan');
      const f = document.createDocumentFragment();
      for (let i = from; i < lo; i++) if (evts[i].t !== 'plan' || i === planAt) apply(evts[i], true, i, f);
      const h0 = document.body.scrollHeight;
      log.prepend(f);
      lo = from;
      scrollBy(0, document.body.scrollHeight - h0);
    }
    if (lo === 0 && more) page();
  };
  const topOff = () => { const t = $('#older'); if (t) t.remove(); };
  const page = async () => {
    if (busy || !more || !sc.live) return;
    busy = true;
    const t = el('<div class="px-3 py-2 text-faint" id=older>…</div>');
    log.prepend(t);
    let again = false;
    try {
      const q = (cursor ? `?cursor=${encodeURIComponent(cursor)}&` : '?') + `limit=${PAGE}`;
      const v = await (await api(`/sessions/${encodeURIComponent(id)}/events/before${q}`)).json();
      if (!sc.live) return;
      if (v.reset) return stream(); // the transcript was rewritten: the stream says so
      more = !!v.more;
      cursor = v.cursor || null;
      topOff();
      if (!v.events?.length) return;
      // one plan a page (the newest of them, and only when none is drawn —
      // the stream's is the newest there is), and nothing the stream drew
      // (a message by its id, a call its card already in the log)
      const planAt = v.events.map(e => e.t).lastIndexOf('plan');
      const known = e => e.e?.Parts?.some(p => p.Call && calls.has(p.Call.ID));
      const kept = v.events.filter((e, n) =>
        (e.t !== 'plan' || (n === planAt && !plan))
        && !(e.t === 'message' && e.e?.ID && seenIds.has(e.e.ID))
        && !(e.t === 'message' && known(e)));
      if (!kept.length) return;
      const k = kept.length;
      // what the page holds sits above everything evts had: their indexes
      // move up by k, the drawn nodes with them.
      const seam = log.firstElementChild;
      for (const n of log.children) if (n._i !== undefined) n._i += k;
      evts = kept.concat(evts);
      lo += k;
      const f = document.createDocumentFragment();
      kept.forEach((e, i) => { if (e.e?.ID) seenIds.add(e.e.ID); apply(e, true, i, f); });
      const h0 = document.body.scrollHeight;
      log.prepend(f);
      scrollBy(0, document.body.scrollHeight - h0);
      // the newest of the page may be one the stream drew (an id-less user
      // message, live before the page was asked for): drop the page's copy
      // when the pair renders the same
      const above = seam?.previousElementSibling;
      if (above && above._i !== undefined && seam?._i !== undefined && above.outerHTML === seam.outerHTML) above.remove();
      if (more && lo === 0 && (!k || log.scrollHeight < innerHeight * 1.5)) again = true;
    } catch (e) { topOff(); failed(e); }
    finally { busy = false; if (again) page(); }
  };

  // keeps the log a window: when it holds more than CAP items and you're at
  // the bottom, whole events drop off the top (lo names where it starts).
  const trim = () => {
    if (!stick) return;
    while (log.children.length > CAP && log.firstElementChild?._i !== undefined) {
      const i0 = log.firstElementChild._i;
      if (i0 >= evts.length - 40) break; // the live end stays
      while (log.firstElementChild && log.firstElementChild._i === i0) log.firstElementChild.remove();
      lo = i0 + 1;
    }
    if (plan && !plan.isConnected) plan = null; // trimmed off with its turn
  };

  // a call's kind as rush numbers it (core's KINDS), the glyph each draws,
  // and one drawn: folded, what it works on, and — open — what went in and
  // what came out (the output lands in it as it arrives).
  const K = {SHELL: 1, READ: 2, EDIT: 3, WRITE: 4, DELETE: 5, MOVE: 6, SEARCH: 7, GLOB: 8, FETCH: 9, WEB: 10, AGENT: 11, TODO: 12, ASK: 13, PLAN: 14, MCP: 15, NOTE: 16, THINK: 17};
  const kindGlyph = ['·', '$', '◧', '✎', '✎', '✕', '→', '⌕', '⌕', '⊕', '⊕', '⇉', '☐', '?', '◫', '◆', '▦', '·'];
  const kindName = k => Object.entries(K).find(([, n]) => n === k)?.[0].toLowerCase() || 'other';
  const diff = ed => `<pre class=indiff>${ed.Old ? ed.Old.split('\n').map(x => `<span class=del>- ${esc(x)}</span>`).join('\n') + '\n' : ''}${(ed.New || '').split('\n').map(x => `<span class=add>+ ${esc(x)}</span>`).join('\n')}</pre>`;
  // what an edit or write did, as its output's patches say: a file's hunks,
  // each line prefixed as the patch has it
  const patchDiff = p => `<div class="text-blue truncate">${esc(home(p.path || ''))}</div><pre>${p.lines ? p.lines.map(l => `<span class="${l.startsWith('+') ? 'add' : l.startsWith('-') ? 'del' : ''}">${esc(l)}</span>`).join('\n') : ''}</pre>`;
  const todoBox = (label, ts) => `<div class=plan><div>${esc(label)} <span class=text-dim>${ts.filter(t => t.Status === 'completed').length}/${ts.length}</span></div><ul>${ts.map(t => `<li data-s="${esc(t.Status)}"><span>${t.Status === 'completed' ? '✓' : t.Status === 'in_progress' ? '▸' : '·'}</span><span>${esc(t.Label)}</span></li>`).join('')}</ul></div>`;
  const callBody = c => {
    const i = c.Input || {};
    switch (c.Kind) {
      case K.SHELL: return `${i.Cwd ? `<div class=where>${esc(home(i.Cwd))}${i.Background ? ' · in the background' : ''}</div>` : ''}<pre class=cmd>${esc(i.Command || '')}</pre>`;
      case K.EDIT: return (i.Edits || []).map(e => `<div class="text-blue truncate">${esc(home(e.Path || i.Path))}</div>${diff(e)}`).join('') || `<div class="text-blue truncate">${esc(home(i.Path))}</div>`;
      case K.WRITE: return `<div class="text-blue truncate">${esc(home(i.Path))}</div>${i.Content ? `<pre>${esc(i.Content.slice(0, 4000))}</pre>${i.Content.length > 4000 ? '<p class=text-faint>…</p>' : ''}` : ''}`;
      case K.READ: case K.NOTE: return `<div class="text-blue truncate">${esc(home(i.Path))}${i.Offset ? ` <span class=text-dim>from line ${i.Offset}${i.Limit ? `, ${i.Limit} lines` : ''}</span>` : ''}</div>`;
      case K.FETCH: return `${i.URL ? `<a href="${esc(i.URL)}" target=_blank rel=noreferrer>${esc(i.URL)}</a>` : ''}${i.Prompt ? `<p class=text-dim>${esc(i.Prompt)}</p>` : ''}`;
      case K.WEB: return `<p>${esc(i.Query || '')}</p>`;
      case K.SEARCH: case K.GLOB: return `<pre>${esc(i.Pattern || '')}${i.Path ? `\n<span class=text-dim>in ${esc(home(i.Path))}</span>` : ''}</pre>`;
      case K.TODO: return todoBox((i.Todos || []).find(t => t.Status === 'in_progress')?.Active || 'task list', i.Todos || []);
      case K.AGENT: return `<div class=agent>${esc(i.Agent || c.Name || 'agent')}</div>${i.Prompt ? `<p class=text-dim>${esc(i.Prompt.slice(0, 600))}${i.Prompt.length > 600 ? '…' : ''}</p>` : ''}`;
      default: return c.Title ? `<p class=text-dim>${esc(c.Title)}</p>` : '';
    }
  };
  const callWhat = c => { const i = c.Input || {};
    switch (c.Kind) {
      case K.SHELL: return (i.Command || '').split('\n')[0] || i.Description || '';
      case K.READ: case K.EDIT: case K.WRITE: case K.NOTE: return home(i.Path);
      case K.MOVE: return `${home(i.Path)} → ${home(i.To)}`;
      case K.SEARCH: case K.GLOB: return i.Pattern || '';
      case K.FETCH: return i.URL || '';
      case K.WEB: return i.Query || '';
      case K.AGENT: return i.Description || (i.Prompt || '').split('\n')[0] || 'subagent';
      case K.TODO: { const t = (i.Todos || []).find(x => x.Status === 'in_progress'); return (t && (t.Active || t.Label)) || 'task list'; }
      default: return i.Description || home(i.Path) || i.Pattern || (i.Command || '').split('\n')[0] || '';
    }
  };
  const callMore = c => { const i = c.Input || {};
    if (c.Kind === K.EDIT || c.Kind === K.WRITE) {
      const a = (i.Content || '').split('\n').length + (i.Edits || []).reduce((n, e) => n + (e.New || '').split('\n').length, 0);
      const d = (i.Edits || []).reduce((n, e) => n + (e.Old || '').split('\n').length, 0);
      return `+${Math.max(0, a - 1)} −${Math.max(0, d - 1)}`;
    }
    if (c.Kind === K.AGENT) return i.Agent || '';
    // what a shell call is for, when the agent said and it isn't the command again
    const cmd = (i.Command || '').split('\n')[0];
    return c.Kind === K.SHELL && i.Description && i.Description !== cmd ? i.Description : '';
  };
  const callEl = c => el(`<details class=step data-kind=${kindName(c.Kind)}><summary><span class=run>✻</span><span class=glyph>${kindGlyph[c.Kind] || glyphs[c.Name] || '·'}</span><span class=what>${esc(callWhat(c) || c.Title || c.Name)}</span><span class=more>${esc(callMore(c))}</span></summary><div class=step-body>${callBody(c)}</div></details>`);
  const finish = (c, bad) => { const m = $('summary > span', c); m.className = 'ok'; m.textContent = bad ? '✗' : '✓'; if (bad) c.dataset.err = ''; };
  // a call's output lands in its card: what its edit did as patches, its
  // text or stdout (kept to a scroll), and its stderr when it failed
  const callOut = (c, o) => {
    const b = $('.step-body', c);
    if (!b) return;
    if (o.Patches && o.Patches.length) { const d = $('.indiff', c); if (d) d.remove(); b.insertAdjacentHTML('beforeend', o.Patches.map(patchDiff).join('')); }
    const txt = (o.Text || o.Stdout || '').slice(0, 3000);
    if (txt) b.insertAdjacentHTML('beforeend', `<pre class=out>${esc(txt)}${(o.Text || o.Stdout || '').length > 3000 ? '\n…' : ''}</pre>`);
    if (o.IsError && o.Stderr && o.Stderr !== o.Text) b.insertAdjacentHTML('beforeend', `<pre class="out err-out">${esc(o.Stderr.slice(0, 1500))}</pre>`);
    if (o.Exit != null && o.Exit !== 0) b.insertAdjacentHTML('beforeend', `<p class=exit>exit ${+o.Exit}</p>`);
    finish(c, o.IsError || (o.Exit != null && o.Exit !== 0));
  };

  const askEl = (t, v) => {
    const a = document.createElement('div'); a.className = 'ask';
    if (t === 'approval') {
      a.innerHTML = `<h3>allow ${esc(v.Call.Name)}?</h3>${v.Reason ? `<p class=text-sub>${esc(v.Reason)}</p>` : ''}${callBody(v.Call) || `<p class=text-sub>${esc(callWhat(v.Call))}</p>`}
        <div class=actions><button class="key key-on" data-k=a>allow</button><button class=key data-k=al>always</button><button class="key key-bad" data-k=d>deny</button></div>`;
      a.onclick = ev => { const k = ev.target.closest('[data-k]')?.dataset.k; if (k) op(k === 'd' ? {op: 'deny', id: v.ID} : {op: 'allow', id: v.ID, always: k === 'al'}); };
    } else {
      const ans = {};
      a.innerHTML = `<h3>${esc(v.Title || 'question')}</h3>` + (v.Asks || []).map((q, n) => `<div class=space-y-1.5><p class=text-text>${esc(q.Text)}</p><div class=choices>${(q.Options || []).map(o =>
        `<button class="key choice" data-n="${n}" data-l="${esc(o.Label)}" title="${esc(o.Description)}">${esc(o.Label)}</button>`).join('')}</div></div>`).join('') +
        '<div class=actions><button class="key key-on" data-k=ok>answer</button></div>';
      a.onclick = ev => {
        const b = ev.target.closest('button');
        if (!b) return;
        if (b.dataset.l) {
          const q = v.Asks[b.dataset.n];
          ans[q.Text] = q.Multi && ans[q.Text] ? ans[q.Text] + ', ' + b.dataset.l : b.dataset.l;
          if (!q.Multi) $$('button', b.parentNode).forEach(x => x.classList.remove('on'));
          b.classList.add('on');
        } else if (b.dataset.k === 'ok') {
          // the input AskUserQuestion takes: the questions, and answers by question text
          const input = {title: v.Title || undefined, answers: ans, questions: (v.Asks || []).map(q => ({question: q.Text, header: q.Header, multiSelect: !!q.Multi,
            options: (q.Options || []).map(o => ({label: o.Label, description: o.Description, preview: o.Preview || undefined}))}))};
          op({op: 'allow', id: v.ID, input});
        }
      };
    }
    return a;
  };
  const drop = i => { const a = asks.get(i); a && a.remove(); asks.delete(i); };

  // decisions: the block its last turn ended with (info.decisions), the agent's
  // picks chosen to start with. The answers go as your next message, in the
  // block's numbering ("1a. Topic: label", "2: your words"); dismiss sends nothing.
  let decided = null, decEl = null;
  const decisionsEl = v => {
    const a = document.createElement('div'); a.className = 'ask';
    const ans = {};
    (v.Asks || []).forEach((q, n) => { const p = (q.Options || []).find(o => o.Picked); if (p) ans[n] = p.Label; });
    a.innerHTML = `<h3>decide</h3>` + (v.Asks || []).map((q, n) => `<div class=space-y-2><p class=text-text>${esc(q.Text)}</p><div class=choices>${(q.Options || []).map(o =>
      `<button class="key choice${o.Picked ? ' on' : ''}" data-n="${n}" data-l="${esc(o.Label)}" title="${esc(o.Description)}">${esc(o.Label)}${o.Picked ? ' ★' : ''}</button>`).join('')}</div>
      <input class=field data-own="${n}" placeholder="or in your own words"></div>`).join('') +
      '<div class=actions><button class="key key-on" data-k=ok>send answers</button><button class=key data-k=no>dismiss</button></div>';
    a.onclick = ev => {
      const b = ev.target.closest('button');
      if (!b) return;
      if (b.dataset.l) {
        ans[b.dataset.n] = b.dataset.l;
        $$('button', b.parentNode).forEach(x => x.classList.remove('on'));
        b.classList.add('on');
      } else if (b.dataset.k === 'no') {
        decided = v.ID; a.remove(); op({op: 'decisions_dismiss', id: v.ID});
      } else if (b.dataset.k === 'ok') {
        const text = (v.Asks || []).map((q, n) => {
          const own = $(`[data-own="${n}"]`, a).value.trim(), pick = ans[n];
          if (own) return `${q.ID}: ${own}`;
          const o = (q.Options || []).find(o => o.Label === pick);
          return o ? `${q.ID}${o.Key}. ${q.Header}: ${o.Label}` : '';
        }).filter(Boolean).join('\n\n');
        decided = v.ID; a.remove();
        op(text ? {op: 'send', text} : {op: 'decisions_dismiss', id: v.ID});
      }
    };
    return a;
  };
  const showDecisions = d => {
    if (decEl && (!d || decEl.dataset.id !== d.ID)) { decEl.remove(); decEl = null; }
    if (!d || d.ID === decided || decEl) return;
    decEl = decisionsEl(d); decEl.dataset.id = d.ID; $('#asks').append(decEl); down();
  };

  // details: how it is, and what can be done with it from here
  let open = null;
  const details = () => { open = sheet('<div id=dt class=space-y-3></div>', 'details'); open.addEventListener('close', () => open = null); drawDetails(); };
  const drawDetails = () => {
    if (!open) return;
    const t = [...tasks.values()].filter(x => !x.done);
    const away = info.away && !info.away.ended, ended = info.away && info.away.ended, lim = info.limit;
    const hm = d => new Date(d).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'});
    const held = info.away && (info.away.held || []).length;
    $('#dt', open).innerHTML = `<fieldset class=box><legend><b>${esc(info.name || id)}</b></legend><dl class=facts>
        <dt>state</dt><dd>${state(stateOf({...info, alive: info.alive ?? true}), info.needs)}</dd>
        <dt>agent</dt><dd>${esc([agentName(info.kind), info.model, info.effort].filter(Boolean).join(' · '))}</dd>
        ${info.costUsd ? `<dt>spent</dt><dd class=tabular-nums>${money(info.costUsd)}</dd>` : ''}
        <dt>folder</dt><dd class=text-blue>${esc(home(info.cwd))}</dd>
        <dt>machine</dt><dd>${esc(machine)}</dd></dl></fieldset>
      ${lim ? `<fieldset class=box><legend>usage limit</legend><p class=text-sub>until ${hm(lim.resetsAt)}; ${lim.continue ? 'carries on then' : 'waits for you'}</p><div class="pt-2"><button class=key data-l=${lim.continue ? 0 : 1}>${lim.continue ? "don't carry on" : 'carry on then'}</button></div></fieldset>` : ''}
      <fieldset class=box><legend>away</legend>${away ? `<p class=text-sub>you're away${info.away.until && !info.away.until.startsWith('0001') ? ' until ' + hm(info.away.until) : ''}${held ? ` · ${held} held for you` : ''}</p><div class=pt-2><button class="key key-on" data-a=back>i'm back</button></div>`
        : ended ? `<p class=text-sub>your away is over${held ? ` · ${held} held for you` : ''}</p><div class=pt-2><button class=key data-a=back>ok</button></div>`
        : `<p class=text-dim>the agent carries on, checked on every 30 minutes</p><div class="flex flex-wrap gap-1.5 pt-2"><button class=key data-a=away>i'm away…</button><span id=awayfor class="hidden flex gap-1.5">${[1, 2, 4, 8].map(h => `<button class=key data-h=${h}>${h}h</button>`).join('')}</span></div>`}</fieldset>
      ${t.length ? `<fieldset class=box><legend>in the background</legend>${t.map(x => `<div class="flex items-center gap-2 py-0.5"><span class="flex-1 truncate">${esc(x.Label || x.ID)} <span class=text-dim>${esc(x.status || '')}</span></span><button class="key key-plain key-bad" data-t="${esc(x.ID)}">stop</button></div>`).join('')}</fieldset>` : ''}
      ${(info.queue || []).length ? `<fieldset class=box><legend class=text-queue>queue ${info.queue.length} · sends when this turn ends</legend>${info.queue.map((m, i) => `<div class="flex items-center gap-2 py-0.5"><span class=text-dim>${i + 1}</span><span class="flex-1 truncate">${esc(m)}</span><button class="key key-plain" data-q=${i} data-w="${esc(m)}">now</button><button class="key key-plain key-bad" data-r=${i} data-w="${esc(m)}">remove</button></div>`).join('')}</fieldset>` : ''}`;
    $('#dt', open).onclick = e => {
      const b = e.target.closest('button');
      if (!b) return;
      if (b.dataset.t) op({op: 'stop_task', id: b.dataset.t});
      if (b.dataset.q) op({op: 'queue_send', index: +b.dataset.q, was: b.dataset.w});
      if (b.dataset.r) op({op: 'queue_remove', index: +b.dataset.r, was: b.dataset.w});
      if (b.dataset.l) op({op: 'limit', now: b.dataset.l === '1'});
      if (b.dataset.a === 'back') op({op: 'away'});
      if (b.dataset.a === 'away') $('#awayfor', open).classList.toggle('hidden');
      if (b.dataset.h) {
        const now = new Date();
        op({op: 'away', away: {from: now.toISOString(), until: new Date(+now + b.dataset.h * 3600e3).toISOString(), next: '0001-01-01T00:00:00Z'}})
          .then(ok => ok && toast(`away for ${b.dataset.h}h`));
      }
    };
  };

  // one event drawn. prep (a page, or a range filled back in from evts)
  // appends its nodes to box, a fragment the caller puts above the log;
  // the live stream appends to the log itself. Old asks, tasks and info
  // are live state, not history: prep leaves them alone.
  const apply = (e, prep, i, box) => {
    const put = x => {
      const n = typeof x === 'string' ? el(x) : x;
      n._i = i;
      (box || log).append(n);
      if (!prep) down();
      return n;
    };
    const v = e.e || {};
    switch (e.t) {
      case 'reset': break; // the stream handles it where it reads it
      case 'cut': more = true; break;
      case 'sent': if (prep) break; live = null; put(`<div class=you><span class=who>you</span><div>${esc(v.Text)}${v.Images && v.Images.length ? ` <span class="font-normal text-dim">+ ${v.Images.length} image${v.Images.length > 1 ? 's' : ''}</span>` : ''}</div></div>`); break;
      case 'delta': if (v.Kind === 0 && !prep) {
        if (!live) { live = put('<div class=say></div>'); live._s = ''; }
        live._s += v.Text; live.textContent = live._s; down();
      } break;
      case 'message': {
        if (v.Parent) break;
        if (v.ID) seenIds.add(v.ID);
        if (v.Role === 'assistant') {
          for (const p of v.Parts || []) {
            if (p.Kind === 0 && p.Text.trim()) { if (!prep && live) { live.innerHTML = md(p.Text); live = null; } else put(`<div class=say>${md(p.Text)}</div>`); }
            else if (p.Kind === 2 && p.Call) { const c = put(callEl(p.Call)); c._call = p.Call; calls.set(p.Call.ID, c); }
          }
        } else for (const p of v.Parts || []) {
          if (p.Kind === 0 && p.Text && !v.Injected && !(live && !prep)) put(`<div class=you><span class=who>you</span><div>${esc(p.Text)}</div></div>`);
          if (p.Kind === 3 && p.Output) {
            const c = calls.get(p.Output.CallID);
            if (c) callOut(c, p.Output);
          }
        }
      } break;
      case 'call_updated': { const c = calls.get(v.Call.ID); if (c) { const n = callEl(v.Call); n._i = c._i; n._call = v.Call; n.open = c.open; c.replaceWith(n); calls.set(v.Call.ID, n); } } break;
      case 'approval': case 'question': if (prep) break; { drop(v.ID); const a = askEl(e.t, v); asks.set(v.ID, a); $('#asks').append(a); down(); navigator.vibrate && navigator.vibrate(60); } break;
      case 'approval_cancelled': case 'answered': drop(v.ID); break;
      case 'task_started': if (prep) break; tasks.set(v.ID, v); drawDetails(); break;
      case 'task_updated': case 'task_done': if (prep) break; { const t = tasks.get(v.ID) || {ID: v.ID}; if (v.Status) t.done = true; tasks.set(v.ID, t); drawDetails(); } break;
      case 'task_progress': if (prep) break; { const t = tasks.get(v.ID); if (t) { t.status = v.Summary || v.LastTool; drawDetails(); } } break;
      case 'background': if (prep) break; { const ids = new Set((v.Tasks || []).map(t => t.ID)); tasks.forEach((t, k) => { if (!ids.has(k)) t.done = true; }); (v.Tasks || []).forEach(t => { if (!tasks.has(t.ID)) tasks.set(t.ID, t); }); drawDetails(); } break;
      case 'plan': {
        const todos = v.Todos || [], now = todos.find(t => t.Status === 'in_progress');
        const h = todoBox(now ? now.Label : 'plan', todos);
        if (plan && !prep && plan.isConnected) { const n = el(h); n._i = plan._i; plan.replaceWith(n); plan = n; } else plan = put(h);
      } break;
      case 'turn_end': if (!prep) live = null; if (v.Err) put(`<div class=err-line>✗ ${esc(v.Err)}</div>`); break;
      case 'error': put(`<div class=err-line>✗ ${esc(v.Error)}</div>`); break;
      case 'info': if (prep) break;
        info = v; head(); drawDetails();
        if (v.state && v.state !== 'blocked') { asks.forEach(a => a.remove()); asks.clear(); } // a decision no longer waited on is stale
        showDecisions(v.decisions);
        break;
    }
  };

  // ── the session's tabs ──
  // The conversation is one of five tabs on the session: its stream keeps
  // running under whichever is shown, so switching shows and hides rather
  // than rebuilding, and each tab draws into the one pane. The hash names
  // the tab, so it's a place you can come back to (and 'all', the old
  // link, still reads as the conversation).
  let tab = '', back = 0, tabTimer = null, lastSubs = [];
  const pane = $('#tabpane');
  const stopTab = () => { if (tabTimer) { clearInterval(tabTimer); tabTimer = null; } };
  const tok = n => n >= 1e6 ? (n / 1e6).toFixed(1) + 'M' : n >= 1e4 ? Math.round(n / 1e3) + 'k' : n >= 1e3 ? (n / 1e3).toFixed(1) + 'k' : String(n || 0);
  const dur = ms => { const m = Math.round((ms || 0) / 6e4); return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m}m`; };

  // subagents: what each is, what it last said, and its own conversation
  // one tap in. Whether one is running is read off the parent's card for
  // the call that started it, when the window still holds it.
  const subCard = s => {
    const c = s.toolUseId && calls.get(s.toolUseId);
    const st = c && $('summary .run', c) ? '<span class=st-working>✻ running</span>'
      : c && $('summary .ok', c) ? (c.dataset.err !== undefined ? '<span class=text-red>✗ failed</span>' : '<span class=st-idle>✓ done</span>') : '';
    const when = s.modified || s.born;
    return `<div class=row-top><b>⇉ ${esc(s.description || s.type || s.id)}</b>${when ? `<time data-t="${new Date(when).toISOString()}">${ago(new Date(when).toISOString())}</time>` : ''}</div>
      <div class=row-sub>${s.activity ? `<span>${esc(s.activity)}</span>` : '<span class=text-faint>nothing said yet</span>'}</div>
      <div class=row-sub>${st ? `<span>${st}</span>` : ''}${s.model ? `<span>${esc(s.model)}</span>` : ''}${s.depth > 1 ? `<span>depth ${s.depth}</span>` : ''}</div>`;
  };
  const drawAgents = v => {
    lastSubs = v.subagents || [];
    if (!lastSubs.length) { pane.innerHTML = `<div class=empty><p>no subagents</p></div>`; return; }
    keyed(pane, lastSubs, s => s.id, () => el('<button class=row></button>'), (e, s) => { e.dataset.sub = s.id; e.innerHTML = subCard(s); });
  };
  const openSub = s => {
    const d = sheet(`<fieldset class=box><legend>⇉ ${esc(s.description || s.id)}${s.model ? ` · <span class=text-dim>${esc(s.model)}</span>` : ''}</legend><div id=sublog><div class="skeleton h-40"></div></div></fieldset>`, 'subagent');
    api(`/sessions/${encodeURIComponent(id)}/subagents/${encodeURIComponent(s.id)}/events?limit=400`)
      .then(r => r.json(), () => null)
      .then(v => {
        const box = $('#sublog', d);
        if (!box || !v) return;
        box.innerHTML = '';
        const cs = new Map();
        for (const e of v.events || []) {
          const w = e.e || {};
          if (e.t === 'turn_end') { if (w.Err) box.insertAdjacentHTML('beforeend', `<div class=err-line>✗ ${esc(w.Err)}</div>`); continue; }
          if (e.t === 'plan') { const ts = w.Todos || [], now = ts.find(t => t.Status === 'in_progress'); box.insertAdjacentHTML('beforeend', todoBox(now ? now.Label : 'plan', ts)); continue; }
          if (e.t !== 'message' || w.Parent) continue;
          if (w.Role === 'assistant') {
            for (const p of w.Parts || []) {
              if (p.Kind === 0 && p.Text.trim()) box.insertAdjacentHTML('beforeend', `<div class=say>${md(p.Text)}</div>`);
              else if (p.Kind === 2 && p.Call) { const c = callEl(p.Call); box.append(c); cs.set(p.Call.ID, c); }
            }
          } else for (const p of w.Parts || []) {
            if (p.Kind === 0 && p.Text && !w.Injected) box.insertAdjacentHTML('beforeend', `<div class=you><span class=who>you</span><div>${esc(p.Text)}</div></div>`);
            if (p.Kind === 3 && p.Output) { const c = cs.get(p.Output.CallID); if (c) callOut(c, p.Output); }
          }
        }
      });
  };

  // shells: the session's background — the tasks its events tell of, and
  // the shell calls it asked to run there. A finished one stays, its
  // output one tap in.
  const drawShells = () => {
    const rows = [];
    for (const t of tasks.values()) if (!t.done) rows.push({top: t.Label || t.ID, sub: t.status || 'running', stop: t.ID});
    for (const c of calls.values()) {
      const k = c._call;
      if (!k || k.Kind !== K.SHELL || !k.Input?.Background) continue;
      const running = !!$('summary .run', c), failed = c.dataset.err !== undefined;
      rows.push({top: (k.Input.Command || '').split('\n')[0] || k.Title || 'shell', mark: running ? '<span class=st-working>✻</span>' : failed ? '<span class=text-red>✗</span>' : '<span class=st-idle>✓</span>', card: c});
    }
    rows.reverse();
    if (!rows.length) { pane.innerHTML = `<div class=empty><p>nothing in the background</p></div>`; pane.onclick = null; return; }
    pane.innerHTML = rows.map((r, i) => `<div class=row data-i=${i} tabindex=0>
      <div class=row-top><b>${esc(r.top)}</b>${r.stop ? `<button class="key key-plain key-bad" data-stop="${esc(r.stop)}">stop</button>` : ''}${r.mark ? `<span class=shrink-0>${r.mark}</span>` : ''}</div>
      ${r.sub ? `<div class=row-sub><span>${esc(r.sub)}</span></div>` : ''}</div>`).join('');
    pane.onclick = e => {
      const b = e.target.closest('[data-stop]');
      if (b) { op({op: 'stop_task', id: b.dataset.stop}); return; }
      const r = e.target.closest('[data-i]');
      if (r) {
        const x = rows[+r.dataset.i];
        if (!x?.card) return;
        const d = sheet('<div id=shout></div>', 'shellout');
        const n = x.card.cloneNode(true);
        n.open = true;
        $('#shout', d).append(n);
      }
    };
  };

  // overview: the session's numbers at a glance, as its transcript reads
  const drawOverview = v => {
    if (!v || (!v.turns && !v.window)) { pane.innerHTML = `<div class=empty><p>no overview for this one</p></div>`; return; }
    const fill = () => { const f = $('.ctxfill', pane); if (f) { f.style.width = pct + '%'; f.classList.toggle('ctx-hot', pct >= 80); } };
    const pct = v.window ? Math.min(100, Math.round(100 * (v.context || 0) / v.window)) : 0;
    const models = Object.entries(v.models || {}).filter(([, c]) => c > 0);
    pane.innerHTML = `<fieldset class=box><legend>overview</legend><dl class=facts>
        <dt>agent</dt><dd>${esc(agentName(info.kind))}${v.model ? ` · ${esc(v.model)}` : ''}${info.effort ? ` · ${esc(info.effort)}` : ''}</dd>
        <dt>folder</dt><dd class=text-blue>${esc(home(v.cwd || info.cwd))}</dd>
        <dt>turns</dt><dd>${v.turns || 0} · ${dur(v.workingMs)} working</dd>
        ${v.cost ? `<dt>spent</dt><dd class=tabular-nums>${money(v.cost)}</dd>` : ''}
        ${v.version ? `<dt>version</dt><dd>${esc(v.version)}</dd>` : ''}
        ${(v.mcp || []).length ? `<dt>mcp</dt><dd>${v.mcp.map(esc).join(', ')}</dd>` : ''}</dl>
        ${v.window ? `<div class=ctxbar><div class=ctxfill></div></div>` : ''}
        <p class="text-dim pt-2">${v.window ? `${tok(v.context)} of ${tok(v.window)} in context · ${pct}%` : `${tok(v.context)} in context`}</p></fieldset>
      <fieldset class=box><legend>tokens</legend><dl class=facts>
        <dt>input</dt><dd>${tok(v.input)}</dd><dt>output</dt><dd>${tok(v.output)}</dd>
        <dt>cache read</dt><dd>${tok(v.cacheRead)}</dd><dt>cache write</dt><dd>${tok(v.cacheWrite)}</dd></dl>
        ${models.length ? `<div class=pt-2>${models.map(([m, c]) => `<div class="flex justify-between gap-2"><span class=truncate>${esc(m)}</span><span class="text-dim tabular-nums">${money(c)}</span></div>`).join('')}</div>` : ''}</fieldset>
      ${(v.tools || []).length ? `<fieldset class=box><legend>tools · ${(v.tools || []).reduce((n, t) => n + (t.calls || 0), 0)} calls</legend><table class=ttable>${v.tools.map(t => `<tr><td>${esc(t.name)}</td><td>${t.calls || 0}${t.failed ? ` <span class=text-red>✗${t.failed}</span>` : ''}</td><td class=text-dim>${t.secs ? Math.round(t.secs) + 's' : ''}</td></tr>`).join('')}</table></fieldset>` : ''}`;
    fill();
  };

  const startTab = () => {
    stopTab();
    pane.innerHTML = '';
    pane.onclick = null;
    if (tab === 'agents') {
      const poll = () => api(`/sessions/${encodeURIComponent(id)}/subagents`).then(r => r.json(), () => null).then(v => { if (v && sc.live && tab === 'agents') drawAgents(v); });
      poll();
      tabTimer = setInterval(poll, 5000);
      pane.onclick = e => { const b = e.target.closest('[data-sub]'); if (!b) return; const s = lastSubs.find(x => x.id === b.dataset.sub); if (s) openSub(s); };
    } else if (tab === 'shells') {
      drawShells();
    } else if (tab === 'changes') {
      pane.innerHTML = `<fieldset class=box><legend>changes · <span class=text-blue>${esc(home(info.cwd))}</span></legend><div id=chgbody></div></fieldset>`;
      fillChanges($('#chgbody', pane));
    } else if (tab === 'overview') {
      pane.innerHTML = `<div class="skeleton h-40"></div>`;
      api(`/sessions/${encodeURIComponent(id)}/overview`).then(r => r.json(), () => null).then(v => { if (sc.live && tab === 'overview') drawOverview(v); });
    }
  };
  const setTab = t => {
    if (t === tab) return;
    const was = tab;
    tab = t;
    $$('.tab', $('#tabs')).forEach(b => b.classList.toggle('tab-on', b.dataset.t === t));
    const conv = t === 'c';
    if (was === 'c') back = scrollY; // where the conversation was, kept for the way back
    log.classList.toggle('hidden', !conv);
    $('#asks').classList.toggle('hidden', !conv);
    $('#composer').classList.toggle('hidden', !conv);
    const j = $('#jump');
    if (j) j.classList.toggle('hidden', !conv || stick);
    pane.classList.toggle('hidden', conv);
    if (conv) {
      stopTab(); pane.innerHTML = ''; pane.onclick = null;
      if (was) { scrollTo(0, back); stick = innerHeight + back >= document.body.scrollHeight - 120; }
    } else {
      scrollTo(0, 0);
      startTab();
    }
  };
  $('#tabs').onclick = e => {
    const b = e.target.closest('[data-t]');
    if (!b || b.dataset.t === tab) return;
    go(b.dataset.t === 'c' ? `#/s/${encodeURIComponent(machine)}/${id}` : `#/s/${encodeURIComponent(machine)}/${id}/${b.dataset.t}`);
  };
  setTab(init);
  // for route(): the same session's tab switches in place, its scope alive
  curTab = {id, set: setTab, get live() { return sc.live; }};
  sc.on(() => { stopTab(); if (curTab?.id === id) curTab = null; });

  // the stream: the conversation's live end, with the transcript's end
  // before it. What it sends is kept in evts, drawn into the log.
  let es = null;
  const clearAll = () => {
    log.innerHTML = ''; asks.clear(); $('#asks').innerHTML = ''; decEl = null; tasks.clear(); calls.clear();
    live = null; plan = null; evts = []; lo = 0; cursor = null; more = false; seenIds.clear();
  };
  const stream = () => {
    if (es) es.close();
    clearAll();
    es = new EventSource(base + `/api/sessions/${encodeURIComponent(id)}/events`);
    es.onmessage = m => {
      if (!sc.live) return;
      const e = JSON.parse(m.data);
      if (e.t === 'reset') return clearAll();
      evts.push(e);
      apply(e, false, evts.length - 1, null);
      trim();
      if (tab === 'shells') drawShells(); // its rows are the background's live state
    };
  };
  sc.on(() => es && es.close());
  stream();
}

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

async function subscribe() {
  try {
    const k = await (await api('/push')).json();
    if (await Notification.requestPermission() !== 'granted') return;
    const reg = await navigator.serviceWorker.ready;
    const sub = await reg.pushManager.subscribe({userVisibleOnly: true,
      applicationServerKey: Uint8Array.from(atob(k.key.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0))});
    await post('/push', sub.toJSON()); toast('notifications on');
  } catch (e) { toast('Notifications: ' + e, 'bad'); }
}
start();
