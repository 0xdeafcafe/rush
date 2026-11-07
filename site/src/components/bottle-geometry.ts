// The drawn bottle's geometry, shared by the build-time markup (Bottle.tsx)
// and the scroll script (scripts/bottle.ts). Ported from the bottle concept
// (site/concepts/bottle/index.html): the label is one flat drawing, cut into
// N vertical strips that are placed round a cylinder of radius R.

export const LABEL_W = 754;
export const LABEL_H = 176;
export const FACES = 72;
export const RADIUS = 120;
export const TILT = -12;

const FACE_W = LABEL_W / FACES;

/** Where strip i sits along the label, centred on the front. */
export function faceArc(i: number): number {
  return (i + 0.5) * FACE_W - LABEL_W / 2;
}

/** The strip's width in px, one over so neighbours never show a seam. */
export const FACE_PX = FACE_W + 1;

/** The CSS transform for strip i with the bottle turned phi radians, or
 *  null when the strip is round the back. */
export function faceTransform(i: number, phi: number): string | null {
  const k = 1 / RADIUS;
  let s = faceArc(i) + phi * RADIUS;
  s = (((s + LABEL_W / 2) % LABEL_W) + LABEL_W) % LABEL_W - LABEL_W / 2;
  const th = k * s;
  const c = Math.cos(th);
  if (c <= 0.002) return null;
  const x = Math.sin(th) / k;
  return `translateX(${x.toFixed(2)}px) scaleX(${c.toFixed(4)})`;
}

/** The cap's ridges: x, stroke width and opacity, or null round the back. */
export const RIDGES = 18;
export function ridge(j: number, phi: number): { x: number; w: number; o: number } | null {
  const a = (j / RIDGES) * 2 * Math.PI + phi;
  const c = Math.cos(a);
  if (c <= 0) return null;
  return { x: 160 + 72 * Math.sin(a), w: 1 + 5 * c, o: 0.25 + 0.45 * c };
}

/** The label as an SVG string: directions, the front, the contents. A
 *  barcode made from 0xdeafcafe, the same every build. */
export function labelSvg(yellow: string, red: string): string {
  let bars = "";
  let bx = 0;
  let seed = 0xdeafcafe >>> 0;
  for (let b = 0; b < 34; b++) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    const bw = 1 + ((seed >>> 29) % 3);
    bars += `<rect x="${bx}" width="${bw}" height="28" fill="#111"/>`;
    bx += bw + 1 + ((seed >>> 26) % 2);
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${LABEL_W} ${LABEL_H}" width="${LABEL_W}" height="${LABEL_H}">
<rect width="${LABEL_W}" height="${LABEL_H}" fill="${yellow}"/>
<rect width="${LABEL_W}" height="9" fill="${red}"/><rect y="167" width="${LABEL_W}" height="9" fill="${red}"/>
<g font-family="Helvetica Neue, Helvetica, Arial, sans-serif" fill="#111">
<text x="22" y="34" font-size="11" font-weight="800" letter-spacing="1">DIRECTIONS</text>
<text x="22" y="52" font-size="10.5" font-family="Menlo, monospace">brew install 0xdeafcafe/tap/rush</text>
<text font-size="10.5"><tspan x="22" y="70">use the full name. plain brew install rush</tspan><tspan x="22" y="83">gets gnu's restricted shell.</tspan><tspan x="22" y="101">macs with apple silicon only.</tspan></text>
<text x="22" y="122" font-size="11" font-weight="800" letter-spacing="1">KEEP AWAY FROM PROD</text>
<g transform="translate(22 132)">${bars}</g>
<text x="150" y="157" font-size="9" font-family="Menlo, monospace">0xDEAFCAFE</text>
<g transform="translate(377 0)" text-anchor="middle">
<text y="96" font-family="Helvetica Neue, Helvetica, Arial, sans-serif" font-stretch="condensed" font-weight="900" font-style="italic" font-size="80" fill="${red}" stroke="#111" stroke-width="5" paint-order="stroke" letter-spacing="-1">RUSH</text>
<text y="124" font-size="17.5" font-weight="800" letter-spacing=".5">AI HARNESS POLISHER</text>
<rect x="-86" y="132" width="172" height="2.5"/>
<text y="151" font-size="10.5" font-weight="700" letter-spacing=".8">NOT FOR AGENT CONSUMPTION</text>
</g>
<text x="525" y="34" font-size="11" font-weight="800" letter-spacing="1">CONTAINS</text>
<text font-size="10.5"><tspan x="525" y="52">one terminal app for every coding agent</tspan><tspan x="525" y="65">you've got: claude code, codex, copilot,</tspan><tspan x="525" y="78">kimi, antigravity, opencode, vibe,</tspan><tspan x="525" y="91">deepseek, glm and ollama.</tspan><tspan x="525" y="109">about 40 MB of ram with a session open.</tspan><tspan x="525" y="127">written in rust. wtfpl.</tspan></text>
<text x="525" y="153" font-size="11" font-weight="800" letter-spacing="1">40 MB</text>
</g></svg>`;
}
