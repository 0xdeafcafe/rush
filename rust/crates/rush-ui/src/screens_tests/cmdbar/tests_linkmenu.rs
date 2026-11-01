//! rush-ui-cmdbar's linkmenu.rs tests: they make a whole UI model.

use super::linkmenu::*;
use crate::frame::{Press, Sheet};
use crate::model::Model;
use rush_core::text::Line;

fn menu(m: &mut Model) -> &mut LinkMenu {
    m.sheet_as::<LinkMenu>().expect("an image's menu")
}

// linkmenu.go drawShot and picker.go "v": an image's menu draws it
// above its acts; v draws it big, the window's width, and back.
#[test]
fn an_image_link_shows_its_picture() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("shot.png");
    let mut img = image::RgbaImage::new(400, 200);
    for (x, _, px) in img.enumerate_pixels_mut() {
        *px = image::Rgba([if x < 200 { 200 } else { 10 }, 40, 90, 255]);
    }
    img.save(&p).unwrap();
    let mut m = crate::new_model("t");
    (m.w, m.h) = (120, 50);
    let u = format!("file://{}", p.display());
    assert!(open_link_menu(&mut m, &u));
    let other = crate::new_model("t");
    let s = menu(&mut m);
    let shot = s.shot.clone().expect("drawn at once with no loop");
    assert_eq!((shot.len(), shot[0].width()), (12, 48));
    let body: Vec<String> = s.body(&other, 100, 40).iter().map(Line::plain).collect();
    assert!(
        body[2].trim().chars().all(|c| "▀▄ ".contains(c)),
        "{body:?}"
    );
    assert_eq!(
        body.last().unwrap().trim(),
        "↑↓ choose  ·  enter do it  ·  v bigger  ·  esc close"
    );
    m.sheet_key(&Press::of("v").unwrap());
    let s = menu(&mut m);
    assert!(s.big);
    let big = s.shot.clone().expect("drawn big");
    assert_eq!(big[0].width(), 110);
    m.sheet_key(&Press::of("v").unwrap());
    assert!(!menu(&mut m).big);
    assert_eq!(menu(&mut m).shot.as_ref().unwrap()[0].width(), 48);
    // A web address's menu has no picture.
    assert!(open_link_menu(&mut m, "https://example.com/a.png"));
    assert!(m.sheet_as::<LinkMenu>().is_none());
}
