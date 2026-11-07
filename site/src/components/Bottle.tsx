// The rush bottle, drawn: amber glass, a black cap and the yellow label,
// which is cut into strips and wrapped round the glass. Rendered here at
// build time facing front; scripts/bottle.ts turns it as the page scrolls.
// Geometry ported from site/concepts/bottle/index.html.
import { FACES, FACE_PX, LABEL_H, LABEL_W, RIDGES, TILT, faceTransform, labelSvg, ridge } from "./bottle-geometry";

const art = `url("data:image/svg+xml;charset=utf-8,${encodeURIComponent(labelSvg("#ffe600", "#e8231f"))}")`;

export default function Bottle() {
  return (
    <div className="bottle" data-bottle role="img" aria-label="The rush bottle: amber glass, a black cap and a yellow label reading RUSH, AI harness polisher, not for agent consumption">
      <div className="bottle-stage" style={{ transform: `scale(var(--bottle-scale)) rotate(${TILT}deg)` }}>
        <svg className="bottle-glass" viewBox="0 0 320 560" aria-hidden="true">
          <defs>
            <linearGradient id="amber" x1="0" x2="1">
              <stop offset="0" stopColor="#2a1204" />
              <stop offset=".25" stopColor="#7a3a0e" />
              <stop offset=".5" stopColor="#9a4c14" />
              <stop offset=".8" stopColor="#5a2808" />
              <stop offset="1" stopColor="#1e0c02" />
            </linearGradient>
            <linearGradient id="capg" x1="0" x2="1">
              <stop offset="0" stopColor="#050505" />
              <stop offset=".35" stopColor="#3a3a3a" />
              <stop offset=".55" stopColor="#1a1a1a" />
              <stop offset="1" stopColor="#000" />
            </linearGradient>
          </defs>
          <rect x="100" y="112" width="120" height="44" fill="url(#amber)" />
          <path d="M40 210 Q40 160 100 152 L220 152 Q280 160 280 210 L280 494 Q280 520 254 520 L66 520 Q40 520 40 494 Z" fill="url(#amber)" />
          <rect x="85" y="18" width="150" height="102" rx="12" fill="url(#capg)" />
          <g stroke="#000" strokeLinecap="round" data-ridges>
            {Array.from({ length: RIDGES }, (_, j) => {
              const r = ridge(j, 0);
              return <line key={j} y1="30" y2="112" x1={r ? r.x.toFixed(2) : 0} x2={r ? r.x.toFixed(2) : 0} strokeWidth={r ? r.w.toFixed(2) : 0} strokeOpacity={r ? r.o.toFixed(2) : 0} />;
            })}
          </g>
          <rect x="85" y="18" width="150" height="8" rx="4" fill="#555" />
        </svg>
        <div className="bottle-faces" style={{ ["--art" as string]: art, ["--lw" as string]: `${LABEL_W}px`, ["--lh" as string]: `${LABEL_H}px` }}>
          {Array.from({ length: FACES }, (_, i) => {
            const t = faceTransform(i, 0);
            return (
              <div
                key={i}
                className="bottle-face"
                style={{
                  width: `${FACE_PX}px`,
                  left: `${-FACE_PX / 2}px`,
                  backgroundPosition: `${-(i * (LABEL_W / FACES) - 0.5)}px 0`,
                  transform: t ?? undefined,
                  visibility: t ? "visible" : "hidden",
                }}
              />
            );
          })}
        </div>
        <div className="bottle-shade" />
        <svg className="bottle-glass" viewBox="0 0 320 560" aria-hidden="true">
          <defs>
            <linearGradient id="shine" x1="0" x2="1">
              <stop offset="0" stopColor="#fff" stopOpacity="0" />
              <stop offset=".5" stopColor="#fff" stopOpacity=".5" />
              <stop offset="1" stopColor="#fff" stopOpacity="0" />
            </linearGradient>
          </defs>
          <rect x="68" y="176" width="22" height="330" rx="11" fill="url(#shine)" opacity=".7" />
          <rect x="248" y="196" width="8" height="300" rx="4" fill="url(#shine)" opacity=".35" />
          <rect x="104" y="30" width="10" height="82" rx="5" fill="url(#shine)" opacity=".25" />
        </svg>
      </div>
    </div>
  );
}
