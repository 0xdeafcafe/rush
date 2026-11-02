//! The menu's rows, drawn by hand: a dot, a name, what it's doing in grey
//! and a time at the right; and an account's name with its two limits as
//! bars. Port of internal/menubar/app.swift:506-706 (MenuView, RowView,
//! UsageView).

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSFont, NSFontAttributeName, NSFontWeightMedium,
    NSFontWeightRegular, NSForegroundColorAttributeName, NSLineBreakMode, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSStringDrawing, NSTextAlignment, NSTrackingArea,
    NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

use crate::menu::{Level, level, percent};
use crate::wire::{Account, Window};

pub const ROW_WIDTH: f64 = 340.0;
const INSET: f64 = 14.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// The colour for how much of a window is used.
fn level_color(pct: f64) -> Retained<NSColor> {
    match level(pct) {
        Level::Red => NSColor::systemRedColor(),
        Level::Orange => NSColor::systemOrangeColor(),
        Level::Green => NSColor::systemGreenColor(),
    }
}

/// Draws s in r, cut short with "…" when it doesn't fit, centred down it.
fn text(s: &str, font: &NSFont, color: &NSColor, r: NSRect, right: bool) {
    let p = NSMutableParagraphStyle::new();
    p.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    p.setAlignment(if right {
        NSTextAlignment::Right
    } else {
        NSTextAlignment::Left
    });
    // SAFETY: the keys are AppKit's own, with values of their types.
    let attrs = unsafe {
        NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
            ],
            &[font as &AnyObject, color as &AnyObject, &*p as &AnyObject],
        )
    };
    let h = font.ascender() - font.descender();
    let r = rect(r.origin.x, mid_y(r) - h / 2.0, r.size.width, h + 1.0);
    // SAFETY: attributes as AppKit takes them.
    unsafe { NSString::from_str(s).drawInRect_withAttributes(r, Some(&attrs)) };
}

/// How wide s is in font, rounded up.
fn width(s: &str, font: &NSFont) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    // SAFETY: the key is AppKit's own, with a font for its value.
    let attrs = unsafe {
        NSDictionary::<NSString, AnyObject>::from_slices(
            &[NSFontAttributeName],
            &[font as &AnyObject],
        )
    };
    // SAFETY: attributes as AppKit takes them.
    unsafe { NSString::from_str(s).sizeWithAttributes(Some(&attrs)) }
        .width
        .ceil()
}

fn mid_y(r: NSRect) -> f64 {
    r.origin.y + r.size.height / 2.0
}

pub struct RowIvars {
    title: String,
    detail: String,
    trailing: String,
    dot: Option<Retained<NSColor>>,
    /// The agent a click goes to; None when it can't be clicked.
    action: Option<String>,
}

define_class!(
    /// One line: a dot, a name, what it's doing in grey, and a time at the
    /// right; highlighted as the system's own rows are when it can be
    /// clicked.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RushMenuRowView"]
    #[ivars = RowIvars]
    pub struct RowView;

    impl RowView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _r: NSRect) {
            self.draw();
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            // SAFETY: the superclass's own.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            for a in self.trackingAreas().iter() {
                self.removeTrackingArea(&a);
            }
            // SAFETY: owner is this view, which outlives its tracking areas.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    NSTrackingAreaOptions::MouseEnteredAndExited
                        | NSTrackingAreaOptions::ActiveAlways,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _e: &NSEvent) {
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _e: &NSEvent) {
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _e: &NSEvent) {
            let Some(key) = self.ivars().action.clone() else {
                return;
            };
            if let Some(m) = self.enclosingMenuItem().and_then(|i| unsafe { i.menu() }) {
                m.cancelTracking();
            }
            super::with_app(|a| a.show(&key));
        }
    }
);

