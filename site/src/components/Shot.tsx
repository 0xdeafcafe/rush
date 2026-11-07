// A real rush screenshot, cropped to one rectangle of the original (x, y, w,
// h in the original's pixels) and never scaled past its native size. The
// image itself comes from astro:assets (getImage in the page), so it's
// resized and served as webp with a srcset.
import type { ReactNode } from "react";

export type Img = { src: string; srcSet: string; width: number; height: number };
type Crop = { x: number; y: number; w: number; h: number };

export default function Shot(props: {
  img: Img;
  alt: string;
  crop: Crop;
  narrow?: Partial<Crop>;
  sizes: string;
  className?: string;
  children: ReactNode;
}) {
  const { img, crop, narrow } = props;
  const vars: Record<string, string | number> = {
    "--n": img.width,
    "--x": crop.x,
    "--y": crop.y,
    "--w": crop.w,
    "--h": crop.h,
  };
  if (narrow) for (const [k, v] of Object.entries(narrow)) vars[`--n${k}`] = v;
  return (
    <figure className={props.className}>
      <div className={"crop" + (narrow ? " crop-narrow" : "")} style={vars}>
        <img src={img.src} srcSet={img.srcSet} sizes={props.sizes} width={img.width} height={img.height} alt={props.alt} loading="lazy" decoding="async" />
      </div>
      <figcaption className="small">{props.children}</figcaption>
    </figure>
  );
}
