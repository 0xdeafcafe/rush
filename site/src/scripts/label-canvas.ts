// A deliberately small painter for Label.tsx, not a general HTML screenshotter.
// Layout, wrapping, colours, paths and font metrics all come from the live DOM.
// No SVG images/foreignObject: every texture pixel has an origin-clean 2D source.
type Paint = (context: CanvasRenderingContext2D) => void;

export async function measureLabel(
  face: HTMLElement,
  width: number,
  height: number,
) {
  await document.fonts.ready;
  const host = document.createElement("div");
  host.className = "label-frame";
  host.setAttribute("aria-hidden", "true");
  host.inert = true;
  host.style.cssText = `position:fixed;left:0;top:0;width:${width}px;height:${height}px;visibility:hidden;pointer-events:none;overflow:hidden;contain:layout style;`;
  const copy = face.cloneNode(true) as HTMLElement;
  host.append(copy);
  document.body.append(host);
  try {
    // Measure before transforms, then apply those same matrices while painting.
    const transforms = new Map<Element, DOMMatrix>();
    for (const element of [
      copy,
      ...copy.querySelectorAll<HTMLElement>("*:not(svg):not(svg *)"),
    ]) {
      const transform = getComputedStyle(element).transform;
      if (transform !== "none") {
        transforms.set(element, new DOMMatrix(transform));
        element.style.transform = "none";
      }
    }
    const commands: Paint[] = [];
    const baselines = new Map<string, number>();
    const fontFor = (style: CSSStyleDeclaration) =>
      `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
    function baseline(style: CSSStyleDeclaration) {
      const font = fontFor(style);
      const key = font + "/" + style.lineHeight;
      if (baselines.has(key)) return baselines.get(key)!;
      // Canvas and DOM expose different ascent metrics on WebKit. A zero-height
      // inline block sits on the actual DOM baseline, including fallback fonts.
      const probe = document.createElement("span");
      probe.style.cssText = `position:absolute;left:0;top:0;font:${font};line-height:${style.lineHeight};white-space:nowrap;`;
      const text = document.createTextNode("Hg");
      const marker = document.createElement("span");
      marker.style.cssText =
        "display:inline-block;width:0;height:0;vertical-align:baseline;";
      probe.append(text, marker);
      host.append(probe);
      const range = document.createRange();
      range.selectNodeContents(text);
      const offset =
        marker.getBoundingClientRect().top - range.getBoundingClientRect().top;
      probe.remove();
      baselines.set(key, offset);
      return offset;
    }
    function text(node: Text, style: CSSStyleDeclaration) {
      const range = document.createRange();
      const runs: { value: string; x: number; top: number; right: number }[] =
        [];
      const glyphs: { value: string; x: number; top: number }[] = [];
      // Range owns the line breaking, whitespace collapse and inline positioning.
      // Group glyphs back into whole line runs so Canvas preserves font shaping.
      for (let offset = 0; offset < node.length;) {
        const value = String.fromCodePoint(node.data.codePointAt(offset)!);
        range.setStart(node, offset);
        offset += value.length;
        range.setEnd(node, offset);
        const rect = range.getBoundingClientRect();
        if (rect.width < 0.01 || rect.height === 0) continue;
        const glyph = /\s/u.test(value) ? " " : value;
        glyphs.push({ value: glyph, x: rect.left, top: rect.top });
        const previous = runs.at(-1);
        if (
          previous &&
          Math.abs(previous.top - rect.top) < 0.5 &&
          Math.abs(previous.right - rect.left) < 1
        ) {
          previous.value += glyph;
          previous.right = rect.right;
        } else
          runs.push({
            value: glyph,
            x: rect.left,
            top: rect.top,
            right: rect.right,
          });
      }
      const font = fontFor(style),
        offset = baseline(style);
      const colour = style.color,
        spacing = style.letterSpacing;
      const uppercase = style.textTransform === "uppercase";
      const underline = style.textDecorationLine.includes("underline");
      const underlineOffset =
        parseFloat(style.textUnderlineOffset) ||
        parseFloat(style.fontSize) * 0.2;
      const thickness = parseFloat(style.textDecorationThickness) || 1;
      commands.push((ctx) => {
        ctx.font = font;
        ctx.textBaseline = "alphabetic";
        ctx.textAlign = "left";
        ctx.fillStyle = colour;
        // Older iOS lacks Canvas letterSpacing; the measured glyph positions
        // still preserve HTML tracking instead of silently widening the type.
        const tracksText = "letterSpacing" in ctx;
        if (tracksText)
          ctx.letterSpacing = spacing === "normal" ? "0px" : spacing;
        for (const run of tracksText ? runs : glyphs) {
          ctx.fillText(
            uppercase ? run.value.toUpperCase() : run.value,
            run.x,
            run.top + offset,
          );
        }
        if (underline)
          for (const run of runs) {
            ctx.fillRect(
              run.x,
              run.top + offset + underlineOffset,
              run.right - run.x,
              thickness,
            );
          }
      });
    }
    function svg(element: SVGSVGElement) {
      const box = element.getBoundingClientRect(),
        view = element.viewBox.baseVal;
      const scale = Math.min(box.width / view.width, box.height / view.height);
      commands.push((ctx) => {
        ctx.save();
        ctx.translate(
          box.x + (box.width - view.width * scale) / 2,
          box.y + (box.height - view.height * scale) / 2,
        );
        ctx.scale(scale, scale);
        ctx.translate(-view.x, -view.y);
      });
      function shape(node: SVGGraphicsElement) {
        const matrix = node.transform.baseVal.consolidate()?.matrix;
        commands.push((ctx) => {
          ctx.save();
          if (matrix)
            ctx.transform(
              matrix.a,
              matrix.b,
              matrix.c,
              matrix.d,
              matrix.e,
              matrix.f,
            );
        });
        if (node instanceof SVGPathElement) {
          const path = new Path2D(node.getAttribute("d") ?? ""),
            style = getComputedStyle(node);
          const fill = style.fill,
            stroke = style.stroke,
            lineWidth = parseFloat(style.strokeWidth);
          const rule = style.fillRule as CanvasFillRule;
          commands.push((ctx) => {
            if (fill !== "none") {
              ctx.fillStyle = fill;
              ctx.fill(path, rule);
            }
            if (stroke !== "none") {
              ctx.strokeStyle = stroke;
              ctx.lineWidth = lineWidth;
              ctx.stroke(path);
            }
          });
        }
        for (const child of node.children) shape(child as SVGGraphicsElement);
        commands.push((ctx) => ctx.restore());
      }
      for (const child of element.children) shape(child as SVGGraphicsElement);
      commands.push((ctx) => ctx.restore());
    }
    function element(node: Element) {
      if (node instanceof SVGSVGElement) {
        svg(node);
        return;
      }
      const style = getComputedStyle(node),
        rect = node.getBoundingClientRect();
      if (style.display === "none") return;
      const matrix = transforms.get(node);
      const [ox, oy] = style.transformOrigin.split(" ").map(parseFloat);
      const background = style.backgroundColor;
      const borders = ["Top", "Right", "Bottom", "Left"].map((side) => ({
        size: parseFloat(
          style.getPropertyValue(`border-${side.toLowerCase()}-width`),
        ),
        colour: style.getPropertyValue(`border-${side.toLowerCase()}-color`),
      }));
      const clip = style.overflowX === "hidden" || style.overflowY === "hidden";
      commands.push((ctx) => {
        ctx.save();
        if (matrix) {
          ctx.translate(rect.x + ox, rect.y + oy);
          ctx.transform(
            matrix.a,
            matrix.b,
            matrix.c,
            matrix.d,
            matrix.e,
            matrix.f,
          );
          ctx.translate(-rect.x - ox, -rect.y - oy);
        }
        ctx.fillStyle = background;
        ctx.fillRect(rect.x, rect.y, rect.width, rect.height);
        const [top, right, bottom, left] = borders;
        ctx.fillStyle = top.colour;
        ctx.fillRect(rect.x, rect.y, rect.width, top.size);
        ctx.fillStyle = right.colour;
        ctx.fillRect(rect.right - right.size, rect.y, right.size, rect.height);
        ctx.fillStyle = bottom.colour;
        ctx.fillRect(
          rect.x,
          rect.bottom - bottom.size,
          rect.width,
          bottom.size,
        );
        ctx.fillStyle = left.colour;
        ctx.fillRect(rect.x, rect.y, left.size, rect.height);
        if (clip) {
          ctx.beginPath();
          ctx.rect(rect.x, rect.y, rect.width, rect.height);
          ctx.clip();
        }
      });
      for (const child of node.childNodes) {
        if (child instanceof Text) text(child, style);
        else if (child instanceof Element) element(child);
      }
      commands.push((ctx) => ctx.restore());
    }
    element(copy);
    const ground = getComputedStyle(document.body).backgroundColor;
    return (scale: number) => {
      const canvas = document.createElement("canvas");
      canvas.width = Math.round(width * scale);
      canvas.height = Math.round(height * scale);
      const context = canvas.getContext("2d")!;
      context.scale(canvas.width / width, canvas.height / height);
      context.fillStyle = ground;
      context.fillRect(0, 0, width, height);
      for (const paint of commands) paint(context);
      return canvas;
    };
  } finally {
    host.remove();
  }
}
