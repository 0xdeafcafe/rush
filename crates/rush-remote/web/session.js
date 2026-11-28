'use strict';
// One session (#/s/<machine>/<id>): its conversation as serve streams it,
// and under it the dock, by your thumb: what waits on you, what's under
// way, and the message box. Uses app.js's helpers; loaded before it runs.
//
// The stream is events {t, e}. A `reset` says what follows is the whole
// conversation again (at opening, and whenever serve sends it afresh):
// that replay is built off the page and swapped in once the stream goes
// quiet, and only if it differs from what's shown, so the page never runs
// through its history and a reader keeps their place.

const glyphs = {Bash: '$', Edit: '✎', MultiEdit: '✎', Write: '✎', Read: '◧', Grep: '⌕', Glob: '⌕', WebFetch: '⊕', WebSearch: '⊕', Task: '⇉', Agent: '⇉', TodoWrite: '☐'};
// what a run of steps did, in rush's words: "edited 2 files, ran 3 commands"
const did = {Bash: ['ran', 'command'], Edit: ['edited', 'file'], MultiEdit: ['edited', 'file'], Write: ['wrote', 'file'], Read: ['read', 'file'], Grep: ['searched', 'time'], Glob: ['searched', 'time'],
  WebFetch: ['fetched', 'page'], WebSearch: ['searched the web', 'time'], Task: ['ran', 'subagent'], Agent: ['ran', 'subagent']};
const OUT_MOST = 6000;
const shellRuns = new Map(); // what a session's shell has run, while the page lives
let earlier = null; // where "show earlier" was pressed, to come back to

