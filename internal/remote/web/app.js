'use strict';
const $ = s => document.querySelector(s);
const esc = s => String(s ?? '').replace(/[&<>"]/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;'}[c]));
let base = '', machine = '', machines = [], es = null, poll = null;

// api: GET reads are safe to repeat; writes are sent once and never retried
// (a send that was delivered but unanswered would be sent twice).
const api = (p, o) => fetch(base + '/api' + p, o).then(async r => {
  if (r.status === 401) throw 401;
  if (!r.ok) throw ((await r.json().catch(() => ({}))).error || r.statusText);
  return r;
});
const post = (p, b) => api(p, {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(b)});
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js');

async function start() {
  try {
    machines = await (await fetch('/api/machines').then(r => { if (r.status === 401) throw 401; return r })).json();
  } catch (e) { return e === 401 ? login() : ($('#main').textContent = e); }
  const s = $('#mach');
  s.innerHTML = machines.map(m => `<option value="${esc(m.name)}">${esc(m.name)}${m.self ? '' : ' (remote)'}</option>`).join('');
  s.classList.toggle('hide', machines.length < 2);
  s.onchange = () => { setMachine(s.value); home(); };
  $('#bell').classList.toggle('hide', !('PushManager' in window));
  route();
}
function setMachine(name) {
  const m = machines.find(x => x.name === name) || machines[0];
  machine = m.name; base = m.self ? '' : '/m/' + encodeURIComponent(m.name); $('#mach').value = machine;
}
function route() {
  const m = location.hash.match(/^#\/s\/([^/]+)\/(.+)$/);
  if (m) { setMachine(decodeURIComponent(m[1])); return open(m[2], m[2]); }
  setMachine(machine || machines[0].name); home();
}
addEventListener('hashchange', () => machines.length && route());

function login() {
  $('#main').innerHTML = '<p>Access token (<code>rush remote token</code> on that machine)</p><input id=tok type=password autocomplete=current-password><p><button class=p id=go>Sign in</button>';
  $('#go').onclick = async () => {
    const r = await fetch('/api/login', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({token: $('#tok').value})});
    r.ok ? start() : alert('wrong token');
  };
}
function stop() { es && es.close(); es = null; clearInterval(poll); }
async function home() {
  stop(); history.replaceState(null, '', location.pathname); $('#back').classList.add('hide'); $('#title').textContent = 'Rush';
  const draw = async () => {
    let l;
    try { l = await (await api('/sessions')).json(); } catch (e) { return e === 401 ? login() : ($('#main').textContent = 'offline: ' + e); }
    l.sort((a, b) => (b.state === 'blocked') - (a.state === 'blocked') || (b.alive - a.alive) || (b.updatedAt > a.updatedAt ? 1 : -1));
    if (others) return; // the list of others is up instead
    $('#main').innerHTML = (l.map(i => `<button class="row s-${i.alive ? esc(i.state) : 'off'}" data-id="${esc(i.id)}" data-name="${esc(i.name || i.id)}"><b>${esc(i.name || i.id)}</b><small>${i.alive ? esc(i.state) : 'stopped'}${i.needs ? ' · needs ' + esc(i.needs) : ''} · ${esc(i.cwd)}</small></button>`).join('') || '<p class=d>No sessions.</p>') +
      '<p><button id=oth>Sessions Rush didn\'t start…</button>';
    document.querySelectorAll('.row').forEach(b => b.onclick = () => { location.hash = '#/s/' + encodeURIComponent(machine) + '/' + b.dataset.id; });
    $('#oth').onclick = showOthers;
  };
  let others = false;
  // showOthers lists the machine's agents' sessions from outside Rush: a past
  // one carries on in Rush when picked; one running in a terminal is only shown.
  const showOthers = async () => {
    others = true; $('#main').innerHTML = '<p class=d>Reading…</p>';
    let l;
    try { l = await (await api('/others')).json(); } catch (e) { return ($('#main').textContent = e); }
    $('#main').innerHTML = '<p><button id=ours>‹ Rush sessions</button>' + (l.map(o => `<button class="row${o.live ? ' s-off' : ''}" data-r="${o.live ? '' : esc(o.sessionId)}"><b>${esc(o.name || o.sessionId)}</b><small>${esc(o.agent)}${o.account ? ' · ' + esc(o.account) : ''} · ${o.live ? 'running outside Rush' : 'past'} · ${esc(o.cwd)}</small></button>`).join('') || '<p class=d>None.</p>');
    $('#ours').onclick = () => { others = false; draw(); };
    document.querySelectorAll('.row').forEach(b => b.onclick = async () => {
      if (!b.dataset.r) return alert('It runs in a terminal there; it can be carried on in Rush once it ends.');
      if (!confirm('Carry this session on in Rush on ' + machine + '?')) return;
      try { const v = await (await post('/sessions', {resume: b.dataset.r})).json(); location.hash = '#/s/' + encodeURIComponent(machine) + '/' + v.id; }
      catch (e) { alert(e); }
    });
  };
  await draw(); poll = setInterval(draw, 4000);
}
$('#back').onclick = () => { location.hash = ''; home(); };
$('#new').onclick = () => {
  stop(); $('#back').classList.remove('hide');
  $('#main').innerHTML = '<p><input id=cwd placeholder="folder on this machine"><p><input id=ag placeholder="agent (default)"><p><textarea id=pr placeholder="first message" rows=4></textarea><p><button class=p id=mk>Start</button>';
  $('#mk').onclick = async () => {
    try { const v = await (await post('/sessions', {cwd: $('#cwd').value, agent: $('#ag').value, prompt: $('#pr').value})).json(); location.hash = '#/s/' + encodeURIComponent(machine) + '/' + v.id; }
    catch (e) { alert(e); }
  };
};

function open(id, name, full) {
  stop(); $('#back').classList.remove('hide'); $('#title').textContent = name;
  $('#main').innerHTML = '<div id=log></div><div id=asks></div><div id=side class=d></div><div id=bar><div id=pics></div><textarea id=txt placeholder="Message"></textarea><button class=p id=send>Send</button><button id=pic title="attach images">📎</button><input id=file type=file accept="image/*" multiple class=hide><button id=steer title="into the turn under way">Steer</button><button class=x id=halt>Stop</button><button id=chg title="what changed in its folder">Changes</button></div>';
  const log = $('#log'), asks = new Map(), tasks = new Map(), calls = new Map();
  let info = {}, stick = !full, live = null; // the whole history opens at its start
  const op = o => post(`/sessions/${encodeURIComponent(id)}/op`, o).then(() => true, e => { alert(e); return false; });
  // say sends the draft once. It stays in the box until the server has taken
  // it (a 202 means serve handed it to the session, not that the agent has
  // read it: the conversation shows that), and a failed or unanswered request
  // keeps it, to be sent again by hand. Typing meanwhile is kept.
  let sending = false, pics = [];
  const drawPics = () => {
    $('#pics').innerHTML = pics.map((p, i) => `<span class=chip><img src="data:${p.type};base64,${p.data}" alt=""><button class=x data-i=${i} title=remove>×</button></span>`).join('');
  };
  $('#pics').onclick = e => { const i = e.target.dataset.i; if (i !== undefined && !sending) { pics.splice(+i, 1); drawPics(); } };
  const addFiles = async files => {
    for (const f of files) {
      if (!f.type.startsWith('image/')) continue;
      try { pics.push(await picture(f)); } catch (e) { alert(`${f.name || 'image'}: ${e}`); }
    }
    drawPics();
  };
  $('#pic').onclick = () => $('#file').click();
  $('#file').onchange = e => { addFiles([...e.target.files]); e.target.value = ''; };
  $('#txt').onpaste = e => { const fs = [...(e.clipboardData?.files || [])]; if (fs.length) { e.preventDefault(); addFiles(fs); } };
  const say = async guide => {
    const box = $('#txt'), sent = box.value, t = sent.trim(), att = pics.slice();
    if ((!t && !att.length) || sending) return;
    sending = true; document.querySelectorAll('#send,#steer').forEach(b => b.disabled = true);
    try {
      await post(`/sessions/${encodeURIComponent(id)}/op`, {op: 'send', text: t, guide, pictures: att.length ? att : undefined});
      pics = pics.filter(p => !att.includes(p)); drawPics();
      box.value = box.value === sent ? '' : box.value.startsWith(sent) ? box.value.slice(sent.length).trimStart() : box.value;
    } catch (e) {
      alert(`Not confirmed: ${e}. Your message is still in the box. It may have been delivered; check the conversation before sending again.`);
    } finally {
      sending = false; document.querySelectorAll('#send,#steer').forEach(b => b.disabled = false);
    }
  };
  $('#send').onclick = () => say(false);
  $('#steer').onclick = () => say(true);
  $('#halt').onclick = () => op({op: 'interrupt'});
  // changes: git's diff of the working folder against HEAD, and what's untracked, read on its machine
  $('#chg').onclick = async () => {
    const old = $('#changes');
    if (old) return old.remove();
    const git = async args => {
      const r = await (await post('/git', {cwd: info.cwd, args})).json();
      return new TextDecoder().decode(Uint8Array.from(atob(r.out || ''), c => c.charCodeAt(0))) + (r.error ? '\n' + r.error : '');
    };
    try {
      const [d, u] = await Promise.all([git(['diff', 'HEAD', '--no-color', '--no-ext-diff']), git(['ls-files', '--others', '--exclude-standard', '--full-name', '--', ':/'])]);
      const lines = d.split('\n').map(l => `<span class="${l.startsWith('+') ? 'add' : l.startsWith('-') ? 'del' : l.startsWith('@@') || l.startsWith('diff ') ? 'd' : ''}">${esc(l)}</span>`).join('\n');
      const el = document.createElement('div'); el.id = 'changes';
      el.innerHTML = `<b>Changes</b> <span class=d>${esc(info.cwd)}</span>${u.trim() ? `<div class=d>untracked: ${esc(u.trim().split('\n').join(', '))}</div>` : ''}<pre>${d.trim() ? lines : 'no changes against HEAD'}</pre>`;
      $('#asks').before(el); el.scrollIntoView();
    } catch (e) { alert('Changes: ' + e); }
  };
  const down = () => { if (stick) scrollTo(0, document.body.scrollHeight); };
  const add = h => { log.insertAdjacentHTML('beforeend', h); down(); return log.lastElementChild; };
  addEventListener('scroll', () => stick = innerHeight + scrollY >= document.body.scrollHeight - 80, {passive: true});
  const diff = ed => `<pre>${ed.Old ? ed.Old.split('\n').map(x => `<span class=del>- ${esc(x)}</span>`).join('\n') + '\n' : ''}${(ed.New || '').split('\n').map(x => `<span class=add>+ ${esc(x)}</span>`).join('\n')}</pre>`;
  const callHtml = c => {
    const i = c.Input || {}; let body = '';
    if (i.Edits && i.Edits.length) body = i.Edits.map(e => `<div class=d>${esc(e.Path || i.Path)}</div>${diff(e)}`).join('');
    else if (i.Content) body = `<div class=d>${esc(i.Path)}</div><pre>${esc(i.Content.slice(0, 4000))}</pre>`;
    else if (i.Command) body = `<pre>${esc(i.Command)}</pre>`;
    return `<div class=tool>→ ${esc(c.Title || c.Name)} ${esc(i.Path || i.Pattern || i.Description || '')}</div>` + body;
  };
  const askEl = (t, v) => {
    const el = document.createElement('div'); el.className = 'ask';
    if (t === 'approval') {
      el.innerHTML = `<b>Allow ${esc(v.Call.Name)}?</b> <span class=d>${esc(v.Reason || '')}</span>${callHtml(v.Call)}<button class=p data-k=a>Allow</button><button data-k=al>Always</button><button class=x data-k=d>Deny</button>`;
      el.onclick = ev => { const k = ev.target.dataset.k; if (k) op(k === 'd' ? {op: 'deny', id: v.ID} : {op: 'allow', id: v.ID, always: k === 'al'}); };
    } else {
      const ans = {};
      el.innerHTML = `<b>${esc(v.Title || 'Question')}</b>` + (v.Asks || []).map((a, n) => `<div><p>${esc(a.Text)}</p>${(a.Options || []).map(o => `<button data-n="${n}" data-l="${esc(o.Label)}" title="${esc(o.Description)}">${esc(o.Label)}</button>`).join('')}</div>`).join('') + '<p><button class=p data-k=ok>Answer</button>';
      el.onclick = ev => {
        const b = ev.target;
        if (b.dataset.l) {
          const a = v.Asks[b.dataset.n];
          ans[a.Text] = a.Multi && ans[a.Text] ? ans[a.Text] + ', ' + b.dataset.l : b.dataset.l;
          b.parentNode.querySelectorAll('button').forEach(x => x.classList.remove('on')); b.classList.add('on');
        } else if (b.dataset.k === 'ok') {
          // the input AskUserQuestion takes: the questions, and answers by question text
          const input = {title: v.Title || undefined, answers: ans, questions: (v.Asks || []).map(a => ({question: a.Text, header: a.Header, multiSelect: !!a.Multi,
            options: (a.Options || []).map(o => ({label: o.Label, description: o.Description, preview: o.Preview || undefined}))}))};
          op({op: 'allow', id: v.ID, input});
        }
      };
    }
    return el;
  };
  const side = () => {
    const t = [...tasks.values()].filter(x => !x.done);
    const q = (info.queue || []).map((m, i) => `<div>⏳ ${esc(m)} <button data-q=${i} data-w="${esc(m)}">now</button> <button class=x data-r=${i} data-w="${esc(m)}">remove</button></div>`).join('');
    const away = info.away && !info.away.ended, ended = info.away && info.away.ended, lim = info.limit;
    const t0 = d => new Date(d).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'});
    $('#side').innerHTML = (info.state ? `<div>${esc(info.state)}${info.needs ? ' · needs ' + esc(info.needs) : ''} · $${(info.costUsd || 0).toFixed(2)}</div>` : '') +
      (lim ? `<div>⏸ limit until ${t0(lim.resetsAt)} · ${lim.continue ? 'carries on then' : 'waits for you'} <button data-l=${lim.continue ? 0 : 1}>${lim.continue ? "don't carry on" : 'carry on then'}</button></div>` : '') +
      (away ? `<div>🌙 away${info.away.until && !info.away.until.startsWith('0001') ? ' until ' + t0(info.away.until) : ''}${(info.away.held || []).length ? ` · ${info.away.held.length} held` : ''} <button data-a=back>I'm back</button></div>`
        : ended ? `<div>🌙 away is over${(info.away.held || []).length ? ` · ${info.away.held.length} held` : ''} <button data-a=back>OK</button></div>`
        : info.state ? `<div><button data-a=away>I'm away…</button></div>` : '') +
      t.map(x => `<div>⚙ ${esc(x.Label || x.ID)} ${esc(x.status || '')} <button class=x data-t="${esc(x.ID)}">stop</button></div>`).join('') + q;
    $('#side').onclick = e => {
      const b = e.target;
      if (b.dataset.t) op({op: 'stop_task', id: b.dataset.t});
      if (b.dataset.q) op({op: 'queue_send', index: +b.dataset.q, was: b.dataset.w});
      if (b.dataset.r) op({op: 'queue_remove', index: +b.dataset.r, was: b.dataset.w});
      if (b.dataset.l) op({op: 'limit', now: b.dataset.l === '1'});
      if (b.dataset.a === 'back') op({op: 'away'});
      if (b.dataset.a === 'away') {
        const h = prompt('Away for how many hours? (the agent carries on, checked on every 30 minutes)', '2');
        if (h && +h > 0) { const now = new Date(); op({op: 'away', away: {from: now.toISOString(), until: new Date(+now + h * 3600e3).toISOString(), next: '0001-01-01T00:00:00Z'}}); }
      }
    };
  };
  const drop = i => { const el = asks.get(i); el && el.remove(); asks.delete(i); };
  es = new EventSource(base + `/api/sessions/${encodeURIComponent(id)}/events` + (full ? '?full=1' : ''));
  es.onmessage = m => {
    const e = JSON.parse(m.data), v = e.e || {};
    switch (e.t) {
      case 'reset': log.innerHTML = ''; asks.clear(); $('#asks').innerHTML = ''; tasks.clear(); calls.clear(); live = null; break;
      case 'cut': if (full) add('<div class=d>— earlier history is too long to show —</div>');
        else add('<button id=earlier>Show earlier</button>').onclick = () => { open(id, $('#title').textContent, true); }; break;
      case 'sent': live = null; add(`<div class="m u">${esc(v.Text)}${v.Images && v.Images.length ? ` <span class=d>📎 ${v.Images.length}</span>` : ''}</div>`); break;
      case 'delta': if (v.Kind === 0) {
        if (!live) { live = add('<div class=m></div>'); live._s = ''; }
        live._s += v.Text; live.textContent = live._s; down();
      } break;
      case 'message':
        if (v.Parent) break;
        if (v.Role === 'assistant') {
          for (const p of v.Parts || []) {
            if (p.Kind === 0 && p.Text.trim()) { if (live) { live.textContent = p.Text; live = null; } else add(`<div class=m>${esc(p.Text)}</div>`); }
            else if (p.Kind === 2 && p.Call) calls.set(p.Call.ID, add(`<div>${callHtml(p.Call)}</div>`));
          }
        } else for (const p of v.Parts || []) {
          if (p.Kind === 0 && p.Text && !v.Injected && !live) add(`<div class="m u">${esc(p.Text)}</div>`);
          if (p.Kind === 3 && p.Output) {
            const o = p.Output, el = calls.get(o.CallID), txt = (o.Text || o.Stdout || '').slice(0, 3000);
            if (el && txt) el.insertAdjacentHTML('beforeend', `<details><summary class=d>${o.IsError ? 'error' : 'output'}</summary><pre>${esc(txt)}</pre></details>`);
          }
        }
        break;
      case 'call_updated': { const el = calls.get(v.Call.ID); if (el) el.innerHTML = callHtml(v.Call); } break;
      case 'approval': case 'question': { drop(v.ID); const el = askEl(e.t, v); asks.set(v.ID, el); $('#asks').append(el); navigator.vibrate && navigator.vibrate(60); } break;
      case 'approval_cancelled': case 'answered': drop(v.ID); break;
      case 'task_started': tasks.set(v.ID, v); side(); break;
      case 'task_updated': case 'task_done': { const t = tasks.get(v.ID) || {ID: v.ID}; if (v.Status) t.done = true; tasks.set(v.ID, t); side(); } break;
      case 'task_progress': { const t = tasks.get(v.ID); if (t) { t.status = v.Summary || v.LastTool; side(); } } break;
      case 'background': { const ids = new Set((v.Tasks || []).map(t => t.ID)); tasks.forEach((t, k) => { if (!ids.has(k)) t.done = true; }); (v.Tasks || []).forEach(t => { if (!tasks.has(t.ID)) tasks.set(t.ID, t); }); side(); } break;
      case 'plan': add(`<div class=tool>${(v.Todos || []).map(t => (t.Status === 'completed' ? '☑ ' : t.Status === 'in_progress' ? '▶ ' : '☐ ') + esc(t.Label)).join('<br>')}</div>`); break;
      case 'turn_end': live = null; if (v.Err) add(`<div class="m err">${esc(v.Err)}</div>`); break;
      case 'error': add(`<div class="m err">${esc(v.Error)}</div>`); break;
      case 'info':
        info = v; side(); if (v.name) $('#title').textContent = v.name;
        if (v.state && v.state !== 'blocked') { asks.forEach(el => el.remove()); asks.clear(); } // a decision no longer waited on is stale
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

$('#bell').onclick = async () => {
  try {
    const k = await (await api('/push')).json();
    if (await Notification.requestPermission() !== 'granted') return;
    const reg = await navigator.serviceWorker.ready;
    const sub = await reg.pushManager.subscribe({userVisibleOnly: true,
      applicationServerKey: Uint8Array.from(atob(k.key.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0))});
    await post('/push', sub.toJSON()); alert('Notifications on');
  } catch (e) { alert('Notifications: ' + e); }
};
start();
