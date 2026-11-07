const root = document.documentElement;
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
    chirps.forEach((c, i) =>
      c.classList.toggle("landed", top < 0.85 - 0.125 * i),
    );
  };
}

// With no JS or reduced motion, the list and all chirps show their final state.
const preference = matchMedia("(prefers-reduced-motion: reduce)");
const parts = [list(), feed()];
let queued = false;
const frame = () => {
  queued = false;
  if (!preference.matches) parts.forEach((part) => part());
};
const request = () => {
  if (!queued && !preference.matches) {
    queued = true;
    requestAnimationFrame(frame);
  }
};
const updatePreference = () => {
  root.classList.toggle("motion", !preference.matches);
  frame();
};
preference.addEventListener("change", updatePreference);
addEventListener("scroll", request, { passive: true });
addEventListener("resize", request, { passive: true });
updatePreference();
