//! rush's menu bar icon, the app itself: AppKit through objc2. Everything
//! it shows comes from `rush menubar feed`, which it runs and talks to one
//! JSON object per line. Port of internal/menubar/app.swift:96-500
//! (AppDelegate: the feed, the icon, the menu, notifications) and :690
//! (Main).
//!
//! Idle costs nothing: the feed's lines are read on a thread of their own
//! that blocks on the pipe and hands each to the main thread; the only
//! timer is the icon's, every other second while an agent works.

mod bottle;
mod views;

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write as _};
use std::process::{ChildStdin, Command, Stdio};
use std::ptr::NonNull;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSCellImagePosition,
    NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontAttributeName,
    NSFontWeightSemibold, NSForegroundColorAttributeName, NSImage, NSMenu, NSMenuDelegate,
    NSMenuItem, NSRunningApplication, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSBundle, NSDate, NSDefaultRunLoopMode, NSDictionary,
    NSNotification, NSNumber, NSRunLoop, NSSet, NSString, NSTimer, NSUserDefaults,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound,
    UNTextInputNotificationAction, UNTextInputNotificationResponse, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use crate::menu;
use crate::wire::{Line, Op, Waiting};

use views::{RowView, UsageView};

/// The feed as the app has it running: where its ops go.
struct Feed {
    input: ChildStdin,
    pid: u32,
}

pub struct Ivars {
    item: OnceCell<Retained<NSStatusItem>>,
    menu: OnceCell<Retained<NSMenu>>,
    /// The rush whose feed this runs.
    bin: String,
    feed: RefCell<Option<Feed>>,
    /// Which feed is the current one: lines and ends of an older one are
    /// dropped.
    run: Cell<u64>,
    state: RefCell<Line>,
    /// A first state has come; what's waiting then isn't news.
    got: Cell<bool>,
    problem: RefCell<Option<String>>,
    /// What each key's notification was asking.
    notified: RefCell<HashMap<String, String>>,
    categories: RefCell<HashMap<String, Retained<UNNotificationCategory>>>,
    /// What the icon shows, so drawing it again the same is skipped.
    drawn: RefCell<String>,
    tick: RefCell<Option<Retained<NSTimer>>>,
    images: OnceCell<[Retained<NSImage>; 3]>,
    /// The menu's answer items' ops, by their tags.
    ops: RefCell<Vec<Op>>,
    notes: OnceCell<Retained<Notes>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RushMenuBarDelegate"]
    #[ivars = Ivars]
    pub struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            if !self.alone() {
                return;
            }
            self.launch();
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            if let Some(f) = self.ivars().feed.borrow_mut().take() {
                // SAFETY: a plain signal to the feed this app started.
                unsafe { libc::kill(f.pid as i32, libc::SIGTERM) };
            }
        }
    }

    unsafe impl NSMenuDelegate for Delegate {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, m: &NSMenu) {
            self.fill(m);
        }
    }

    impl Delegate {
        #[unsafe(method(answerFromMenu:))]
        fn answer_from_menu(&self, sender: &NSMenuItem) {
            let op = self.ivars().ops.borrow().get(sender.tag() as usize).cloned();
            if let Some(op) = op {
                self.send(&op);
            }
        }

        #[unsafe(method(openRush:))]
        fn open_rush(&self, _sender: Option<&AnyObject>) {
            self.show("");
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: Option<&AnyObject>) {
            toggle_login();
        }
    }
);

thread_local! {
    static APP: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
}

/// Runs f with the app's delegate, on the main thread.
fn with_app(f: impl FnOnce(&Delegate)) {
    let d = APP.with(|a| a.borrow().clone());
    if let Some(d) = d {
        f(&d);
    }
}