impl RowView {
    pub fn new(
        mtm: MainThreadMarker,
        title: &str,
        detail: &str,
        trailing: &str,
        dot: Option<Retained<NSColor>>,
        action: Option<String>,
    ) -> Retained<RowView> {
        let this = Self::alloc(mtm).set_ivars(RowIvars {
            title: title.into(),
            detail: detail.into(),
            trailing: trailing.into(),
            dot,
            action,
        });
        // SAFETY: NSView's designated initialiser.
        let v: Retained<RowView> =
            unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, ROW_WIDTH, 22.0)] };
        let tip: Vec<&str> = [title, detail]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        v.setToolTip(Some(&NSString::from_str(&tip.join("\n"))));
        v
    }

    fn highlighted(&self) -> bool {
        self.ivars().action.is_some() && self.enclosingMenuItem().is_some_and(|i| i.isHighlighted())
    }

    fn draw(&self) {
        let iv = self.ivars();
        let b = self.bounds();
        let hi = self.highlighted();
        if hi {
            NSColor::selectedContentBackgroundColor().setFill();
            let r = rect(
                b.origin.x + 5.0,
                b.origin.y,
                b.size.width - 10.0,
                b.size.height,
            );
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, 4.0, 4.0).fill();
        }
        let primary = if hi {
            NSColor::selectedMenuItemTextColor()
        } else {
            NSColor::labelColor()
        };
        let secondary = if hi {
            NSColor::selectedMenuItemTextColor().colorWithAlphaComponent(0.8)
        } else {
            NSColor::secondaryLabelColor()
        };
        let font = NSFont::menuFontOfSize(13.0);
        let small = NSFont::systemFontOfSize(12.0);
        // SAFETY: AppKit's own constant.
        let digits =
            NSFont::monospacedDigitSystemFontOfSize_weight(11.0, unsafe { NSFontWeightRegular });
        let mut x = INSET;
        if let Some(dot) = &iv.dot {
            if hi {
                NSColor::selectedMenuItemTextColor().setFill();
            } else {
                dot.setFill();
            }
            NSBezierPath::bezierPathWithOvalInRect(rect(x, mid_y(b) - 3.5, 7.0, 7.0)).fill();
            x += 15.0;
        }
        let mut right = b.origin.x + b.size.width - INSET;
        if !iv.trailing.is_empty() {
            let w = width(&iv.trailing, &digits);
            let c = if hi {
                secondary.clone()
            } else {
                NSColor::tertiaryLabelColor()
            };
            text(
                &iv.trailing,
                &digits,
                &c,
                rect(right - w, 0.0, w, b.size.height),
                false,
            );
            right -= w + 10.0;
        }
        if iv.title.is_empty() {
            x = x.max(INSET + 15.0); // under the names, not the dots
            text(
                &iv.detail,
                &small,
                &secondary,
                rect(x, 0.0, right - x, b.size.height),
                false,
            );
            return;
        }
        let room = right - x;
        let tw = if iv.detail.is_empty() {
            room
        } else {
            width(&iv.title, &font).min(room * 0.5)
        };
        text(
            &iv.title,
            &font,
            &primary,
            rect(x, 0.0, tw, b.size.height),
            false,
        );
        if !iv.detail.is_empty() {
            let dx = x + tw + 8.0;
            text(
                &iv.detail,
                &small,
                &secondary,
                rect(dx, 0.0, (right - dx).max(0.0), b.size.height),
                false,
            );
        }
    }
}

pub struct UsageIvars {
    a: Account,
}

define_class!(
    /// An account's name and its two windows as bars, inline.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RushMenuUsageView"]
    #[ivars = UsageIvars]
    pub struct UsageView;

    impl UsageView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _r: NSRect) {
            self.draw();
        }
    }
);

impl UsageView {
    pub fn new(mtm: MainThreadMarker, a: Account) -> Retained<UsageView> {
        let this = Self::alloc(mtm).set_ivars(UsageIvars { a });
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, ROW_WIDTH, 24.0)] }
    }

    fn draw(&self) {
        let a = &self.ivars().a;
        let b = self.bounds();
        let font = NSFont::menuFontOfSize(13.0);
        // SAFETY: AppKit's own constants.
        let label = NSFont::systemFontOfSize_weight(11.0, unsafe { NSFontWeightMedium });
        let digits =
            NSFont::monospacedDigitSystemFontOfSize_weight(11.0, unsafe { NSFontWeightRegular });
        let name_w = 96.0;
        let c = if a.current {
            NSColor::labelColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        text(
            &a.name,
            &font,
            &c,
            rect(INSET, 0.0, name_w - 6.0, b.size.height),
            false,
        );
        if !a.problem.is_empty() && !a.five_hour.present && !a.seven_day.present {
            text(
                &a.problem,
                &NSFont::systemFontOfSize(11.0),
                &NSColor::tertiaryLabelColor(),
                rect(
                    INSET + name_w,
                    0.0,
                    b.size.width - INSET * 2.0 - name_w,
                    b.size.height,
                ),
                false,
            );
            return;
        }
        let gap = 14.0;
        let each = (b.origin.x + b.size.width - INSET - (INSET + name_w) - gap) / 2.0;
        let or = |s: &str, d: &'static str| {
            if s.is_empty() {
                d.to_string()
            } else {
                s.to_string()
            }
        };
        self.window(
            &or(&a.five_hour.label, "5h"),
            &a.five_hour,
            INSET + name_w,
            each,
            &label,
            &digits,
        );
        self.window(
            &or(&a.seven_day.label, "wk"),
            &a.seven_day,
            INSET + name_w + each + gap,
            each,
            &label,
            &digits,
        );
    }

    fn window(&self, name: &str, win: &Window, x: f64, w: f64, label: &NSFont, digits: &NSFont) {
        let b = self.bounds();
        let (lw, pw) = (20.0, 32.0);
        text(
            name,
            label,
            &NSColor::tertiaryLabelColor(),
            rect(x, 0.0, lw, b.size.height),
            false,
        );
        let c = if win.present {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::tertiaryLabelColor()
        };
        text(
            &percent(win),
            digits,
            &c,
            rect(x + w - pw, 0.0, pw, b.size.height),
            true,
        );
        let track = rect(x + lw, mid_y(b) - 2.5, w - lw - pw - 4.0, 5.0);
        NSColor::quaternaryLabelColor().setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(track, 2.5, 2.5).fill();
        if !win.present || win.percent <= 0.0 {
            return;
        }
        let mut fill = track;
        fill.size.width = track
            .size
            .height
            .max(track.size.width * (win.percent / 100.0).min(1.0));
        level_color(win.percent).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(fill, 2.5, 2.5).fill();
    }
}
