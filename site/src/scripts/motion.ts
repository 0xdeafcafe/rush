// The page's three bits of motion, all tied to where the page is scrolled
// and all reversible: scroll back and they go back. Nothing takes the
// scroll over. With prefers-reduced-motion (no .motion on <html>) none of
// this runs and the page shows every final state.
import { FACES, RIDGES, TILT, faceTransform, ridge } from "../components/bottle-geometry";

const root = document.documentElement;
const clamp = (v: number, a: number, b: number) => Math.min(b, Math.max(a, v));

function bottle(): () => void {
  const cover = document.querySelector<HTMLElement>(".cover");
  const box = document.querySelector<HTMLElement>("[data-bottle]");
  if (!cover || !box) return () => {};
  const faces = [...box.querySelectorAll<HTMLElement>(".bottle-face")];
  const ridges = [...box.querySelectorAll<SVGLineElement>("[data-ridges] line")];
  const stage = box.querySelector<HTMLElement>(".bottle-stage");
  if (faces.length !== FACES || ridges.length !== RIDGES || !stage) return () => {};
  let last = -1;
  return () => {
    // One full turn while the cover scrolls away.
    const p = clamp(window.scrollY / Math.max(1, cover.offsetHeight), 0, 1);
    if (p === last) return;
    last = p;
    const phi = p * 2 * Math.PI;
    faces.forEach((f, i) => {
      const t = faceTransform(i, phi);
      f.style.visibility = t ? "visible" : "hidden";
      if (t) f.style.transform = t;
    });
    ridges.forEach((l, j) => {
      const r = ridge(j, phi);
      l.setAttribute("x1", r ? r.x.toFixed(2) : "0");
      l.setAttribute("x2", r ? r.x.toFixed(2) : "0");
      l.setAttribute("stroke-width", r ? r.w.toFixed(2) : "0");
      l.setAttribute("stroke-opacity", r ? r.o.toFixed(2) : "0");
    });
    // A little sway as it turns, back to the resting tilt at the end.
    const tilt = TILT + 6 * Math.sin(phi);
    stage.style.transform = `scale(var(--bottle-scale)) rotate(${tilt.toFixed(2)}deg)`;
  };
}

function list(): () => void {
  const el = document.querySelector<HTMLElement>("[data-list]");
  if (!el) return () => {};
  return () => {
    // The question arrives once the list is well up the screen, and
    // un-arrives only once it's back near the bottom, so it never flickers.
    const top = el.getBoundingClientRect().top / window.innerHeight;
    if (top < 0.55) el.classList.add("arrived");
    else if (top > 0.85) el.classList.remove("arrived");
  };
}

function feed(): () => void {
  const el = document.querySelector<HTMLElement>("[data-feed]");
  if (!el) return () => {};
  const chirps = [...el.querySelectorAll<HTMLElement>("[data-chirp]")];
  return () => {
    // One chirp lands for every eighth of the screen the dock rises.
    const top = el.getBoundingClientRect().top / window.innerHeight;
    chirps.forEach((c, i) => c.classList.toggle("landed", top < 0.85 - 0.125 * i));
  };
}

if (root.classList.contains("motion")) {
  const parts = [bottle(), list(), feed()];
  let queued = false;
  const frame = () => {
    queued = false;
    parts.forEach((p) => p());
  };
  const request = () => {
    if (!queued) {
      queued = true;
      requestAnimationFrame(frame);
    }
  };
  addEventListener("scroll", request, { passive: true });
  addEventListener("resize", request, { passive: true });
  frame();
}