/// Runs f with the app's delegate on the main thread, from anywhere.
fn on_main(f: impl FnOnce(&Delegate) + Send + 'static) {
    DispatchQueue::main().exec_async(move || with_app(f));
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn bundle_id() -> String {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .map_or_else(|| crate::BUNDLE_ID.to_string(), |s| s.to_string())
}

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Delegate> {
        let bin = NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&ns("RushBinary"))
            .and_then(|o| o.downcast::<NSString>().ok())
            .map_or_else(|| "rush".to_string(), |s| s.to_string());
        let this = Self::alloc(mtm).set_ivars(Ivars {
            item: OnceCell::new(),
            menu: OnceCell::new(),
            bin,
            feed: RefCell::new(None),
            run: Cell::new(0),
            state: RefCell::new(Line::default()),
            got: Cell::new(false),
            problem: RefCell::new(None),
            notified: RefCell::new(HashMap::new()),
            categories: RefCell::new(HashMap::new()),
            drawn: RefCell::new(String::new()),
            tick: RefCell::new(None),
            images: OnceCell::new(),
            ops: RefCell::new(Vec::new()),
            notes: OnceCell::new(),
        });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }

    fn mtm(&self) -> MainThreadMarker {
        MainThreadMarker::from(self)
    }

    /// Keeps to one icon: rushes starting together, or a new build opened
    /// over an old, can each launch one. The newest stays and the rest
    /// quit; false when this one is quitting.
    fn alone(&self) -> bool {
        let me = NSRunningApplication::currentApplication();
        let others: Vec<_> =
            NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns(&bundle_id()))
                .iter()
                .filter(|a| a.processIdentifier() != me.processIdentifier())
                .collect();
        let launched = |a: &NSRunningApplication| {
            a.launchDate()
                .map_or(f64::MIN, |d| d.timeIntervalSince1970())
        };
        let newer = |a: &NSRunningApplication, b: &NSRunningApplication| {
            let (x, y) = (launched(a), launched(b));
            if x != y {
                x > y
            } else {
                a.processIdentifier() > b.processIdentifier()
            }
        };
        if others.iter().any(|o| newer(o, &me)) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
            return false;
        }
        for o in &others {
            o.terminate();
        }
        true
    }

    fn launch(&self) {
        let mtm = self.mtm();
        let iv = self.ivars();
        // A new icon goes left of every other, which on a Mac with a notch
        // is often behind it; this one starts near the clock instead, and
        // stays wherever you ⌘-drag it after.
        let defaults = NSDictionary::<NSString, AnyObject>::from_slices(
            &[&*ns("NSStatusItem Preferred Position rush")],
            &[&*NSNumber::new_i64(120) as &AnyObject],
        );
        // SAFETY: a plist-typed default.
        unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        item.setAutosaveName(Some(&ns("rush")));
        item.setVisible(true);
        let m = NSMenu::new(mtm);
        m.setDelegate(Some(ProtocolObject::from_ref(self)));
        m.setAutoenablesItems(false);
        item.setMenu(Some(&m));
        let _ = iv.item.set(item);
        let _ = iv.menu.set(m);
        let notes = Notes::new();
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.setDelegate(Some(ProtocolObject::from_ref(&*notes)));
        let _ = iv.notes.set(notes);
        let asked = RcBlock::new(|_: objc2::runtime::Bool, _: *mut objc2_foundation::NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert
                | UNAuthorizationOptions::Sound
                | UNAuthorizationOptions::Badge,
            &asked,
        );
        self.draw();
        self.start_feed();
    }

    // ---- the feed ----

    fn start_feed(&self) {
        let iv = self.ivars();
        let run = iv.run.get() + 1;
        iv.run.set(run);
        let child = Command::new(&iv.bin)
            .args(["menubar", "feed"])
            .env("RUSH_MENUBAR_PID", std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => {
                *iv.problem.borrow_mut() = Some(format!(
                    "rush isn't at {} any more: run rush menubar again",
                    iv.bin
                ));
                self.draw();
                return;
            }
        };
        let (Some(input), Some(out)) = (child.stdin.take(), child.stdout.take()) else {
            return;
        };
        *iv.feed.borrow_mut() = Some(Feed {
            input,
            pid: child.id(),
        });
        *iv.problem.borrow_mut() = None;
        std::thread::Builder::new()
            .name("menubar-feed".into())
            .spawn(move || {
                for l in BufReader::new(out).split(b'\n') {
                    let Ok(l) = l else { break };
                    let Ok(f) = serde_json::from_slice::<Line>(&l) else {
                        continue;
                    };
                    on_main(move |d| {
                        if d.ivars().run.get() == run {
                            d.apply(f);
                        }
                    });
                }
                let _ = child.wait();
                on_main(move |d| {
                    if d.ivars().run.get() == run {
                        d.feed_ended();
                    }
                });
            })
            .expect("a thread to read the feed");
    }

    fn feed_ended(&self) {
        let iv = self.ivars();
        *iv.feed.borrow_mut() = None;
        iv.problem
            .borrow_mut()
            .get_or_insert_with(|| "lost touch with rush; trying again…".into());
        self.draw();
        let again = DispatchTime::try_from(Duration::from_secs(5)).unwrap_or(DispatchTime::NOW);
        let _ = DispatchQueue::main().after(again, || {
            with_app(|d| {
                if d.ivars().feed.borrow().is_none() {
                    d.start_feed();
                }
            })
        });
    }

    fn send(&self, op: &Op) {
        let mut feed = self.ivars().feed.borrow_mut();
        let Some(f) = feed.as_mut() else { return };
        let Ok(mut b) = serde_json::to_vec(op) else {
            return;
        };
        b.push(b'\n');
        let _ = f.input.write_all(&b);
    }

    fn apply(&self, f: Line) {
        if let Some(e) = &f.error {
            self.post("error", "rush", None, e, None, &[]);
            return;
        }
        *self.ivars().problem.borrow_mut() = None;
        *self.ivars().state.borrow_mut() = f;
        self.notify_waiting();
        self.ivars().got.set(true);
        self.draw();
    }

    // ---- the icon ----

    fn images(&self) -> &[Retained<NSImage>; 3] {
        self.ivars().images.get_or_init(|| {
            [
                bottle::bottle(false),
                bottle::bottle(true),
                bottle::tinted(bottle::bottle(false), NSColor::systemOrangeColor()),
            ]
        })
    }

    fn draw(&self) {
        let iv = self.ivars();
        let Some(b) = iv.item.get().and_then(|i| i.button(self.mtm())) else {
            return;
        };
        let (waiting, working) = {
            let s = iv.state.borrow();
            (
                s.waiting.as_ref().map_or(0, Vec::len),
                s.working.as_ref().map_or(0, Vec::len),
            )
        };
        // The bottle: working, its cap pops on and off every other second;
        // needing you, it's orange.
        let secs = rush_core::time::now() / 1000;
        let pose = menu::pose(waiting, working, secs);
        self.animate(waiting == 0 && working > 0);
        // Setting the button's image or title has the menu bar lay it out
        // and draw it again, even when they're what they were.
        let problem = iv.problem.borrow().is_some();
        let look = format!("{pose:?} {waiting} {problem}");
        if *iv.drawn.borrow() == look {
            return;
        }
        *iv.drawn.borrow_mut() = look;
        let [idle, cap_off, needs] = self.images();
        b.setImage(Some(match pose {
            menu::Pose::Needs => needs,
            menu::Pose::Working => cap_off,
            menu::Pose::Idle => idle,
        }));
        b.setAppearsDisabled(problem);
        b.setImagePosition(NSCellImagePosition::ImageLeading);
        let title = if waiting > 0 {
            // SAFETY: AppKit's own constant.
            let font =
                NSFont::monospacedDigitSystemFontOfSize_weight(NSFont::systemFontSize(), unsafe {
                    NSFontWeightSemibold
                });
            let color = NSColor::systemOrangeColor();
            // SAFETY: the keys are AppKit's own, with values of their types.
            unsafe {
                let attrs = NSDictionary::<NSString, AnyObject>::from_slices(
                    &[NSForegroundColorAttributeName, NSFontAttributeName],
                    &[&*color as &AnyObject, &*font as &AnyObject],
                );
                NSAttributedString::new_with_attributes(&ns(&format!(" {waiting}")), &attrs)
            }
        } else {
            NSAttributedString::from_nsstring(&ns(""))
        };
        b.setAttributedTitle(&title);
        let tip = if waiting > 0 {
            format!("{waiting} waiting on you")
        } else {
            "rush".into()
        };
        b.setToolTip(Some(&ns(&tip)));
    }

    /// Ticks while an agent works, on the even seconds the pose changes
    /// at, and not at all otherwise.
    fn animate(&self, on: bool) {
        let mut tick = self.ivars().tick.borrow_mut();
        if on == tick.is_some() {
            return;
        }
        if let Some(t) = tick.take() {
            t.invalidate();
        }
        if !on {
            return;
        }
        let now = rush_core::time::now() as f64 / 1000.0;
        let next = NSDate::dateWithTimeIntervalSince1970((now / 2.0).floor() * 2.0 + 2.0);
        let block = RcBlock::new(|_: NonNull<NSTimer>| with_app(|d| d.draw()));
        // SAFETY: the block only runs on the main run loop it's added to.
        let t = unsafe {
            NSTimer::initWithFireDate_interval_repeats_block(
                NSTimer::alloc(),
                &next,
                2.0,
                true,
                &block,
            )
        };
        t.setTolerance(0.1);
        // SAFETY: the main run loop, in its default mode.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&t, NSDefaultRunLoopMode) };
        *tick = Some(t);
    }

    // ---- the menu ----

    fn fill(&self, m: &NSMenu) {
        let mtm = self.mtm();
        let iv = self.ivars();
        m.removeAllItems();
        iv.ops.borrow_mut().clear();
        let view = |v: &objc2_app_kit::NSView| {
            let i = NSMenuItem::new(mtm);
            i.setView(Some(v));
            i
        };
        if let Some(p) = iv.problem.borrow().as_deref() {
            m.addItem(&view(&RowView::new(
                mtm,
                p,
                "",
                "",
                Some(NSColor::systemYellowColor()),
                None,
            )));
            m.addItem(&NSMenuItem::separatorItem(mtm));
        }
        let state = iv.state.borrow().clone();
        let now = rush_core::time::now();
        let waiting = state.waiting.unwrap_or_default();
        let working = state.working.unwrap_or_default();
        if !waiting.is_empty() {
            m.addItem(&NSMenuItem::sectionHeaderWithTitle(&ns("Needs You"), mtm));
            for w in &waiting {
                let detail = if w.text.is_empty() { &w.needs } else { &w.text };
                let row = RowView::new(
                    mtm,
                    &w.name,
                    detail,
                    &menu::ago(w.since, now),
                    Some(NSColor::systemOrangeColor()),
                    Some(w.key.clone()),
                );
                m.addItem(&view(&row));
                for (label, op) in menu::actions(w) {
                    let tag = {
                        let mut ops = iv.ops.borrow_mut();
                        ops.push(op);
                        ops.len() - 1
                    };
                    let i = self.item(&label, sel!(answerFromMenu:), "");
                    i.setTag(tag as isize);
                    i.setIndentationLevel(1);
                    m.addItem(&i);
                }
            }
        }
        if !working.is_empty() {
            if !waiting.is_empty() {
                m.addItem(&NSMenuItem::separatorItem(mtm));
            }
            m.addItem(&NSMenuItem::sectionHeaderWithTitle(&ns("Working"), mtm));
            for w in &working {
                let row = RowView::new(
                    mtm,
                    &w.name,
                    &w.doing,
                    &menu::ago(w.since, now),
                    Some(NSColor::systemGreenColor()),
                    Some(w.key.clone()),
                );
                m.addItem(&view(&row));
            }
            if let Some(more) = state.more.filter(|m| *m > 0) {
                let row = RowView::new(mtm, "", &format!("and {more} more"), "", None, None);
                m.addItem(&view(&row));
            }
        }
        if waiting.is_empty() && working.is_empty() && iv.got.get() {
            m.addItem(&view(&RowView::new(
                mtm,
                "",
                "Nothing running",
                "",
                None,
                None,
            )));
        }
        let accounts = state.accounts.unwrap_or_default();
        if !accounts.is_empty() {
            m.addItem(&NSMenuItem::separatorItem(mtm));
            m.addItem(&NSMenuItem::sectionHeaderWithTitle(&ns("Usage"), mtm));
            for a in accounts {
                let tip = menu::usage_tip(&a, now);
                let i = view(&UsageView::new(mtm, a));
                i.setToolTip(Some(&ns(&tip)));
                m.addItem(&i);
            }
        }
        m.addItem(&NSMenuItem::separatorItem(mtm));
        m.addItem(&self.item("Open rush", sel!(openRush:), "o"));
        let login = self.item("Open at Login", sel!(toggleLogin:), "");
        login.setState(if crate::bundle::login_plist().exists() {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        m.addItem(&login);
        // SAFETY: terminate: is NSApplication's, found through the
        // responder chain.
        let quit = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &ns("Quit"),
                Some(sel!(terminate:)),
                &ns("q"),
            )
        };
        m.addItem(&quit);
    }

    /// A menu item that calls one of this delegate's actions.
    fn item(&self, title: &str, action: objc2::runtime::Sel, key: &str) -> Retained<NSMenuItem> {
        // SAFETY: action is one of this delegate's, which is the target.
        unsafe {
            let i = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &ns(title),
                Some(action),
                &ns(key),
            );
            i.setTarget(Some(self));
            i
        }
    }

    /// Brings rush forward, on the agent key: the feed finds the terminal
    /// it's open in, or opens it in the one it last was.
    fn show(&self, key: &str) {
        if self.ivars().feed.borrow().is_some() {
            self.send(&Op {
                op: "show".into(),
                key: key.into(),
                ..Op::default()
            });
            return;
        }
        let _ = Command::new("/usr/bin/open").arg(&self.ivars().bin).spawn();
    }

    // ---- notifications ----

    /// Posts once for each new question and takes back the notifications
    /// of the ones answered or gone.
    fn notify_waiting(&self) {
        let iv = self.ivars();
        let state = iv.state.borrow();
        let waiting = state.waiting.as_deref().unwrap_or_default();
        let n = menu::notices(&iv.notified.borrow(), waiting, iv.got.get());
        if !n.gone.is_empty() {
            let ids: Vec<Retained<NSString>> = n.gone.iter().map(|k| ns(k)).collect();
            UNUserNotificationCenter::currentNotificationCenter()
                .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
        }
        for w in &n.post {
            self.notify(w);
        }
        *iv.notified.borrow_mut() = n.now;
    }

    fn notify(&self, w: &Waiting) {
        let acts = menu::actions(w);
        let mut category = None;
        if !acts.is_empty() {
            let mut buttons: Vec<Retained<UNNotificationAction>> = acts
                .iter()
                .enumerate()
                .map(|(i, (label, _))| {
                    let opts = if label == "Deny" {
                        UNNotificationActionOptions::Destructive
                    } else {
                        UNNotificationActionOptions::empty()
                    };
                    UNNotificationAction::actionWithIdentifier_title_options(
                        &ns(&format!("a.{i}")),
                        &ns(label),
                        opts,
                    )
                })
                .collect();
            if w.kind == "question" {
                let text = UNTextInputNotificationAction::actionWithIdentifier_title_options_textInputButtonTitle_textInputPlaceholder(
                    &ns("text"),
                    &ns("Answer…"),
                    UNNotificationActionOptions::empty(),
                    &ns("Send"),
                    &ns("Your answer"),
                );
                buttons.push(text.into_super());
            }
            let id = menu::category(w, &acts);
            let c =
                UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                    &ns(&id),
                    &NSArray::from_retained_slice(&buttons),
                    &NSArray::new(),
                    UNNotificationCategoryOptions::empty(),
                );
            let mut cats = self.ivars().categories.borrow_mut();
            cats.insert(id.clone(), c);
            let all: Vec<_> = cats.values().cloned().collect();
            UNUserNotificationCenter::currentNotificationCenter()
                .setNotificationCategories(&NSSet::from_retained_slice(&all));
            category = Some(id);
        }
        let mut info = vec![("key", w.key.as_str()), ("kind", w.kind.as_str())];
        if !w.req.is_empty() {
            info.push(("req", w.req.as_str()));
        }
        let body = if w.text.is_empty() { &w.needs } else { &w.text };
        self.post(
            &w.key,
            &w.name,
            Some(menu::subtitle(w)),
            body,
            category.as_deref(),
            &info,
        );
    }

    fn post(
        &self,
        id: &str,
        title: &str,
        subtitle: Option<&str>,
        body: &str,
        category: Option<&str>,
        info: &[(&str, &str)],
    ) {
        let c = UNMutableNotificationContent::new();
        c.setTitle(&ns(title));
        if let Some(s) = subtitle {
            c.setSubtitle(&ns(s));
        }
        c.setBody(&ns(body));
        c.setSound(Some(&UNNotificationSound::defaultSound()));
        let keys: Vec<Retained<NSString>> = info.iter().map(|(k, _)| ns(k)).collect();
        let vals: Vec<Retained<NSString>> = info.iter().map(|(_, v)| ns(v)).collect();
        let dict = NSDictionary::<NSString, NSString>::from_slices(
            &keys.iter().map(|k| &**k).collect::<Vec<_>>(),
            &vals.iter().map(|v| &**v).collect::<Vec<_>>(),
        );
        // SAFETY: a dictionary of strings, as userInfo takes; the cast only
        // forgets its generic types.
        unsafe { c.setUserInfo(&Retained::cast_unchecked::<NSDictionary>(dict)) };
        if let Some(cat) = category {
            c.setCategoryIdentifier(&ns(cat));
        }
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&ns(id), &c, None);
        // Categories register asynchronously; reading them back first
        // makes sure the buttons are there when the banner shows.
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let add = RcBlock::new(move |_: NonNull<NSSet<UNNotificationCategory>>| {
            UNUserNotificationCenter::currentNotificationCenter()
                .addNotificationRequest_withCompletionHandler(&req, None);
        });
        center.getNotificationCategoriesWithCompletionHandler(&add);
    }

    /// What a click on a notification, or one of its buttons, asks.
    fn respond(&self, key: &str, req: &str, action: &str, text: Option<String>) {
        // SAFETY: UserNotifications' own constant.
        if action == unsafe { UNNotificationDefaultActionIdentifier }.to_string() {
            self.show(key);
            return;
        }
        // The buttons were made from the wait as it was; answer it only if
        // it's still asking that.
        let w = {
            let s = self.ivars().state.borrow();
            s.waiting
                .as_deref()
                .unwrap_or_default()
                .iter()
                .find(|w| w.key == key)
                .cloned()
        };
        let Some(w) = w.filter(|w| w.req == req) else {
            return;
        };
        if action == "text" {
            if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
                self.send(&Op {
                    op: "answer".into(),
                    key: key.into(),
                    req: w.req.clone(),
                    answer: t,
                    ..Op::default()
                });
            }
            return;
        }
        let acts = menu::actions(&w);
        if let Some(i) = action
            .strip_prefix("a.")
            .and_then(|i| i.parse::<usize>().ok())
            && let Some((_, op)) = acts.get(i)
        {
            self.send(op);
        }
    }
}

