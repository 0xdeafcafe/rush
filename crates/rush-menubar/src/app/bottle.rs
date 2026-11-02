//! The menu bar's glyph: the app icon's bottle, leaning, its label a band
//! across it. Working, its cap pops off and it fumes; needing you, it's
//! orange. Port of internal/menubar/app.swift:241-301 (bottle, tinted).

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSAffineTransformNSAppKitAdditions, NSBezierPath, NSColor, NSCompositingOperation,
    NSGraphicsContext, NSImage,
};
use objc2_foundation::{NSAffineTransform, NSPoint, NSRect, NSSize, NSString};

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// The bottle, its cap on or popped.
pub fn bottle(popped: bool) -> Retained<NSImage> {
    let draw = RcBlock::new(move |_r: NSRect| -> Bool {
        let Some(g) = NSGraphicsContext::currentContext() else {
            return Bool::NO;
        };
        let t = NSAffineTransform::new();
        t.translateXBy_yBy(9.0, 9.0);
        t.rotateByDegrees(22.0);
        t.translateXBy_yBy(-8.0, -8.5);
        t.concat();
        NSColor::blackColor().set();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect(4.0, 0.5, 8.0, 9.5), 2.0, 2.0)
            .fill();
        NSBezierPath::bezierPathWithRect(rect(5.5, 9.5, 5.0, 1.5)).fill();
        // The cap: on, or lifted off and tipped, with fumes under it.
        let cap = NSAffineTransform::new();
        if popped {
            cap.translateXBy_yBy(10.4, 12.5);
            cap.rotateByDegrees(-40.0);
            cap.translateXBy_yBy(-8.0, -11.5);
            NSBezierPath::bezierPathWithOvalInRect(rect(5.4, 11.9, 1.4, 1.4)).fill();
            NSBezierPath::bezierPathWithOvalInRect(rect(3.9, 13.6, 1.1, 1.1)).fill();
        }
        g.saveGraphicsState();
        cap.concat();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect(5.0, 11.5, 6.0, 3.5),
            1.0,
            1.0,
        )
        .fill();
        g.setCompositingOperation(NSCompositingOperation::Clear);
        for x in [7.1, 8.9] {
            // its ribs
            NSBezierPath::bezierPathWithRect(rect(x - 0.4, 12.2, 0.8, 2.1)).fill();
        }
        g.restoreGraphicsState();
        // The label's edges.
        g.setCompositingOperation(NSCompositingOperation::Clear);
        NSBezierPath::bezierPathWithRect(rect(3.0, 2.6, 10.0, 0.7)).fill();
        NSBezierPath::bezierPathWithRect(rect(3.0, 7.2, 10.0, 0.7)).fill();
        Bool::YES
    });
    let i = NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18.0, 18.0), false, &draw);
    i.setTemplate(true);
    i.setAccessibilityDescription(Some(&NSString::from_str("rush")));
    i
}

/// i in colour c, where it isn't clear.
pub fn tinted(i: Retained<NSImage>, c: Retained<NSColor>) -> Retained<NSImage> {
    let size = i.size();
    let desc = i.accessibilityDescription();
    let draw = RcBlock::new(move |r: NSRect| -> Bool {
        i.drawInRect(r);
        c.set();
        if let Some(g) = NSGraphicsContext::currentContext() {
            g.setCompositingOperation(NSCompositingOperation::SourceAtop);
        }
        NSBezierPath::fillRect(r);
        Bool::YES
    });
    let t = NSImage::imageWithSize_flipped_drawingHandler(size, false, &draw);
    t.setAccessibilityDescription(desc.as_deref());
    t
}
