// Caches nothing: the app and API are always live. Push shows what serve
// sent, and a tap opens the session it names.
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', e => e.waitUntil(clients.claim()));
self.addEventListener('fetch', () => {}); // installability wants a handler; the network answers
self.addEventListener('push', e => {
  const d = e.data ? e.data.json() : {};
  e.waitUntil(self.registration.showNotification(d.title || 'Rush', {body: d.body, tag: d.tag, data: d.url || '/', icon: '/icon.svg'}));
});
self.addEventListener('notificationclick', e => {
  e.notification.close();
  e.waitUntil(clients.matchAll({type: 'window'}).then(ws => {
    const u = new URL(e.notification.data, self.location.origin).href;
    const w = ws[0];
    if (w) { w.navigate(u); return w.focus(); }
    return clients.openWindow(u);
  }));
});
