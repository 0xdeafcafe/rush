// Drives the web app's controls in headless Chrome against fake hosts. Start
// the fixture first (see docs/remote.md), then:
//   PWDIR=<node_modules with playwright-core> node controls.js http://127.0.0.1:<port> <fixture dir>
const { chromium } = require(process.env.PWDIR + '/playwright-core');
const fs = require('fs');
const [url, dir] = process.argv.slice(2);
const tok = 'fixture' + 'x'.repeat(57);
const ops = () => fs.readFileSync(dir + '/ops.log', 'utf8').trim().split('\n').filter(Boolean);
const sleep = ms => new Promise(r => setTimeout(r, ms));
let fails = 0;
const check = (name, ok, extra = '') => { console.log((ok ? 'ok   ' : 'FAIL ') + name + (ok ? '' : ' ' + extra)); if (!ok) fails++; };
(async () => {
  const b = await chromium.launch({ channel: 'chrome', headless: true });
  const ctx = await b.newContext({ viewport: { width: 390, height: 800 } });
  const p = await ctx.newPage();
  const problems = [];
  p.on('console', m => { if (m.type() === 'error' && !/401|500/.test(m.text())) problems.push(m.text()); });
  p.on('pageerror', e => problems.push('pageerror ' + e.message));
  let dialogs = [];
  p.on('dialog', d => { dialogs.push(d.message()); d.accept(d.type() === 'prompt' ? d.defaultValue() : undefined); });
  await p.goto(url);
  await p.fill('#tok', tok); await p.click('#go');
  await p.waitForSelector('.row');
  check('lists the sessions, blocked first', (await p.locator('.row').count()) === 3 && /needs you/.test(await p.locator('.row').first().innerText()));
  await p.locator('.row').first().click();
  await p.waitForSelector('.ask');
  check('approval and question shown', (await p.locator('.ask').count()) === 2);
  const before = ops().length;

  // approve / always / deny
  const approval = p.locator('.ask', { hasText: 'Allow Bash' });
  await approval.locator('[data-k=a]').click(); await sleep(300);
  await approval.locator('[data-k=al]').click(); await sleep(300);
  await approval.locator('[data-k=d]').click(); await sleep(300);
  // question
  const q = p.locator('.ask', { hasText: 'Which?' });
  await q.locator('button', { hasText: /^B$/ }).click();
  await q.locator('[data-k=ok]').click(); await sleep(400);
  const got = ops().slice(before).map(l => JSON.parse(l.slice(l.indexOf(' ') + 1)));
  check('allow sent', got.some(o => o.op === 'allow' && o.id === 'ap1' && !o.always));
  check('allow always sent', got.some(o => o.op === 'allow' && o.id === 'ap1' && o.always === true));
  check('deny sent', got.some(o => o.op === 'deny' && o.id === 'ap1'));
  check('question answered with its choice', got.some(o => o.op === 'allow' && o.id === 'q1' && o.input && o.input.answers && o.input.answers['Pick one'] === 'B'), JSON.stringify(got));

  // send / steer / stop
  const n0 = ops().length;
  await p.fill('#txt', 'hello there'); await p.click('#send'); await sleep(400);
  check('send clears the box once accepted', (await p.inputValue('#txt')) === '');
  await p.fill('#txt', 'turn left'); await p.click('#steer'); await sleep(400);
  await p.click('#halt'); await sleep(400);
  const o2 = ops().slice(n0).map(l => JSON.parse(l.slice(l.indexOf(' ') + 1)));
  check('send op', o2.some(o => o.op === 'send' && o.text === 'hello there' && !o.guide));
  check('steer op', o2.some(o => o.op === 'send' && o.text === 'turn left' && o.guide === true));
  check('stop op', o2.some(o => o.op === 'interrupt'));

  // away, then back
  const n4 = ops().length;
  await p.click('#side [data-a=away]'); await sleep(500);
  const o4 = ops().slice(n4).map(l => JSON.parse(l.slice(l.indexOf(' ') + 1)));
  check('away sent with an end', o4.some(o => o.op === 'away' && o.away && o.away.until), JSON.stringify(o4));

  // an image: picked, shown, sent as bytes with the message, then cleared
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==', 'base64');
  const n3 = ops().length;
  await p.setInputFiles('#file', { name: 'dot.png', mimeType: 'image/png', buffer: png }); await sleep(300);
  check('picked image shown', await p.locator('#pics img').count() === 1);
  await p.fill('#txt', 'see this'); await p.click('#send'); await sleep(500);
  const o3 = ops().slice(n3).map(l => JSON.parse(l.slice(l.indexOf(' ') + 1)));
  const sentPic = o3.find(o => o.op === 'send' && o.text === 'see this');
  check('image reaches the host as a file of its own', !!sentPic && sentPic.images && sentPic.images.length === 1 && /remote-[0-9a-f]+\.png$/.test(sentPic.images[0]), JSON.stringify(o3));
  check('image cleared once sent', await p.locator('#pics img').count() === 0);

  // failure keeps the draft, and nothing retries
  let posts = 0;
  await p.route('**/op', r => { if (r.request().method() === 'POST') { posts++; return r.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"boom"}' }); } r.continue(); });
  dialogs = [];
  await p.fill('#txt', 'keep me'); await p.click('#send'); await sleep(800);
  check('failed send keeps the draft', (await p.inputValue('#txt')) === 'keep me');
  check('failed send says so', dialogs.length === 1 && /Not confirmed/.test(dialogs[0]), JSON.stringify(dialogs));
  check('one request, no retry', posts === 1, String(posts));
  check('buttons usable again', !(await p.locator('#send').isDisabled()));
  await p.unroute('**/op');

  // pending: no duplicate, typing meanwhile survives
  posts = 0;
  await p.route('**/op', async r => { if (r.request().method() === 'POST') { posts++; await sleep(700); } r.continue(); });
  await p.fill('#txt', 'once'); 
  await p.click('#send');
  check('send disabled while pending', await p.locator('#send').isDisabled());
  await p.evaluate(() => { document.querySelector('#send').disabled = false; document.querySelector('#send').click(); }); // a second tap anyway
  await p.fill('#txt', 'once and more');
  await sleep(1200);
  check('only one request while pending', posts === 1, String(posts));
  check('text typed after the sent part is kept, the sent part not', (await p.inputValue('#txt')) === 'and more', await p.inputValue('#txt'));
  await p.unroute('**/op');

  // changes of a session's folder, read on its machine
  await p.goto(url + '/#/s/fixture/9e9e9e9e'); await sleep(800);
  await p.click('#chg'); await sleep(800);
  const chg = await p.locator('#changes').innerText().catch(() => '');
  check('changes show the diff and what is untracked', /\+two/.test(chg) && /untracked: new\.txt/.test(chg), chg);

  // a past session from outside Rush, carried on in it
  const n5 = ops().length;
  await p.goto(url + '/'); await p.waitForSelector('#oth'); await p.click('#oth'); await p.waitForSelector('#ours');
  check('sessions Rush did not start listed', /the bug/.test(await p.locator('#main').innerText()));
  await p.locator('.row', { hasText: 'the bug' }).click(); await sleep(600);
  const rs = ops().slice(n5).find(l => l.startsWith('start '));
  check('picking one resumes it in Rush', !!rs && /"resume":"past-new"/.test(rs), rs);

  // create
  const n1 = ops().length;
  await p.click('#back'); await p.click('#new');
  await p.fill('#cwd', '/work/new'); await p.fill('#ag', 'claude'); await p.fill('#pr', 'do the thing'); await p.click('#mk'); await sleep(800);
  const st = ops().slice(n1).find(l => l.startsWith('start '));
  check('create sent', !!st && /"cwd":"\/work\/new"/.test(st) && /do the thing/.test(st), st);
  check('create opens the session', /#\/s\/fixture\/b2b2b2b2/.test(p.url()), p.url());
  check('no console errors / CSP violations', problems.length === 0, JSON.stringify(problems));
  await b.close();
  process.exit(fails ? 1 : 0);
})();