/// Opens the app at login, or stops it: a launch agent that opens the
/// bundle.
fn toggle_login() {
    let p = crate::bundle::login_plist();
    if p.exists() {
        let _ = std::fs::remove_file(&p);
        return;
    }
    let bundle = NSBundle::mainBundle().bundlePath().to_string();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, login_plist(&bundle_id(), &bundle));
}

/// The launch agent's plist, as PropertyListSerialization writes it.
fn login_plist(label: &str, bundle: &str) -> String {
    let e = crate::bundle::xml_escape;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{}</string>
	<key>ProgramArguments</key>
	<array>
		<string>/usr/bin/open</string>
		<string>{}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
</dict>
</plist>
"#,
        e(label),
        e(bundle)
    )
}

define_class!(
    /// The notification center's delegate: it may be called off the main
    /// thread, so it takes what it needs and hands it to the app there.
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[name = "RushMenuBarNotes"]
    pub struct Notes;

    unsafe impl NSObjectProtocol for Notes {}

    unsafe impl UNUserNotificationCenterDelegate for Notes {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _c: &UNUserNotificationCenter,
            _n: &UNNotification,
            done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _c: &UNUserNotificationCenter,
            r: &UNNotificationResponse,
            done: &block2::DynBlock<dyn Fn()>,
        ) {
            let info = r.notification().request().content().userInfo();
            let get = |k: &str| {
                info.objectForKey(&*ns(k))
                    .and_then(|o| o.downcast::<NSString>().ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            };
            let (key, req) = (get("key"), get("req"));
            let action = r.actionIdentifier().to_string();
            let text = r
                .downcast_ref::<UNTextInputNotificationResponse>()
                .map(|t| t.userText().to_string());
            on_main(move |d| d.respond(&key, &req, &action, text));
            done.call(());
        }
    }
);

