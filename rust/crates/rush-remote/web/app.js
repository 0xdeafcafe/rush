'use strict';
// Rush's web app: your sessions on this machine (and its peers), from a
// phone or a browser, in rush's own look. Plain JS, no build but the CSS
// (webstyle/app.css).
//
// Routes are the hash, and the hash alone says what's on screen: #/ the
// sessions, #/s/<machine>/<id> one (…/all with its whole history), #/new,
// #/devices. Each screen is a scope: leaving it ends its streams and
// timers, and anything still in flight from it is dropped, so nothing
// late ever draws over the screen you're on. Lists update rows in place.

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

function route() {
  if (!machines.length) return;
  const h = location.hash;
  let m;
  $('#main').style.paddingBottom = '';
  if ((m = h.match(/^#\/s\/([^/]+)\/([^/]+)(\/all)?$/))) { setMachine(decodeURIComponent(m[1])); return sessionScreen(m[2], !!m[3]); }
  if (h === '#/new') return newScreen();
  if (h === '#/devices') return devicesScreen();
  homeScreen();
}
addEventListener('hashchange', route);

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
function homeScreen() {
  const sc = newScope();
  const acts = [];
  if (machines.length > 1) {
    const s = el(`<select id=mach class="key" aria-label=Machine>${machines.map(m => `<option value="${esc(m.name)}">${esc(m.name)}</option>`).join('')}</select>`);
    s.value = machine; s.onchange = () => { setMachine(s.value); homeScreen(); };
    acts.push(s);
  }
  if ('PushManager' in window) acts.push(action('bell', 'notify', subscribe));
  acts.push(action('devs', 'devices', () => go('#/devices')), action('new', '<span class=text-orange>+</span> new', () => go('#/new')));
  bar({title: machines.length > 1 ? esc(machine) : 'rush', actions: acts});
  $('#main').innerHTML = `<div id=list>${'<div class=row><div class="skeleton h-4 w-2/3 my-0.5"></div><div class="skeleton h-3 w-1/3 ml-[2ch] mt-1.5"></div></div>'.repeat(3)}</div><div id=oth></div>`;
  const draw = l => {
    if (!sc.live) return;
    lastList = l;
    const list = $('#list');
    if (!l.length) { list.innerHTML = `<div class=empty><div class="flex justify-center">${bottle('h-12 w-[29px] opacity-60')}</div><p>no sessions yet</p><p class=text-faint>start one with + new, or carry one on from below</p></div>`; set($('#sub'), '', ''); return; }
    if (!list.querySelector('[data-key]')) list.innerHTML = '';
    const ss = l.map(i => ({...i, st: stateOf(i)}));
    const rows = [];
    for (const [g, name] of groups) {
      const in_ = ss.filter(i => groupOf(i.st) === g).sort((a, b) => b.startedAt > a.startedAt ? 1 : -1);
      if (!in_.length) continue;
      const cost = in_.reduce((n, i) => n + (i.costUsd || 0), 0);
      rows.push({hdr: g, name, n: in_.length, cost}, ...in_);
    }
    keyed(list, rows, i => i.hdr ? 'h:' + i.hdr : i.id,
      i => i.hdr ? el('<div class=rule><b></b><span></span></div>')
        : el(`<a class=row><div class=row-top><span class=mark></span><b></b><span class=cost></span><time></time></div><div class=row-sub><span class=tree>└</span><span class=why></span></div></a>`),
      (e, i) => {
        if (i.hdr) { set(e, 'b', '▾ ' + i.name); set(e, 'span', `${i.n}${i.cost ? ' · ' + money(i.cost) : ''}`); return; }
        e.href = `#/s/${encodeURIComponent(machine)}/${encodeURIComponent(i.id)}`;
        e.dataset.id = i.id; e.dataset.state = i.st;
        set(e, '.mark', marks[i.st] || '·');
        set(e, 'b', i.name || i.id);
        set(e, '.cost', money(i.costUsd));
        const t = $('time', e); t.dataset.t = i.updatedAt; set(t, '', ago(i.updatedAt));
        const where = `<span class=text-blue>${esc(home(i.cwd))}</span> <span class=text-faint>· ${esc([agentName(i.kind), i.model].filter(Boolean).join(' · '))}</span>`;
        set(e, '.why', i.st === 'blocked' ? `<span class=text-yellow>${esc(stateWords(i.st, i.needs))}</span> <span class=text-faint>· ${esc(home(i.cwd))}</span>` : i.detail && i.st === 'working' ? `${esc(i.detail)}` : where, true);
      });
    const n = ss.filter(i => i.st === 'blocked').length, w = ss.filter(i => groupOf(i.st) === 'working').length;
    set($('#sub'), '', [n && `<span class=text-yellow>● ${n} need${n > 1 ? '' : 's'} you</span>`, w && `<span class=text-orange>✻ ${w} working</span>`].filter(Boolean).join(' '), true);
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

// ── one session ──
const glyphs = {Bash: '$', Edit: '✎', MultiEdit: '✎', Write: '✎', Read: '◧', Grep: '⌕', Glob: '⌕', WebFetch: '⊕', WebSearch: '⊕', Task: '⇉', Agent: '⇉', TodoWrite: '☐'};
function sessionScreen(id, full) {
  const sc = newScope();
  let info = lastList.find(i => i.id === id) || {};
  const head = () => {
    const st = stateOf({...info, alive: info.alive ?? true});
    bar({title: `${esc(info.name || id)}`, sub: `${state(st, info.needs)}<span class="truncate text-dim">${esc(home(info.cwd))}${info.model ? ' · ' + esc(info.model) : ''}</span>`,
      back: true, actions: [action('info', 'details', () => details())]});
    const lg = $('#composer legend');
    if (lg) lg.innerHTML = `to <b>${esc(info.name || id)}</b>${st === 'working' ? ' · <em>working</em>, send queues, steer goes in now' : ''}`;
  };
  $('#main').innerHTML = `<div id=log class="pt-1"></div><div id=asks></div>
    <div class=composer id=composer><div id=pics class=chips></div>
      <fieldset class=box><legend></legend><span class=prompt>❯</span><textarea id=txt rows=1 placeholder="message for this agent" enterkeyhint=send></textarea><button class="key key-on h-7" id=send>send</button></fieldset>
      <div class=hints><button id=pic><b>image</b></button><input id=file type=file accept="image/*" multiple class=hidden><span class=sep>·</span>
        <button id=steer title="into the turn under way"><b>steer</b></button><span class=sep>·</span><button id=halt class=text-red>stop</button><span class=sep>·</span><button id=chg><b>changes</b></button></div></div>`;
  head();
  // the conversation stays clear of the box, whatever its height
  const ro = new ResizeObserver(() => { $('#main').style.paddingBottom = $('#composer').offsetHeight + 8 + 'px'; });
  ro.observe($('#composer'));
  sc.on(() => ro.disconnect());

  const log = $('#log'), asks = new Map(), tasks = new Map(), calls = new Map();
  let stick = !full, live = null, plan = null;
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

  // changes: git's diff of the working folder against HEAD, and what's untracked, read on its machine
  $('#chg').onclick = async () => {
    const git = async args => {
      const r = await (await post('/git', {cwd: info.cwd, args})).json();
      return new TextDecoder().decode(Uint8Array.from(atob(r.out || ''), c => c.charCodeAt(0))) + (r.error ? '\n' + r.error : '');
    };
    const d = sheet(`<fieldset class=box><legend>changes · <span class=text-blue>${esc(home(info.cwd))}</span></legend><div id=chgbody><div class="skeleton h-40"></div></div></fieldset>`, 'changes');
    try {
      const [df, u] = await Promise.all([git(['diff', 'HEAD', '--no-color', '--no-ext-diff']), git(['ls-files', '--others', '--exclude-standard', '--full-name', '--', ':/'])]);
      const lines = df.split('\n').map(l => `<span class="${l.startsWith('+') ? 'add' : l.startsWith('-') ? 'del' : l.startsWith('@@') || l.startsWith('diff ') ? 'hunk' : ''}">${esc(l)}</span>`).join('\n');
      $('#chgbody', d).innerHTML = (u.trim() ? `<p class="text-dim pb-1">untracked: ${esc(u.trim().split('\n').join(', '))}</p>` : '') + `<pre>${df.trim() ? lines : 'no changes against HEAD'}</pre>`;
    } catch (e) { $('#chgbody', d).innerHTML = `<div class=err-line>${esc(e)}</div>`; }
  };

  const down = () => { if (stick) scrollTo(0, document.body.scrollHeight); };
  const onScroll = () => { stick = innerHeight + scrollY >= document.body.scrollHeight - 120; };
  addEventListener('scroll', onScroll, {passive: true});
  sc.on(() => removeEventListener('scroll', onScroll));
  const add = h => { const e = el(h); log.append(e); down(); return e; };

  const diff = ed => `<pre>${ed.Old ? ed.Old.split('\n').map(x => `<span class=del>- ${esc(x)}</span>`).join('\n') + '\n' : ''}${(ed.New || '').split('\n').map(x => `<span class=add>+ ${esc(x)}</span>`).join('\n')}</pre>`;
  const callBody = c => {
    const i = c.Input || {};
    if (i.Edits && i.Edits.length) return i.Edits.map(e => `<div class=text-blue>${esc(home(e.Path || i.Path))}</div>${diff(e)}`).join('');
    if (i.Content) return `<div class=text-blue>${esc(home(i.Path))}</div><pre>${esc(i.Content.slice(0, 4000))}</pre>`;
    if (i.Command) return `<pre>${esc(i.Command)}</pre>`;
    return '';
  };
  const callWhat = c => { const i = c.Input || {}; return i.Description || home(i.Path) || i.Pattern || (i.Command || '').split('\n')[0] || ''; };
  const callMore = c => { const i = c.Input || {}; return i.Description ? (i.Command || '').split('\n')[0] : ''; };
  const callEl = c => el(`<details class=step><summary><span class=run>✻</span><span class=glyph>${glyphs[c.Name] || '·'}</span><span class=what>${esc(callWhat(c) || c.Title || c.Name)}</span><span class=more>${esc(callMore(c))}</span></summary><div class=step-body>${callBody(c)}</div></details>`);
  const finish = (c, bad) => { const m = $('summary > span', c); m.className = 'ok'; m.textContent = bad ? '✗' : '✓'; if (bad) c.dataset.err = ''; };

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

  const es = new EventSource(base + `/api/sessions/${encodeURIComponent(id)}/events` + (full ? '?full=1' : ''));
  sc.on(() => es.close());
  es.onmessage = m => {
    if (!sc.live) return;
    const e = JSON.parse(m.data), v = e.e || {};
    switch (e.t) {
      case 'reset': log.innerHTML = ''; asks.clear(); $('#asks').innerHTML = ''; decEl = null; tasks.clear(); calls.clear(); live = null; plan = null; break;
      case 'cut':
        if (full) log.prepend(el('<p class="px-3 py-2 text-faint">— the earliest history is too long to show —</p>'));
        else { const b = el('<div class="px-3 py-2"><button class="key key-plain text-dim" id=earlier>↑ show earlier</button></div>'); log.prepend(b); $('#earlier').onclick = () => go(`#/s/${encodeURIComponent(machine)}/${id}/all`); }
        break;
      case 'sent': live = null; add(`<div class=you><span class=who>you</span><div>${esc(v.Text)}${v.Images && v.Images.length ? ` <span class="font-normal text-dim">+ ${v.Images.length} image${v.Images.length > 1 ? 's' : ''}</span>` : ''}</div></div>`); break;
      case 'delta': if (v.Kind === 0) {
        if (!live) { live = add('<div class=say></div>'); live._s = ''; }
        live._s += v.Text; live.textContent = live._s; down();
      } break;
      case 'message':
        if (v.Parent) break;
        if (v.Role === 'assistant') {
          for (const p of v.Parts || []) {
            if (p.Kind === 0 && p.Text.trim()) { if (live) { live.innerHTML = md(p.Text); live = null; } else add(`<div class=say>${md(p.Text)}</div>`); }
            else if (p.Kind === 2 && p.Call) { const c = callEl(p.Call); log.append(c); calls.set(p.Call.ID, c); down(); }
          }
        } else for (const p of v.Parts || []) {
          if (p.Kind === 0 && p.Text && !v.Injected && !live) add(`<div class=you><span class=who>you</span><div>${esc(p.Text)}</div></div>`);
          if (p.Kind === 3 && p.Output) {
            const o = p.Output, c = calls.get(o.CallID), txt = (o.Text || o.Stdout || '').slice(0, 3000);
            if (c) { if (txt) $('.step-body', c).insertAdjacentHTML('beforeend', `<pre>${esc(txt)}</pre>`); finish(c, o.IsError); }
          }
        }
        break;
      case 'call_updated': { const c = calls.get(v.Call.ID); if (c) { const n = callEl(v.Call); n.open = c.open; c.replaceWith(n); calls.set(v.Call.ID, n); } } break;
      case 'approval': case 'question': { drop(v.ID); const a = askEl(e.t, v); asks.set(v.ID, a); $('#asks').append(a); down(); navigator.vibrate && navigator.vibrate(60); } break;
      case 'approval_cancelled': case 'answered': drop(v.ID); break;
      case 'task_started': tasks.set(v.ID, v); drawDetails(); break;
      case 'task_updated': case 'task_done': { const t = tasks.get(v.ID) || {ID: v.ID}; if (v.Status) t.done = true; tasks.set(v.ID, t); drawDetails(); } break;
      case 'task_progress': { const t = tasks.get(v.ID); if (t) { t.status = v.Summary || v.LastTool; drawDetails(); } } break;
      case 'background': { const ids = new Set((v.Tasks || []).map(t => t.ID)); tasks.forEach((t, k) => { if (!ids.has(k)) t.done = true; }); (v.Tasks || []).forEach(t => { if (!tasks.has(t.ID)) tasks.set(t.ID, t); }); drawDetails(); } break;
      case 'plan': {
        const todos = v.Todos || [], now = todos.find(t => t.Status === 'in_progress'), done = todos.filter(t => t.Status === 'completed').length;
        const h = `<div class=plan><div>${esc(now ? now.Label : 'plan')} <span class=text-dim>${done}/${todos.length}</span></div><ul>${todos.map(t => `<li data-s="${esc(t.Status)}"><span>${t.Status === 'completed' ? '✓' : t.Status === 'in_progress' ? '▸' : '·'}</span><span>${esc(t.Label)}</span></li>`).join('')}</ul></div>`;
        if (plan) { const n = el(h); plan.replaceWith(n); plan = n; } else plan = add(h);
      } break;
      case 'turn_end': live = null; if (v.Err) add(`<div class=err-line>✗ ${esc(v.Err)}</div>`); break;
      case 'error': add(`<div class=err-line>✗ ${esc(v.Error)}</div>`); break;
      case 'info':
        info = v; head(); drawDetails();
        if (v.state && v.state !== 'blocked') { asks.forEach(a => a.remove()); asks.clear(); } // a decision no longer waited on is stale
        showDecisions(v.decisions);
        break;
    }
  };
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
