// A Mermaid `classDef` is a comma-separated property list, so a colour written
// with commas — a theme's `rgba(38, 79, 120, 0.5)` — splits mid-value and the
// parse fails. `toClassDefColor` turns the browser's *computed* colour (always
// `rgb(...)`/`rgba(...)`, comma or space separated) into a hex value, which
// has no commas and is valid CSS. Pure, so it is unit-tested without a DOM.

const RGB = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/i;

const hex2 = (n: number): string => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, "0");

/** `#rrggbb` (or `#rrggbbaa` when not opaque) for a computed `rgb()`/`rgba()`
 *  colour, or `null` when `computed` is not one. */
export function toClassDefColor(computed: string): string | null {
  const m = RGB.exec(computed.trim());
  if (!m) return null;
  let alpha = 1;
  if (m[4] !== undefined) {
    alpha = m[4].endsWith("%") ? parseFloat(m[4]) / 100 : parseFloat(m[4]);
  }
  const base = `#${hex2(+m[1])}${hex2(+m[2])}${hex2(+m[3])}`;
  return alpha >= 1 ? base : `${base}${hex2(alpha * 255)}`;
}