impl Notes {
    fn new() -> Retained<Notes> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

/// Runs the app until it quits.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("the menu bar app runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let d = Delegate::new(mtm);
    APP.with(|a| *a.borrow_mut() = Some(d.clone()));
    app.setDelegate(Some(ProtocolObject::from_ref(&*d)));
    app.run();
}

/// Draws everything the app draws, off screen, without showing anything
/// or starting a feed: the icon's three poses, a menu of every kind of
/// row, and each row's view. For tests, which must call it on the main
/// thread; it says what it drew.
#[doc(hidden)]
pub fn check(state: Line) -> Result<Vec<String>, String> {
    use objc2_app_kit::NSView;
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    // Light, so what's kept to look at reads on white.
    // SAFETY: AppKit's own constant.
    let aqua = objc2_app_kit::NSAppearance::appearanceNamed(unsafe {
        objc2_app_kit::NSAppearanceNameAqua
    });
    app.setAppearance(aqua.as_deref());
    let d = Delegate::new(mtm);
    let _ = Notes::new(); // its methods match the protocol's
    // RUSH_MENUBAR_DRAWN names a folder to keep what was drawn in, to look at.
    let keep = std::env::var_os("RUSH_MENUBAR_DRAWN").map(std::path::PathBuf::from);
    let mut out = Vec::new();
    for (i, img) in d.images().iter().enumerate() {
        let tiff = img
            .TIFFRepresentation()
            .ok_or(format!("image {i} didn't draw"))?;
        if let Some(k) = &keep {
            let _ = std::fs::write(k.join(format!("icon{i}.tiff")), tiff.to_vec());
        }
        out.push(format!("image {i}: {} bytes", tiff.len()));
    }
    *d.ivars().state.borrow_mut() = state;
    d.ivars().got.set(true);
    *d.ivars().problem.borrow_mut() = Some("lost touch with rush; trying again…".into());
    let m = NSMenu::new(mtm);
    d.fill(&m);
    for (n, i) in m.itemArray().iter().enumerate() {
        let Some(v) = i.view() else {
            out.push(format!("item {:?}", i.title().to_string()));
            continue;
        };
        let draw = |v: &NSView| {
            let b = v.bounds();
            v.bitmapImageRepForCachingDisplayInRect(b).map(|rep| {
                v.cacheDisplayInRect_toBitmapImageRep(b, &rep);
                let t = rep
                    .TIFFRepresentation()
                    .map(|t| t.to_vec())
                    .unwrap_or_default();
                if let Some(k) = &keep {
                    let _ = std::fs::write(k.join(format!("row{n}.tiff")), &t);
                }
                t.len()
            })
        };
        let n = draw(&v).ok_or("a row didn't draw")?;
        out.push(format!(
            "view {:?}: {n} bytes",
            v.toolTip().map(|t| t.to_string()).unwrap_or_default()
        ));
    }
    // The answer items call back with the ops they were made from.
    let first = m
        .itemArray()
        .iter()
        .find(|i| i.action() == Some(sel!(answerFromMenu:)));
    if let Some(i) = first {
        out.push(format!(
            "answer op {:?}",
            d.ivars().ops.borrow()[i.tag() as usize].op
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn login_plist_opens_the_bundle() {
        let p = super::login_plist("dev.rush.menubar", "/x/rush & co.app");
        assert!(p.contains("<string>/x/rush &amp; co.app</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
    }
}