function sessionScreen(id, full) {
  const sc = newScope();
  const key = `${machine}/${id}`, opPath = `/sessions/${enc(id)}/op`;
  let info = lastList.find(i => i.id === id) || {}, branch = '', stat = '', lostConn = false;
  $('#main').innerHTML = `<div class=convo id=convo>
    <div id=scroll class=scroller tabindex=-1><div id=log class=log role=log aria-label=Conversation><div class=log-skel><div class="skeleton"></div><div class="skeleton"></div><div class="skeleton"></div></div></div><div id=tail class="log tail" hidden><span class=mark-run aria-hidden=true>✻</span><span id=tailsay class=cut></span><time id=tailtime></time></div></div>
    <div class=dock>
      <button class="key jump" id=jump hidden>↓ latest</button>
      <div id=menu class=menu role=listbox aria-label=Commands hidden></div>
      <div class=dock-in>
        <div id=notices></div><div id=asks aria-live=polite></div><div id=strip></div>
        <div class=composer id=composer><div id=pics class=chips></div>
          <fieldset class=box><legend></legend><span class=prompt aria-hidden=true>❯</span><textarea id=txt rows=1 placeholder="Message this agent" aria-label=Message enterkeyhint=${fine.matches ? 'send' : 'enter'} autocapitalize=sentences></textarea>
            <button class="key key-bad" id=halt hidden aria-label="Stop the turn">■ stop</button><button class="key key-on" id=send>send</button></fieldset>
          <div class=hints><button id=pic>image</button><input id=file type=file accept="image/*" multiple hidden><span class=sep>·</span><button id=shb title="Run a command in this session's folder (or start a message with !)">shell</button><span class=sep>·</span><button id=steer hidden title="Into the turn under way, not after it">steer</button><span class=sep id=steersep hidden>·</span>
            <button id=modeb hidden></button><span class=keys>enter sends · shift enter for a new line · ! runs a command</span></div></div>
      </div></div></div>`;
  const scroll = $('#scroll'), log = $('#log'), box = $('#txt'), jump = $('#jump');
  const op = o => post(opPath, o).then(() => true, e => { failed(e); return false; });
  const asks = new Map();

  // S is what the conversation drawn so far has: a replay builds another
  const blank = () => ({calls: new Map(), tasks: new Map(), subs: new Map(), agents: [], todos: [], group: null, live: null, lastYou: null, said: false, steps: 0, at: 0, sentAt: 0, status: '', cmds: [], sents: []});
  let S = blank(), into = log, re = null;

  // ── the end kept in view, once a frame ──
  let stick = !full, fresh = false, downs = false;
  const atEnd = () => scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 120;
  const bottom = () => { scroll.scrollTop = scroll.scrollHeight; };
  const drawJump = () => { jump.hidden = stick; jump.toggleAttribute('data-new', fresh); jump.textContent = fresh ? ' new below' : '↓ latest'; };
  const down = () => {
    if (re) return;
    if (!stick) { if (!fresh) { fresh = true; drawJump(); } return; }
    if (downs) return;
    downs = true;
    requestAnimationFrame(() => { downs = false; if (sc.live && stick && !re) bottom(); });
  };
  scroll.onscroll = () => { if (re) return; stick = atEnd(); if (stick) fresh = false; drawJump(); };
  jump.onclick = () => { stick = true; fresh = false; bottom(); drawJump(); };
  // the dock growing (a card, a longer message) takes room from the conversation
  const ro = new ResizeObserver(() => { if (stick && !re) bottom(); });
  ro.observe(scroll);
  sc.on(() => ro.disconnect());

  const now = () => re ? S.at : Date.now();
  const add = e => { into.append(e); down(); return e; };
  const html = h => add(el(h));

  // ── the bar, the box's state, what's under way ──
  const stNow = () => stateOf({...info, alive: info.alive ?? true});
  const head = () => {
    const st = stNow(), busy = st === 'working' || st === 'starting', name = info.name || id;
    bar({title: esc(name), plain: name, sub: `${state(st, info.needs)}${branch ? `<span class=c-green>${esc(branch)}</span>` : ''}<span class=cut>${esc(home(info.cwd))}${info.model ? ' · ' + esc(info.model) : ''}</span>`});
    set($('#chg'), '', 'changes' + stat, true);
    set($('#composer legend'), '', `to <b>${esc(name)}</b>${busy ? ' · <em>working</em>, a message queues' : st === 'blocked' ? ' · <em>waiting on you</em>' : st === 'asleep' || st === 'stopped' ? ` · ${st}, a message resumes it` : ''}`, true);
    set($('#send'), '', busy ? 'queue' : 'send');
    $('#halt').hidden = !(busy || st === 'blocked');
    $('#steer').hidden = $('#steersep').hidden = !busy;
    const mode = (info.permissionModes || []).find(m => m.id === info.permissionMode);
    $('#modeb').hidden = !info.permissionMode;
    set($('#modeb'), '', `mode <b>${esc(mode ? mode.name || mode.id : info.permissionMode || '')}</b>`, true);
    $('#tail').hidden = !busy;
    set($('#tailsay'), '', S.status || info.detail || (st === 'starting' ? 'starting' : 'working'));
    tick(); drawNotices(); drawStrip();
  };
  const tick = () => { set($('#tailtime'), '', S.sentAt ? dur((Date.now() - S.sentAt) / 1000) : ''); };
  const ticker = setInterval(tick, 1000);
  sc.on(() => clearInterval(ticker));

  let noticed = '';
  const drawNotices = () => {
    const n = [], r = info.retry, l = info.limit, aw = info.away, held = aw && (aw.held || []).length ? ` · ${aw.held.length} held for you` : '';
    if (lostConn) n.push(['warn', navigator.onLine === false ? 'You are offline. Messages wait in the box.' : 'Connection lost. Trying again…']);
    if (info.error) n.push(['bad', info.error]);
    if (r) n.push(['warn', r.gaveUp ? `gave up: ${r.why || r.reason}` : `retrying ${r.attempt}/${r.max}${r.reason ? ' · ' + r.reason : ''}`]);
    if (l) n.push(['warn', `usage limit until ${clock(l.resetsAt)} · ${l.continue ? 'carries on then' : 'waits for you'}`, l.continue ? 'limit0' : 'limit1', l.continue ? "don't carry on" : 'carry on then']);
    if (aw && !aw.ended) n.push(['', `you're away${zero(aw.until) ? '' : ' until ' + clock(aw.until)}${held}`, 'back', "i'm back"]);
    else if (aw) n.push(['', `your away is over${held}`, 'back', 'ok']);
    const h = n.map(([k, text, act, word]) => `<div class=notice data-kind="${k}" role=status><span class=grow>${esc(text)}</span>${act ? `<button class=key data-n=${act}>${esc(word)}</button>` : ''}</div>`).join('');
    if (h !== noticed) $('#notices').innerHTML = noticed = h;
  };
  $('#notices').onclick = e => {
    const k = e.target.closest('[data-n]')?.dataset.n;
    if (k === 'back') op({op: 'away'});
    if (k === 'limit0' || k === 'limit1') op({op: 'limit', now: k === 'limit1'});
  };

  // the strip: its plan, what runs in the background, what's queued
  let stripped = '';
  const drawStrip = () => {
    const todos = S.todos || [], doing = todos.find(t => t.Status === 'in_progress'), done = todos.filter(t => t.Status === 'completed').length;
    const bg = [...S.tasks.values()].filter(t => !t.done), queue = info.queue || [];
    const h = (todos.length && done < todos.length ? `<details class=pin data-pin=plan><summary><span class=c-orange aria-hidden=true>■</span><span class=cut>${esc(doing ? doing.Active || doing.Label : 'plan')}</span><span class="c-dim num">${done}/${todos.length}</span></summary>
        <ul class=pin-body>${todos.map(t => `<li data-s="${esc(t.Status)}"><span aria-hidden=true>${t.Status === 'completed' ? '✓' : t.Status === 'in_progress' ? '▸' : '·'}</span><span>${esc(t.Label)}</span></li>`).join('')}</ul></details>` : '')
      + (bg.length ? `<button class="pin pin-btn" data-works aria-label="${bg.length} running beside the turn: open them"><span class=c-blue aria-hidden=true>⇉</span><span class=cut>${bg.length} running <span class=c-dim>· ${esc(bg[0].Label || bg[0].ID)}${bg[0].status ? ' · ' + esc(bg[0].status) : ''}</span></span></button>` : '')
      + (queue.length ? `<details class="pin pin-queue" data-pin=queue><summary><span class=c-queue>⋯ queue ${queue.length}</span><span class="cut c-dim">${info.queueHeld ? 'held' : 'sends when this turn ends'} · ${esc(queue[0])}</span></summary>
        <ul class=pin-body>${queue.map((m, i) => `<li><span class="c-dim num">${i + 1}</span><span class="cut grow c-text">${esc(m)}</span><button class="key key-plain" data-q=${i}>now</button><button class="key key-plain key-bad" data-r=${i}>remove</button></li>`).join('')}</ul></details>` : '');
    if (h === stripped) return;
    const open = new Set($$('details[open]', $('#strip')).map(d => d.dataset.pin));
    $('#strip').innerHTML = stripped = h;
    $$('details', $('#strip')).forEach(d => { d.open = open.has(d.dataset.pin); });
  };
  // the bar's ⇉ is there once anything has run beside the turn
  const drawWorks = () => {
    const b = $('#subs'), l = works(), run = l.filter(w => w.st === 'run').length;
    if (b) { b.hidden = !l.length; set(b, '', `<span class="${run ? 'c-orange' : ''}">⇉</span>${run || l.length}`, true); }
    drawSubs();
  };
  $('#strip').onclick = e => {
    const b = e.target.closest('button');
    if (!b) return;
    if ('works' in b.dataset) return openSubs('');
    if (b.dataset.q) op({op: 'queue_send', index: +b.dataset.q, was: info.queue[b.dataset.q]});
    if (b.dataset.r) op({op: 'queue_remove', index: +b.dataset.r, was: info.queue[b.dataset.r]});
  };

  // ── the message box ──
  // say sends the draft once. It stays in the box until the server has taken
  // it (serve handed it to the session, not that the agent has read it: the
  // conversation shows that), and a failed or unanswered request keeps it,
  // to be sent again by hand. Typing meanwhile is kept. A draft outlives
  // the tab: it's this browser's until sent.
  let sending = false, pics = [], recall = -1;
  const grow = () => { box.style.height = 'auto'; box.style.height = Math.min(box.scrollHeight, innerHeight * .4) + 'px'; };
  box.value = store.get('draft.' + key, '');
  const drawPics = () => {
    $('#pics').innerHTML = pics.map((p, i) => `<span class=chip><img src="data:${p.type};base64,${p.data}" alt="attached image ${i + 1}"><button data-i=${i} aria-label="Remove image ${i + 1}">×</button></span>`).join('');
  };
  $('#pics').onclick = e => { const i = e.target.dataset.i; if (i !== undefined && !sending) { pics.splice(+i, 1); drawPics(); } };
  const addFiles = async files => {
    for (const f of files) {
      if (!f.type.startsWith('image/')) continue;
      if (pics.length >= 8) { toast('A message takes 8 images at most.', 'bad'); break; }
      try { pics.push(await picture(f)); } catch (e) { toast(`${f.name || 'image'}: ${e}`, 'bad'); }
    }
    drawPics();
  };
  $('#pic').onclick = () => $('#file').click();
  $('#file').onchange = e => { addFiles([...e.target.files]); e.target.value = ''; };
  box.onpaste = e => { const fs = [...(e.clipboardData?.files || [])]; if (fs.length) { e.preventDefault(); addFiles(fs); } };
  const convo = $('#convo');
  convo.ondragover = e => { if ([...e.dataTransfer.types].includes('Files')) { e.preventDefault(); convo.classList.add('drop'); } };
  convo.ondragleave = e => { if (!convo.contains(e.relatedTarget)) convo.classList.remove('drop'); };
  convo.ondrop = e => { e.preventDefault(); convo.classList.remove('drop'); addFiles([...e.dataTransfer.files]); };
  const say = async guide => {
    const sent = box.value, t = sent.trim(), att = pics.slice();
    if ((!t && !att.length) || sending) return;
    if (t[0] === '!' && !att.length && !guide) {
      if (t.length > 1) shell(t.slice(1), true); else shell();
      box.value = ''; store.set('draft.' + key, ''); grow(); drawBang(); return;
    }
    sending = true; $$('#send,#steer').forEach(b => b.disabled = true); $('#send').classList.add('busy');
    try {
      await post(opPath, {op: 'send', text: t, guide, pictures: att.length ? att : undefined});
      pics = pics.filter(p => !att.includes(p)); drawPics();
      box.value = box.value === sent ? '' : box.value.startsWith(sent) ? box.value.slice(sent.length).trimStart() : box.value;
      store.set('draft.' + key, box.value); recall = -1;
      grow(); stick = true; fresh = false; bottom(); drawJump(); buzz(10);
    } catch (e) {
      if (e === 401) return signIn();
      toast(`Not confirmed: ${e}. Your message is still in the box. It may have been delivered; check the conversation before sending again.`, 'bad');
    } finally {
      sending = false; $$('#send,#steer').forEach(b => b.disabled = false); $('#send').classList.remove('busy');
    }
  };
  const drawBang = () => { const bang = box.value[0] === '!'; $('#composer').toggleAttribute('data-bang', bang); $('#send').textContent = bang ? 'run' : 'send'; };
  $('#send').onclick = () => say(false);
  $('#steer').onclick = () => say(true);
  $('#halt').onclick = () => { buzz(30); op({op: 'interrupt'}); };

  // a / at the start offers the agent's commands
  const menu = $('#menu');
  let menuAt = 0, menuList = [];
  const drawMenu = () => {
    const m = box.value.match(/^\/([\w:-]*)$/);
    menuList = m ? S.cmds.filter(c => c.Name.toLowerCase().includes(m[1].toLowerCase())).slice(0, 30) : [];
    menu.hidden = !menuList.length;
    menuAt = Math.min(menuAt, Math.max(0, menuList.length - 1));
    menu.innerHTML = menuList.map((c, i) => `<button role=option aria-selected=${i === menuAt} data-i=${i}><b>/${esc(c.Name)}</b><span>${esc(c.Description || c.ArgumentHint || '')}</span></button>`).join('');
    $('[aria-selected=true]', menu)?.scrollIntoView({block: 'nearest'});
  };
  const pickMenu = i => { box.value = '/' + menuList[i].Name + ' '; menu.hidden = true; menuList = []; box.focus(); store.set('draft.' + key, box.value); };
  menu.onclick = e => { const b = e.target.closest('[data-i]'); if (b) pickMenu(+b.dataset.i); };
  box.oninput = () => { grow(); store.set('draft.' + key, box.value); recall = -1; drawMenu(); drawBang(); };
  box.onkeydown = e => {
    if (e.isComposing) return;
    if (menuList.length) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); menuAt = (menuAt + (e.key === 'ArrowDown' ? 1 : menuList.length - 1)) % menuList.length; return drawMenu(); }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) { e.preventDefault(); return pickMenu(menuAt); }
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); menu.hidden = true; menuList = []; return; }
    }
    const mod = e.metaKey || e.ctrlKey;
    if (e.key === 'Enter' && mod) { e.preventDefault(); return say(!$('#steer').hidden); }
    if (e.key === 'Enter' && !e.shiftKey && fine.matches) { e.preventDefault(); return say(false); }
    if (e.key === 'c' && e.ctrlKey && !box.value && !$('#halt').hidden) { e.preventDefault(); return $('#halt').click(); }
    // ↑ in an empty box brings back what you said
    if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && (!box.value || recall >= 0) && S.sents.length) {
      const to = recall < 0 ? (e.key === 'ArrowUp' ? S.sents.length - 1 : -1) : recall + (e.key === 'ArrowUp' ? -1 : 1);
      if (to < 0 && recall < 0) return;
      e.preventDefault();
      recall = to >= S.sents.length || to < 0 ? -1 : to;
      box.value = recall < 0 ? '' : S.sents[recall]; grow();
    }
  };
  grow(); drawBang();
  if (fine.matches) box.focus();

  // ── changes: git's diff of the working folder against HEAD, and what's untracked, read on its machine ──
  const git = async args => {
    const r = await (await post('/git', {cwd: info.cwd, args})).json();
    return {out: new TextDecoder().decode(Uint8Array.from(atob(r.out || ''), c => c.charCodeAt(0))), error: r.error || ''};
  };
  const gitStat = async () => {
    if (!info.cwd) return;
    try {
      const [b, s] = await Promise.all([git(['rev-parse', '--abbrev-ref', 'HEAD']), git(['diff', 'HEAD', '--shortstat'])]);
      if (!sc.live) return;
      branch = b.error ? '' : b.out.trim().split('\n')[0];
      const a = s.out.match(/(\d+) insertion/), d = s.out.match(/(\d+) deletion/);
      stat = a || d ? ` <span class=add>+${a ? a[1] : 0}</span> <span class=del>−${d ? d[1] : 0}</span>` : '';
      head();
    } catch {}
  };
  const changes = async () => {
    const d = sheet('<div id=chgbody><div class=log-skel><div class=skeleton></div><div class=skeleton></div><div class=skeleton></div></div></div>', {title: 'Changes in ' + home(info.cwd), big: true});
    try {
      const [df, u] = await Promise.all([git(['diff', 'HEAD', '--no-color', '--no-ext-diff']), git(['ls-files', '--others', '--exclude-standard', '--full-name', '--', ':/'])]);
      if (df.error && !df.out) throw df.error;
      const files = df.out.split(/^(?=diff --git )/m).filter(p => p.startsWith('diff --git')).map(p => {
        const name = (p.match(/^\+\+\+ b\/(.*)$/m) || p.match(/^diff --git a\/(.*?) b\//) || [])[1] || '?', at = p.indexOf('\n@@');
        const body = at < 0 ? '' : p.slice(at + 1).replace(/\n$/, ''), lines = body.split('\n');
        return {name, body, add: lines.filter(l => l[0] === '+').length, del: lines.filter(l => l[0] === '-').length, gone: /^deleted file/m.test(p), made: /^new file/m.test(p)};
      });
      const un = u.out.trim() ? u.out.trim().split('\n') : [];
      const adds = files.reduce((n, f) => n + f.add, 0), dels = files.reduce((n, f) => n + f.del, 0);
      $('#chgbody', d).innerHTML = !files.length && !un.length ? `<div class=empty><p>No changes against HEAD.</p></div>`
        : `<p class=c-sub>${branch ? `<span class=c-green>${esc(branch)}</span> · ` : ''}<span class=add>+${adds}</span> <span class=del>−${dels}</span> in ${plural(files.length, 'file')}${un.length ? ` · ${un.length} untracked` : ''}</p>`
          + files.map(f => `<details class=file${files.length <= 4 ? ' open' : ''}><summary><span class=path>${esc(f.name)}</span>${f.made ? '<span class=c-dim>new</span>' : f.gone ? '<span class=c-dim>deleted</span>' : ''}<span class=add>+${f.add}</span><span class=del>−${f.del}</span></summary><pre class=wrap>${f.body ? diffLines(f.body) : '<span class=hunk>no text to show (a binary file, or only its mode changed)</span>'}</pre></details>`).join('')
          + (un.length ? `<details class=file><summary><span class=path>untracked</span><span class=c-dim>${un.length}</span></summary><pre class=wrap>${esc(un.join('\n'))}</pre></details>` : '');
    } catch (e) { if (e === 401) return signIn(); $('#chgbody', d).innerHTML = `<div class=err-line>${esc(e)}</div>`; }
  };

  // ── the shell: one command at a time in the session's folder, what `!` does in rush ──
  const shKey = () => 'sh.' + machine, runs = shellRuns.get(key) || shellRuns.set(key, []).get(key);
  const plainOut = t => t.replace(/\x1b\[[0-9;?]*[ -\/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[@-_]/g, '').replace(/^.*\r(?!\n)/gm, '').replace(/\r/g, '');
  const toAgent = r => `I ran \`${r.cmd}\` (${r.code === null ? r.error || 'no exit' : 'exit ' + r.code}):\n\`\`\`\n${r.out.trimEnd().split('\n').slice(-80).join('\n').slice(-OUT_MOST)}\n\`\`\`\n`;
  let shOpen = null;
  const shDraw = () => {
    if (!shOpen) return;
    const lg = $('#shlog', shOpen), past = store.get(shKey(), []), busy = runs.some(r => r.busy);
    lg.innerHTML = runs.length ? runs.map((r, i) => `<div class=run data-i=${i} data-s=${r.busy ? 'run' : r.code === 0 ? 'ok' : 'err'}>
        <div class=run-cmd><span class=prompt aria-hidden=true>$</span><code>${hl(r.cmd, 'sh')}</code></div>
        ${r.out ? `<pre class=wrap>${r.cut ? '<span class=hunk>… the start was dropped</span>\n' : ''}${esc(r.out)}</pre>` : ''}
        <div class=run-foot>${r.busy ? `<span class=mark-run aria-hidden=true>✻</span> running` : `<span class=run-exit>${r.code === null ? esc(r.error || 'failed') : 'exit ' + r.code}</span> · ${dur(r.took / 1000)}`}
          ${r.busy ? '' : `<span class=grow></span><button data-a=agent>to the agent</button><button data-a=copy>copy</button><button data-a=again>again</button>${/^stopped after/.test(r.error || '') && r.limit < 600 ? '<button data-a=long>again, 10 min</button>' : ''}`}</div></div>`).join('')
      : `<div class=empty><p>Runs in <span class=c-blue>${esc(home(info.cwd))}</span> on ${esc(machine)}, with your login shell. A command gets a minute, with nothing to type into.</p></div>`;
    lg.scrollTop = lg.scrollHeight;
    $('#shquick', shOpen).innerHTML = [...new Set([...past, 'git status', 'git log --oneline -10', 'ls -la'])].slice(0, 12).map(c => `<button class=key data-c="${esc(c)}">${esc(c)}</button>`).join('');
    $('#shgo', shOpen).disabled = busy; $('#shgo', shOpen).classList.toggle('busy', busy);
  };
  const shRun = async (cmd, limit = 60) => {
    cmd = cmd.trim();
    if (!cmd || runs.some(r => r.busy)) return;
    store.set(shKey(), [cmd, ...store.get(shKey(), []).filter(c => c !== cmd)].slice(0, 30));
    const r = {cmd, out: '', code: null, error: '', busy: true, took: 0, limit, cut: false}, t0 = Date.now();
    runs.push(r); if (runs.length > 40) runs.shift();
    shDraw();
    try {
      const v = await (await post(`/sessions/${enc(id)}/shell`, {command: cmd, timeout: limit})).json();
      r.out = plainOut(new TextDecoder().decode(Uint8Array.from(atob(v.out || ''), c => c.charCodeAt(0))));
      r.code = v.code ?? null; r.error = v.error || ''; r.cut = !!v.cut;
    } catch (e) {
      if (e === 401) return signIn();
      r.error = String(e);
    }
    r.busy = false; r.took = Date.now() - t0; buzz(r.code === 0 ? 10 : [20, 40, 20]);
    shDraw();
    if (!shOpen && sc.live) toast(`${r.cmd}: ${r.code === null ? r.error : 'exit ' + r.code}`, r.code === 0 ? '' : 'bad', {name: 'see', run: () => shell()});
  };
  const shell = (cmd, run) => {
    if (!shOpen) {
      shOpen = sheet(`<div class=shell><div id=shlog class=shlog role=log aria-label="Shell output"></div><div id=shquick class=shquick aria-label="Earlier commands"></div>
        <form id=shform><fieldset class=box><legend></legend><span class=prompt aria-hidden=true>$</span><input id=shin aria-label=Command placeholder="a command" autocapitalize=off autocorrect=off autocomplete=off spellcheck=false enterkeyhint=go><button class="key key-on" id=shgo>run</button></fieldset></form></div>`,
        {title: 'Shell in ' + home(info.cwd), big: true});
      const d = shOpen, inp = $('#shin', d);
      let at = -1;
      d.addEventListener('close', () => { if (shOpen === d) shOpen = null; });
      $('#shform', d).onsubmit = e => { e.preventDefault(); const c = inp.value; inp.value = ''; at = -1; shRun(c); };
      inp.onkeydown = e => {
        const past = store.get(shKey(), []);
        if ((e.key !== 'ArrowUp' && e.key !== 'ArrowDown') || !past.length) return;
        e.preventDefault();
        at = Math.max(-1, Math.min(past.length - 1, at + (e.key === 'ArrowUp' ? 1 : -1)));
        inp.value = at < 0 ? '' : past[at];
      };
      $('#shquick', d).onclick = e => { const b = e.target.closest('[data-c]'); if (b) { inp.value = b.dataset.c; inp.focus(); } };
      $('#shlog', d).onclick = e => {
        const b = e.target.closest('[data-a]'), r = b && runs[+b.closest('.run').dataset.i];
        if (!r) return;
        if (b.dataset.a === 'copy') copy(r.out || r.cmd);
        if (b.dataset.a === 'again') shRun(r.cmd, r.limit);
        if (b.dataset.a === 'long') shRun(r.cmd, 600);
        if (b.dataset.a === 'agent') {
          box.value = (box.value.trim() ? box.value.trimEnd() + '\n\n' : '') + toAgent(r);
          store.set('draft.' + key, box.value); d.close(); grow(); box.focus(); box.scrollTop = box.scrollHeight;
        }
      };
      if (fine.matches || !runs.length) inp.focus();
    }
    if (cmd) { if (run) shRun(cmd); else $('#shin', shOpen).value = cmd; }
    shDraw();
  };

  // ── details: how it is, and what can be done with it from here ──
  let open = null;
  const details = () => { open = sheet('<div id=dt class=sheet-in></div>', {title: info.name || id}); open.addEventListener('close', () => open = null); drawDetails(); };
  let detailed = '';
  const drawDetails = () => {
    if (!open) { detailed = ''; return; }
    const st = stNow(), away = info.away && !info.away.ended, modes = info.permissionModes || [];
    const h = `<dl class=facts>
        <dt>state</dt><dd>${state(st, info.needs)}</dd>
        <dt>agent</dt><dd>${esc([agentName(info.kind), info.model, info.effort].filter(Boolean).join(' · '))}</dd>
        ${info.costUsd ? `<dt>spent</dt><dd class=num>${money(info.costUsd)}</dd>` : ''}
        ${info.contextTokens ? `<dt>context</dt><dd class=num>${tokens(info.contextTokens)} tokens</dd>` : ''}
        <dt>folder</dt><dd class=c-blue>${esc(home(info.cwd))}</dd>
        ${branch ? `<dt>branch</dt><dd class=c-green>${esc(branch)}</dd>` : ''}
        <dt>machine</dt><dd>${esc(machine)}</dd>
        ${zero(info.startedAt) ? '' : `<dt>started</dt><dd>${esc(day(info.startedAt))} ${esc(clock(info.startedAt))}</dd>`}</dl>
      ${modes.length ? `<fieldset class=box><legend>permission mode</legend><div class=seg role=radiogroup aria-label="Permission mode">${modes.map(m => `<button class=key role=radio aria-checked=${m.id === info.permissionMode} data-mode="${esc(m.id)}" title="${esc(m.description || '')}">${esc(m.name || m.id)}</button>`).join('')}</div>
        ${(modes.find(m => m.id === info.permissionMode) || {}).description ? `<p class=note>${esc(modes.find(m => m.id === info.permissionMode).description)}</p>` : ''}</fieldset>` : ''}
      <fieldset class=box><legend>away</legend>${away ? `<p class=c-sub>you're away${zero(info.away.until) ? '' : ' until ' + clock(info.away.until)}</p><div class=line><button class="key key-on" data-a=back>i'm back</button></div>`
        : `<p class=note>The agent carries on while you're gone, checked on every 30 minutes, and holds what needs you.</p><div class=seg>${[1, 2, 4, 8].map(h => `<button class=key data-h=${h}>away ${h}h</button>`).join('')}</div>`}</fieldset>
      <div class=line>${full ? '' : '<button class=key data-a=all>whole history</button>'}<button class=key data-copy="${esc(location.href)}">copy link</button><span class=grow></span>${st === 'stopped' ? '' : '<button class="key key-bad" data-a=stop>stop the session</button>'}</div>`;
    if (h !== detailed) $('#dt', open).innerHTML = detailed = h;
    $('#dt', open).onclick = async e => {
      const b = e.target.closest('button');
      if (!b) return;
      if (b.dataset.mode) op({op: 'mode', mode: b.dataset.mode});
      if (b.dataset.a === 'back') op({op: 'away'});
      if (b.dataset.a === 'all') { open.close(); go(sessionHash(id) + '/all'); }
      if (b.dataset.a === 'stop') {
        open.close();
        if (await confirmSheet('Stop the session', 'Its agent is ended. The conversation is kept, and a message resumes it.', 'stop it', true)) op({op: 'stop'});
      }
      if (b.dataset.h) {
        const t = new Date();
        op({op: 'away', away: {from: t.toISOString(), until: new Date(+t + b.dataset.h * 3600e3).toISOString(), next: '0001-01-01T00:00:00Z'}}).then(ok => ok && toast(`away for ${b.dataset.h}h`));
      }
    };
  };
  $('#modeb').onclick = details;
  $('#shb').onclick = () => shell();
  bar({title: esc(info.name || id), actions: [action('subs', '⇉', () => openSubs(''), 'Subagents and background work'), action('chg', 'changes', changes), action('info', '⋯', details, 'Details')]});
  $('#subs').hidden = true;
  view.keys = {c: changes, i: details, s: () => openSubs(''), '!': () => shell(), e: () => box.focus(),
    'alt+KeyA': () => $('.ask [data-k=a]')?.click(), 'alt+KeyD': () => $('.ask [data-k=d]')?.click()};
  view.acts = [{name: 'Changes in this session', hint: 'c', run: changes}, {name: 'Details of this session', hint: 'i', run: details}, {name: 'Shell in this folder', hint: '!', run: () => shell()},
    {name: 'Stop the turn', run: () => $('#halt').click()}, {name: 'Subagents and background work', hint: 's', run: () => openSubs('')}, {name: 'New session in this folder', run: () => { newIn = info.cwd || ''; go('#/new'); }},
    {name: 'Whole history of this session', run: () => go(sessionHash(id) + '/all')}];

  // ── a step: one tool call, folded to a line; a run of them, to one ──
  const diff = ed => `<pre class=wrap>${ed.Old ? ed.Old.split('\n').map(x => `<span class=del>- ${esc(x)}</span>`).join('') : ''}${(ed.New || '').split('\n').map(x => `<span class=add>+ ${esc(x)}</span>`).join('')}</pre>`;
  const callBody = c => {
    const i = c.Input || {};
    if (i.Edits && i.Edits.length) return i.Edits.map(e => `<div class=path>${esc(home(e.Path || i.Path))}</div>${diff(e)}`).join('');
    if (i.Content) return `<div class=path>${esc(home(i.Path))}</div><pre class=wrap>${esc(i.Content.slice(0, OUT_MOST))}</pre>`;
    if (i.Command) return `<pre class=wrap>${hl(i.Command, 'sh')}</pre>`;
    if (i.Prompt) return `<pre class=wrap>${esc(i.Prompt.slice(0, OUT_MOST))}</pre>`;
    return [i.Path && `<div class=path>${esc(home(i.Path))}</div>`, (i.Pattern || i.Query) && `<pre class=wrap>${esc(i.Pattern || i.Query)}</pre>`, i.Url && `<div class=path>${esc(i.Url)}</div>`].filter(Boolean).join('');
  };
  const callWhat = c => { const i = c.Input || {}; return i.Description || home(i.Path) || i.Pattern || i.Query || i.Url || (i.Command || '').split('\n')[0] || c.Title || c.Name; };
  const callMore = c => { const i = c.Input || {}; return i.Description ? (i.Command || '').split('\n')[0] || i.Agent || '' : i.Path && i.Pattern ? i.Pattern : ''; };
  const callEl = c => {
    const e = el(`<details class=step data-id="${esc(c.ID)}" data-s=run data-name="${esc(c.Name)}"><summary><span class=step-mark>✻</span><span class=glyph aria-hidden=true>${glyphs[c.Name] || '·'}</span><span class=what></span><span class=more></span></summary><div class=step-body><div class=step-in></div><div class=step-out></div></div></details>`);
    callSet(e, c);
    return e;
  };
  const callSet = (e, c) => { e._c = c; set(e, '.what', callWhat(c)); set(e, '.more', callMore(c)); set(e, '.step-in', callBody(c), true); };
  // sum says what a run of steps came to, on its one line
  const sum = g => {
    if (!g) return;
    const steps = $$('.step', g), run = steps.filter(s => s.dataset.s === 'run'), bad = steps.filter(s => s.dataset.s === 'err').length;
    g.dataset.n = steps.length;
    g.dataset.s = run.length ? 'run' : bad ? 'err' : 'ok';
    set(g, '.steps-mark', run.length ? '✻' : bad ? '✗' : '✓');
    const by = new Map();
    for (const s of steps) { const d = did[s.dataset.name] || ['used', s.dataset.name]; const k = d.join(' '); by.set(k, [d, (by.get(k) || [d, 0])[1] + 1]); }
    const words = [...by.values()].map(([d, n]) => d[0] === 'used' ? `${d[1]}${n > 1 ? ' ×' + n : ''}` : `${d[0]} ${plural(n, d[1])}`).join(', ');
    set(g, '.steps-what', run.length ? $('.what', run[run.length - 1]).textContent : words);
    set(g, '.steps-n', `${plural(steps.length, 'step')}${bad ? ` · <em>${bad} failed</em>` : ''}`, true);
  };
  // stepIn adds a call to a thread (the conversation, or a subagent's own): T keeps its run of steps and its calls
  const stepIn = (T, box, c) => {
    if (!T.group) { T.group = el('<div class=steps data-n=0><button class=steps-head aria-expanded=false><span class=steps-mark></span><span class=steps-what></span><span class=steps-n></span></button><div class=steps-list></div></div>'); box.append(T.group); }
    const s = callEl(c);
    $('.steps-list', T.group).append(s); T.calls.set(c.ID, s);
    if (c.Name === 'Agent' || c.Name === 'Task') { S.agents.push(c.ID); s.dataset.sub = ''; }
    sum(T.group);
  };
  const step = c => { who(); stepIn(S, into, c); S.steps++; down(); };
  const finish = (s, bad) => { s.dataset.s = bad ? 'err' : 'ok'; set(s, '.step-mark', bad ? '✗' : '✓'); sum(s.closest('.steps')); };
  const output = (s, text, bad, more) => {
    const o = $('.step-out', s);
    if (more) text = (o._t || '') + text;
    o._t = text;
    const cutAt = text.length > OUT_MOST ? text.length - OUT_MOST : 0;
    o.classList.toggle('bad', !!bad);
    o.innerHTML = text ? `${cutAt ? `<div class=note>… ${cutAt} characters before this</div>` : ''}<pre>${esc(text.slice(cutAt))}</pre>` : '';
  };
  // taps in a thread: a run of steps opens, a subagent's step opens its own thread, copy copies
  const taps = e => {
    const h = e.target.closest('.steps-head');
    if (h) { const g = h.parentNode, o = g.toggleAttribute('data-open'); h.setAttribute('aria-expanded', o); return; }
    const a = e.target.closest('.step[data-sub] > summary');
    if (a) { e.preventDefault(); return openSubs(a.parentNode.dataset.id); }
    const c = e.target.closest('[data-say]');
    if (c) return copy(c.parentNode._raw || c.parentNode.textContent);
  };
  log.onclick = e => {
    taps(e);
    if (e.target.closest('#earlier')) { earlier = {id, h: scroll.scrollHeight - scroll.scrollTop}; go(sessionHash(id) + '/all'); }
  };

  // ── what it said, what you said ──
  const who = () => {
    if (S.said) return;
    S.said = true;
    if (S.lastYou) html(`<div class=who><b class=c-orange>${esc(agentName(info.kind))}</b>${now() ? `<time>${esc(clock(now()))}</time>` : ''}</div>`);
  };
  const sayEl = text => { const e = el(`<div class="say md">${md(text)}<button class=say-copy data-say>copy</button></div>`); e._raw = text; return e; };
  const you = (text, images) => {
    S.live = null; S.group = null; S.said = false; S.steps = 0; S.sentAt = now(); S.status = '';
    S.lastYou = html(`<div class=you><div class=you-head><b>You</b>${now() ? `<time>${esc(clock(now()))}</time>` : ''}</div><div class=you-text>${esc(text)}${images ? ` <span class=c-dim>+ ${plural(images, 'image')}</span>` : ''}</div></div>`);
    S.lastYou._t = text;
    if (text && S.sents[S.sents.length - 1] !== text) S.sents.push(text);
  };
  // what streams in is drawn once a frame
  let paints = false;
  const paint = () => {
    if (paints) return;
    paints = true;
    requestAnimationFrame(() => { paints = false; const l = S.live; if (l && l._dirty && sc.live) { l._dirty = false; l.innerHTML = md(l._s); down(); } });
  };

  // ── subagents, and what else runs beside the turn ──
  // A message with a Parent was written by the subagent that tool call
  // started. It's kept in that call's own thread, off the conversation, and
  // shown when you open it: a tap on its step, the ⇉ in the bar, or the
  // strip while it runs.
  const subOf = id => {
    let t = S.subs.get(id);
    if (!t) { t = {id, el: el('<div class="log sublog"></div>'), group: null, calls: new Map(), said: 0}; S.subs.set(id, t); }
    return t;
  };
  const subMessage = v => {
    const t = subOf(v.Parent);
    for (const p of v.Parts || []) {
      if (p.Kind === 3 && p.Output) {
        const o = p.Output, s = t.calls.get(o.CallID) || S.calls.get(o.CallID);
        if (s) { const text = [o.Text || o.Stdout, o.Stderr].filter(Boolean).join('\n'); if (text) output(s, text, o.IsError); finish(s, o.IsError); }
      } else if (p.Kind === 0 && p.Text && p.Text.trim()) {
        t.group = null; t.said++;
        t.el.append(v.Role === 'assistant' ? sayEl(p.Text) : el(`<div class=you><div class=you-head><b>${v.Injected ? 'told' : 'asked'}</b></div><div class=you-text>${esc(p.Text)}</div></div>`));
      } else if (p.Kind === 2 && p.Call) stepIn(t, t.el, p.Call);
    }
    if (!re && subAt === t.id) subDown();
  };
  const anyCall = id => { let s = S.calls.get(id); if (!s) for (const t of S.subs.values()) if ((s = t.calls.get(id))) break; return s; };
  const kinds = ['command', 'subagent', 'monitor', 'workflow', 'task'];
  // works is everything that ran beside the turn: each subagent call, and each task that isn't one of them
  const works = () => {
    const out = [], seen = new Set(), tasks = [...S.tasks.values()];
    for (const id of S.agents) {
      const s = anyCall(id), t = tasks.find(t => t.CallID === id), i = (s && s._c.Input) || {};
      if (t) seen.add(t.ID);
      out.push({id, task: t, kind: 'subagent', label: (t && t.Label) || i.Description || 'subagent', agent: (t && t.Agent) || i.Agent || '', st: t ? (t.done ? (t.bad ? 'err' : 'ok') : 'run') : (s && s.dataset.s) || 'ok', sub: S.subs.get(id), step: s});
    }
    for (const t of tasks) if (!seen.has(t.ID)) out.push({id: t.CallID || t.ID, task: t, kind: t.KindWord || kinds[t.Kind] || 'task', label: t.Label || t.ID, agent: t.Agent || '', st: t.done ? (t.bad ? 'err' : 'ok') : 'run', sub: S.subs.get(t.CallID), step: anyCall(t.CallID)});
    return out.sort((a, b) => (a.st === 'run' ? 0 : 1) - (b.st === 'run' ? 0 : 1));
  };
  let subSheet = null, subAt = '', subbed = '';
  const subDown = () => { const m = subSheet && $('#submount', subSheet); if (m && m._stick !== false) requestAnimationFrame(() => { m.scrollTop = m.scrollHeight; }); };
  const openSubs = at => {
    subAt = at;
    if (!subSheet) {
      subSheet = sheet('<div id=subbody></div>', {title: 'Beside the turn', big: true});
      subSheet.addEventListener('close', () => { subSheet = null; subAt = ''; subbed = ''; });
      subSheet.addEventListener('click', async e => {
        taps(e);
        const b = e.target.closest('button');
        if (!b) return;
        if (b.dataset.w !== undefined) openSubs(b.dataset.w);
        if ('back' in b.dataset) openSubs('');
        if (b.dataset.halt) { b.disabled = true; if (!await op({op: 'stop_task', id: b.dataset.halt})) b.disabled = false; }
      });
      subSheet.addEventListener('submit', async e => {
        e.preventDefault();
        const i = $('input', e.target), w = works().find(w => w.id === subAt);
        if (!i.value.trim() || !w || !w.task) return;
        if (await op({op: 'tell', id: w.task.ID, text: i.value.trim()})) { toast('told ' + w.label); i.value = ''; }
      });
    }
    subbed = ''; drawSubs();
  };
  const markOf = st => st === 'run' ? '✻' : st === 'err' ? '✗' : '✓';
  const stateOfWork = st => st === 'run' ? 'working' : st === 'err' ? 'failed' : 'idle';
  const drawSubs = () => {
    if (!subSheet) return;
    const body = $('#subbody', subSheet), l = works(), w = subAt && l.find(w => w.id === subAt);
    if (!w) {
      // the list: what runs first
      const h = l.length ? l.map(w => `<button class=row data-w="${esc(w.id)}" data-state=${stateOfWork(w.st)}><div class=row-link><div class=row-top><span class=mark aria-hidden=true>${markOf(w.st)}</span><b>${esc(w.label)}</b><span class=cost>${w.task && w.task.uses ? plural(w.task.uses, 'step') : ''}${w.task && w.task.tokens ? ' · ' + tokens(w.task.tokens) : ''}</span></div>
        <div class=row-sub><span class=tree aria-hidden=true>└</span><span class=why>${esc([w.agent || w.kind, w.st === 'run' ? (w.task && w.task.status) || 'running' : w.st === 'err' ? 'failed' : 'done'].filter(Boolean).join(' · '))}</span></div></div></button>`).join('')
        : '<div class=empty><p>Nothing has run beside the turn: no subagents, no background commands.</p></div>';
      if (h !== subbed) body.innerHTML = subbed = h;
      return;
    }
    const tell = w.st === 'run' && w.kind === 'subagent' && info.inbox && w.task;
    const h = `${w.id}|${w.st}|${w.label}|${(w.task && w.task.status) || ''}|${!!tell}|${!!w.sub}`;
    if (h !== subbed) {
      subbed = h;
      body.innerHTML = `<div class=subview><div class=line><button class="key key-plain" data-back aria-label="All of them">‹ all</button><span class="grow cut"><span class="st-${stateOfWork(w.st)}">${markOf(w.st)}</span> <b>${esc(w.label)}</b> <span class=c-dim>${esc([w.agent || w.kind, w.st === 'run' && w.task && w.task.status].filter(Boolean).join(' · '))}</span></span>
        ${w.st === 'run' && w.task ? `<button class="key key-bad" data-halt="${esc(w.task.ID)}">■ stop</button>` : ''}</div>
        <div id=submount class=scroller></div>
        ${tell ? '<form class=line><input class="field grow" placeholder="Tell it something" aria-label="Tell this subagent" enterkeyhint=send><button class="key key-on">tell</button></form>' : ''}</div>`;
      const m = $('#submount', body), i = (w.step && w.step._c.Input) || {}, out = w.step && $('.step-out', w.step);
      // its own thread; or for one with none (a command, an agent that doesn't say), what it was asked and what came back
      m.replaceChildren(w.sub ? w.sub.el : el(`<div class="log sublog">${i.Prompt || i.Command ? `<div class=you><div class=you-head><b>${i.Command ? 'ran' : 'asked'}</b></div><div class=you-text>${esc(i.Command || i.Prompt)}</div></div>` : ''}${
        out && out._t ? `<div class=say><pre class=wrap>${esc(out._t.slice(-OUT_MOST))}</pre></div>` : `<p class=aside>${w.st === 'run' ? 'Nothing from it yet.' : 'It left nothing to show.'}</p>`}</div>`));
      m.onscroll = () => { m._stick = m.scrollHeight - m.scrollTop - m.clientHeight < 80; };
      subDown();
    }
  };

  // ── what waits on you ──
  const askEl = (t, v) => {
    const a = document.createElement('div'); a.className = 'ask'; a.setAttribute('role', 'group');
    const acting = on => $$('.ask-acts button', a).forEach(b => b.disabled = on);
    const send = async o => { acting(true); buzz(20); if (!await op(o)) acting(false); };
    if (t === 'approval') {
      a.setAttribute('aria-label', 'Approval');
      a.innerHTML = `<h3>allow ${esc(v.Call.Name)}?</h3>${v.Reason ? `<p class=c-sub>${esc(v.Reason)}</p>` : ''}<div class=ask-body>${callBody(v.Call) || `<p class=c-sub>${esc(callWhat(v.Call))}</p>`}</div>
        <div class=ask-acts><button class="key key-on" data-k=a>allow</button><button class=key data-k=al>always</button><button class="key key-bad" data-k=d>deny</button></div>`;
      a.onclick = ev => { const k = ev.target.closest('[data-k]')?.dataset.k; if (k) send(k === 'd' ? {op: 'deny', id: v.ID} : {op: 'allow', id: v.ID, always: k === 'al'}); };
    } else {
      // a question: one option of each, or several where it says so
      a.setAttribute('aria-label', 'Question');
      const qs = v.Asks || [], picks = qs.map(() => new Set());
      a.innerHTML = `<h3>${esc(v.Title || 'a question')}</h3><div class=ask-body>` + qs.map((q, n) => `<section class=decision><p class=c-text>${esc(q.Text)}${q.Multi ? ' <span class=c-dim>(any of them)</span>' : ''}</p><div class=choices>${(q.Options || []).map((o, i) =>
        `<button class=choice data-n=${n} data-i=${i} aria-pressed=false>${esc(o.Label)}${o.Description ? `<small>${esc(o.Description)}</small>` : ''}</button>`).join('')}</div></section>`).join('') +
        '</div><div class=ask-acts><button class="key key-on" data-k=ok disabled>answer</button></div>';
      a.onclick = ev => {
        const b = ev.target.closest('button');
        if (!b || b.disabled) return;
        if (b.dataset.i) {
          const n = +b.dataset.n, i = +b.dataset.i, p = picks[n];
          if (qs[n].Multi) p.has(i) ? p.delete(i) : p.add(i); else { p.clear(); p.add(i); }
          $$(`.choice[data-n="${n}"]`, a).forEach(x => x.setAttribute('aria-pressed', p.has(+x.dataset.i)));
          $('[data-k=ok]', a).disabled = picks.some(p => !p.size);
        } else if (b.dataset.k === 'ok') {
          // the input AskUserQuestion takes: the questions, and answers by question text
          const answers = {};
          qs.forEach((q, n) => { answers[q.Text] = [...picks[n]].sort().map(i => q.Options[i].Label).join(', '); });
          send({op: 'allow', id: v.ID, input: {title: v.Title || undefined, answers, questions: qs.map(q => ({question: q.Text, header: q.Header, multiSelect: !!q.Multi,
            options: (q.Options || []).map(o => ({label: o.Label, description: o.Description, preview: o.Preview || undefined}))}))}});
        }
      };
    }
    return a;
  };
  const drop = i => { const a = asks.get(i); a && a.remove(); asks.delete(i); };

  // decisions: the block its last turn ended with (info.decisions), each
  // decision an ask and its options lettered, the agent's picks chosen to
  // start with. Your answers go as your next message in the block's own
  // numbering, as rush's card sends them (rush_host's decision_answer):
  // "1a. Topic: label" for an option, "2: your words" for words of your
  // own, one left unanswered left out. A card answered or dismissed is
  // remembered here, so the info that still carries it doesn't bring it back.
  const goneKey = 'rush.decided.' + key;
  let decided = sessionStorage.getItem(goneKey), decEl = null;
  const decisionsEl = v => {
    const a = document.createElement('div'); a.className = 'ask'; a.setAttribute('role', 'group'); a.setAttribute('aria-label', 'Decisions');
    const qs = v.Asks || [], pick = qs.map(q => (q.Options || []).findIndex(o => o.Picked));
    a.innerHTML = `<h3>${qs.length > 1 ? qs.length + ' decisions' : 'a decision'}</h3><div class=ask-body>` + qs.map((q, n) => {
      // its text is the lead-in, then "1. Topic"
      const name = `${q.ID}. ${q.Header}`, lead = (q.Text || '').endsWith(name) ? q.Text.slice(0, -name.length).trim() : q.Text || '';
      return `<section class=decision><h4>${esc(name)}</h4>${lead ? `<div class="lead md">${md(lead)}</div>` : ''}
        <div class=choices>${(q.Options || []).map((o, i) => `<button class=choice data-n=${n} data-i=${i} aria-pressed=${i === pick[n]}><b class=c-orange>${esc(o.Key)}</b> ${esc(o.Label)}${o.Picked ? ' <em>its pick</em>' : ''}${
          o.Description ? `<small>${esc(o.Description)}</small>` : ''}${o.Preview ? `<pre class=wrap>${esc(o.Preview)}</pre>` : ''}</button>`).join('')}</div>
        <input class=field data-own=${n} placeholder="or in your own words" aria-label="${esc(name)}, in your own words" enterkeyhint=done></section>`;
    }).join('') + '</div><div class=ask-acts><button class="key key-on" data-k=ok>send answers</button><button class=key data-k=no>dismiss</button></div>';
    const own = n => $(`[data-own="${n}"]`, a).value.trim();
    const draw = () => {
      $$('.choice', a).forEach(b => b.setAttribute('aria-pressed', +b.dataset.i === pick[b.dataset.n] && !own(b.dataset.n)));
      const n = qs.filter((q, n) => own(n) || pick[n] >= 0).length;
      $('[data-k=ok]', a).textContent = n === qs.length ? (n > 1 ? `send ${n} answers` : 'send answer') : `send ${n} of ${qs.length}`;
      $('[data-k=ok]', a).disabled = !n;
    };
    a.oninput = draw; // your own words stand in for an option
    const done = async o => {
      $$('.ask-acts button', a).forEach(b => b.disabled = true);
      if (await op(o)) { decided = v.ID; sessionStorage.setItem(goneKey, v.ID); a.remove(); if (decEl === a) decEl = null; }
      else { $$('.ask-acts button', a).forEach(b => b.disabled = false); draw(); } // it's kept, to send again
    };
    a.onclick = ev => {
      const b = ev.target.closest('button');
      if (!b || b.disabled) return;
      if (b.dataset.i) {
        // picked again, it's let go; picking one puts your own words aside
        const n = b.dataset.n, i = +b.dataset.i;
        pick[n] = pick[n] === i && !own(n) ? -1 : i;
        $(`[data-own="${n}"]`, a).value = '';
        draw();
      } else if (b.dataset.k === 'no') done({op: 'decisions_dismiss', id: v.ID});
      else if (b.dataset.k === 'ok') {
        const text = qs.map((q, n) => {
          const o = (q.Options || [])[pick[n]];
          return own(n) ? `${q.ID}: ${own(n)}` : o ? `${q.ID}${o.Key}. ${q.Header}: ${o.Label}` : '';
        }).filter(Boolean).join('\n\n');
        if (text) done({op: 'send', text});
      }
    };
    draw();
    return a;
  };
  const showDecisions = d => {
    if (decEl && (!d || decEl.dataset.id !== d.ID)) { decEl.remove(); decEl = null; }
    if (!d || d.ID === decided || decEl) return;
    decEl = decisionsEl(d); decEl.dataset.id = d.ID; $('#asks').append(decEl);
  };

  // ── the stream ──
  const works_ = () => { if (!re) { drawStrip(); drawWorks(); } };
  const replay = () => {
    if (re) clearTimeout(re.t);
    else re = {was: S, stick, top: scroll.scrollTop, at: Date.now()};
    into = document.createElement('div');
    S = blank();
    asks.forEach(a => { a.dataset.stale = ''; }); // one the replay brings again stays as it is
  };
  const settle = () => {
    const was = re;
    clearTimeout(was.t); re = null;
    if (!sc.live) return;
    if (!into.firstChild) into.innerHTML = '<p class=aside>Nothing said yet. A message starts it.</p>';
    // a step you opened stays open
    const open = new Set($$('[data-open], details[open][data-id]', log).map(d => d.dataset.id || d.firstElementChild?.dataset.id));
    $$('details[data-id]', into).forEach(d => { if (open.has(d.dataset.id)) { d.open = true; d.closest('.steps')?.setAttribute('data-open', ''); } });
    if (into.innerHTML === log.innerHTML) {
      // the same conversation: what's drawn stays, with what the replay learnt beside it
      const now = S;
      S = was.was; S.tasks = now.tasks; S.subs = now.subs; S.agents = now.agents; S.todos = now.todos; S.cmds = now.cmds.length ? now.cmds : S.cmds; S.status = now.status; S.sentAt = now.sentAt || S.sentAt;
    } else log.replaceChildren(...into.childNodes);
    into = log;
    asks.forEach((a, k) => { if ('stale' in a.dataset) { a.remove(); asks.delete(k); } });
    stick = was.stick;
    if (earlier && earlier.id === id && full) { scroll.scrollTop = scroll.scrollHeight - earlier.h; earlier = null; stick = atEnd(); }
    else if (stick) bottom(); else scroll.scrollTop = was.top;
    subbed = ''; drawJump(); head(); drawWorks();
  };
  const es = new EventSource(base + `/api/sessions/${enc(id)}/events` + (full ? '?full=1' : ''));
  sc.on(() => { es.close(); if (re) clearTimeout(re.t); });
  es.onopen = () => { if (lostConn) { lostConn = false; drawNotices(); } };
  es.onerror = () => {
    if (!sc.live) return;
    lostConn = true; drawNotices();
    if (es.readyState === 2) api('/machines').catch(e => e === 401 && signIn());
  };
  es.onmessage = m => {
    if (!sc.live) return;
    if (lostConn) { lostConn = false; drawNotices(); }
    const e = JSON.parse(m.data), v = e.e || {};
    if (!re && e.t !== 'reset' && $('.log-skel', log)) log.innerHTML = '';
    switch (e.t) {
      case 'reset': replay(); break;
      case 'stamp': S.at = Date.parse(v.At) || 0; break;
      case 'cut':
        into.prepend(el(full ? '<p class=aside>The earliest of it is too long to show.</p>' : '<div class=aside><button class="key key-plain" id=earlier>↑ show earlier</button></div>'));
        break;
      case 'sent': you(v.Text, (v.Images || []).length); break;
      case 'delta':
        if (v.Kind === 0) {
          who(); S.group = null;
          if (!S.live) { S.live = html('<div class="say md"></div>'); S.live._s = ''; }
          S.live._s += v.Text; S.live._dirty = true; paint();
        } else if (v.Kind === 1 && !S.status) { S.status = 'thinking'; head(); }
        break;
      case 'message':
        if (v.Parent) { subMessage(v); drawWorks(); break; }
        if (v.Role === 'assistant') {
          for (const p of v.Parts || []) {
            if (p.Kind === 0 && p.Text.trim()) {
              who(); S.group = null;
              const n = sayEl(p.Text);
              if (S.live) { S.live.replaceWith(n); S.live = null; down(); } else add(n);
            } else if (p.Kind === 1 && p.Text && p.Text.trim()) { who(); html(`<details class=think><summary>▸ thought</summary><div>${esc(p.Text)}</div></details>`); }
            else if (p.Kind === 2 && p.Call) step(p.Call);
          }
          if (S.status === 'thinking') S.status = '';
        } else for (const p of v.Parts || []) {
          // what you said, unless `sent` has just drawn it
          if (p.Kind === 0 && p.Text && !v.Injected && !(S.lastYou && S.lastYou._t === p.Text && !S.said)) you(p.Text, (v.Parts || []).filter(x => x.Kind === 4).length);
          if (p.Kind === 3 && p.Output) {
            const o = p.Output, s = S.calls.get(o.CallID);
            if (s) { const text = [o.Text || o.Stdout, o.Stderr].filter(Boolean).join('\n'); if (text) output(s, text, o.IsError); finish(s, o.IsError); }
          }
        }
        break;
      case 'call_updated': { const s = S.calls.get(v.Call.ID); if (s) { callSet(s, v.Call); sum(s.closest('.steps')); } } break;
      case 'call_output': { const s = S.calls.get(v.ID); if (s) output(s, v.Text || '', false, !v.Whole); } break;
      case 'denied': { const s = S.calls.get(v.CallID); if (s) { output(s, 'denied' + (v.Reason ? ': ' + v.Reason : ''), true); finish(s, true); } } break;
      case 'approval': case 'question': {
        const had = asks.get(v.ID);
        if (re && had) { delete had.dataset.stale; break; }
        drop(v.ID); const a = askEl(e.t, v); asks.set(v.ID, a); $('#asks').append(a);
        if (!re) buzz(60);
      } break;
      case 'approval_cancelled': case 'answered': drop(v.ID); break;
      case 'task_started': S.tasks.set(v.ID, {...S.tasks.get(v.ID), ...v}); works_(); break;
      case 'task_updated': case 'task_done': {
        const t = S.tasks.get(v.ID) || {ID: v.ID};
        if (v.Status) { t.done = true; t.bad = /fail|error|kill/i.test(v.Status) || !!v.Err; }
        if (v.Label) t.Label = v.Label;
        if (v.CallID) t.CallID = v.CallID;
        S.tasks.set(v.ID, t); works_();
      } break;
      case 'task_progress': { const t = S.tasks.get(v.ID); if (t) { t.status = v.Summary || v.LastTool || t.status; t.uses = v.ToolUses || t.uses; t.tokens = v.Tokens || t.tokens; works_(); } } break;
      case 'background': {
        const ids = new Set((v.Tasks || []).map(t => t.ID));
        S.tasks.forEach((t, k) => { if (!ids.has(k) && t.Background !== false) t.done = true; });
        (v.Tasks || []).forEach(t => { if (!S.tasks.has(t.ID)) S.tasks.set(t.ID, t); });
        works_();
      } break;
      case 'plan': S.todos = v.Todos || []; drawStrip(); break;
      case 'commands': S.cmds = v.List || []; break;
      case 'status': S.status = v.Busy ? v.Text || '' : ''; head(); break;
      case 'compacted': html(`<p class=aside>context compacted${v.Before ? ` · ${tokens(v.Before)} → ${tokens(v.After)}` : ''}</p>`); break;
      case 'start_notice': if (v.Text) html(`<p class=aside>${esc(v.Text)}</p>`); break;
      case 'turn_end': {
        S.live = null; S.group = null; S.status = '';
        if (v.Err) html(`<div class=err-line>✗ ${esc(v.Err)}</div>`);
        const bits = [/interrupt/.test(v.Reason || '') && 'interrupted', S.steps && plural(S.steps, 'step'), v.Duration > 0 && dur(v.Duration / 1e9), v.Cost > 0 && money(v.Cost)].filter(Boolean);
        if (bits.length) html(`<div class=meta>${bits.join(' · ')}</div>`);
        S.steps = 0; S.sentAt = 0; S.said = false;
        if (!re) gitStat();
      } break;
      case 'error': html(`<div class=err-line>✗ ${esc(v.Error)}</div>`); break;
      case 'info': {
        const first = !info.cwd;
        info = v; head(); drawDetails();
        if (first) gitStat();
        if (v.state && v.state !== 'blocked') { asks.forEach(a => a.remove()); asks.clear(); } // a decision no longer waited on is stale
        showDecisions(v.decisions);
      } break;
    }
    // the replay is in once nothing has come for a moment, or it has run long
    // (what streams as it's written doesn't hold it open)
    if (re) {
      if (Date.now() - re.at > 2500) settle();
      else if (e.t !== 'delta' || !re.t) { clearTimeout(re.t); re.t = setTimeout(settle, 120); }
    }
  };
  head();
  if (info.cwd) gitStat();
}
