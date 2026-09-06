// Chart.js in an mcp-v8 isolate.
//
// This is the Chart.js "Using from Node.js" example
// (https://www.chartjs.org/docs/latest/getting-started/using-from-node-js.html)
// adapted to run inside the V8 isolate. The upstream example uses skia-canvas,
// a native Node addon — that cannot load here (there is no Node ABI), so the
// canvas is provided in pure JavaScript instead: svgcanvas implements the
// Canvas2D API on top of a small DOM shim and emits SVG.
//
//   OUTPUT = "svg"  → prints the SVG (no capabilities needed beyond imports)
//   OUTPUT = "png"  → rasterizes with resvg-wasm and writes PNG_PATH
//                     (needs a fetch policy for unpkg.com and an fs policy)
//
// See README.md for the server flags each mode needs.

const OUTPUT = "svg";
const PNG_PATH = "/tmp/chart-out/output.png";

// ── minimal DOM shim ────────────────────────────────────────────────────────
// svgcanvas builds an SVG tree through document.createElementNS and serializes
// it with XMLSerializer, and it tracks the current transform with DOMMatrix.
// None of that exists in the isolate, so here are just-enough implementations.

const esc = (s) =>
  String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

class El {
  constructor(name) {
    this.nodeName = name;
    this.attrs = {};
    this.childNodes = [];
    this.parentNode = null;
  }
  setAttribute(k, v) { this.attrs[k] = v; }
  setAttributeNS(_ns, k, v) { this.attrs[k] = v; }
  getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; }
  appendChild(c) { c.parentNode = this; this.childNodes.push(c); return c; }
  removeChild(c) {
    const i = this.childNodes.indexOf(c);
    if (i >= 0) this.childNodes.splice(i, 1);
    c.parentNode = null;
    return c;
  }
  cloneNode(deep) {
    const n = new El(this.nodeName);
    n.attrs = { ...this.attrs };
    if (deep) this.childNodes.forEach((c) => n.appendChild(c.cloneNode(true)));
    return n;
  }
  toXML() {
    const a = Object.entries(this.attrs).map(([k, v]) => ` ${k}="${esc(v)}"`).join("");
    return this.childNodes.length
      ? `<${this.nodeName}${a}>${this.childNodes.map((c) => c.toXML()).join("")}</${this.nodeName}>`
      : `<${this.nodeName}${a}/>`;
  }
}

class TextNode {
  constructor(t) { this.nodeName = "#text"; this.text = t; this.childNodes = []; this.attrs = {}; }
  cloneNode() { return new TextNode(this.text); }
  toXML() { return esc(this.text); }
}

// Chart.js lays out axes and labels from ctx.measureText(), and there is no font
// engine here — approximate per-glyph advances for a typical sans-serif face.
const NARROW = "iljt.,:;'|!I ";
const WIDE = "mwMW@";
function measure(text, fontSize) {
  let w = 0;
  for (const ch of String(text)) {
    if (NARROW.includes(ch)) w += 0.30;
    else if (WIDE.includes(ch)) w += 0.85;
    else if (ch >= "A" && ch <= "Z") w += 0.68;
    else w += 0.52;
  }
  return w * fontSize;
}

// Parses the CSS font shorthand, e.g. "italic bold 12px Helvetica, sans-serif".
function parseFont(font) {
  const m = /^\s*(?:(italic|oblique|normal)\s+)?(?:(bold|bolder|lighter|normal|\d{3})\s+)?([\d.]+)px\s+(.*)$/
    .exec(font || "");
  return m
    ? { fontStyle: m[1] || "normal", fontWeight: m[2] || "normal", fontSize: `${m[3]}px`, fontFamily: m[4] }
    : { fontStyle: "normal", fontWeight: "normal", fontSize: "10px", fontFamily: "sans-serif" };
}

// The context svgcanvas delegates measureText() to.
const measurer = {
  font: "10px sans-serif",
  measureText(text) {
    const size = parseFloat(parseFont(this.font).fontSize) || 10;
    const width = measure(text, size);
    return {
      width,
      actualBoundingBoxLeft: 0,
      actualBoundingBoxRight: width,
      actualBoundingBoxAscent: size * 0.75,
      actualBoundingBoxDescent: size * 0.25,
    };
  },
};

globalThis.document = {
  createElementNS: (_ns, name) => new El(name),
  createTextNode: (t) => new TextNode(t),
  createElement(name) {
    if (name === "canvas") return { getContext: () => measurer };
    // svgcanvas sets a `font:` shorthand on a <span> and reads back the
    // resolved longhands, so parse them here.
    const el = new El(name);
    el.style = {};
    el.setAttribute = (k, v) => {
      el.attrs[k] = v;
      if (k === "style") {
        const font = /font\s*:\s*([^;]+)/.exec(v);
        if (font) Object.assign(el.style, parseFont(font[1]));
      }
    };
    return el;
  },
};

globalThis.XMLSerializer = class {
  serializeToString(node) { return node.toXML(); }
};

globalThis.DOMMatrix = class DOMMatrix {
  constructor(init) {
    const [a, b, c, d, e, f] = init || [1, 0, 0, 1, 0, 0];
    Object.assign(this, { a, b, c, d, e, f });
  }
  multiply(o) {
    return new DOMMatrix([
      this.a * o.a + this.c * o.b,
      this.b * o.a + this.d * o.b,
      this.a * o.c + this.c * o.d,
      this.b * o.c + this.d * o.d,
      this.a * o.e + this.c * o.f + this.e,
      this.b * o.e + this.d * o.f + this.f,
    ]);
  }
  translate(x, y = 0) { return this.multiply(new DOMMatrix([1, 0, 0, 1, x, y])); }
  scale(x, y = x) { return this.multiply(new DOMMatrix([x, 0, 0, y, 0, 0])); }
  rotate(deg) {
    const r = (deg * Math.PI) / 180;
    return this.multiply(new DOMMatrix([Math.cos(r), Math.sin(r), -Math.sin(r), Math.cos(r), 0, 0]));
  }
};

globalThis.DOMPoint = class DOMPoint {
  constructor(x = 0, y = 0) { this.x = x; this.y = y; }
  matrixTransform(m) {
    return new DOMPoint(m.a * this.x + m.c * this.y + m.e, m.b * this.x + m.d * this.y + m.f);
  }
};

// ── the Chart.js example itself ─────────────────────────────────────────────

const { Chart, CategoryScale, LinearScale, LineController, LineElement, PointElement } =
  await import("npm:chart.js@4.5.1");
const { Context } = await import("npm:svgcanvas@2.6.0");

Chart.register([CategoryScale, LineController, LineElement, LinearScale, PointElement]);

const ctx = new Context({ width: 400, height: 300 });
// Chart.js only needs getContext() plus the pixel dimensions off the canvas.
const canvas = { width: 400, height: 300, style: {}, getContext: () => ctx };
ctx.canvas = canvas;

const chart = new Chart(canvas, {
  type: "line",
  data: {
    labels: ["Red", "Blue", "Yellow", "Green", "Purple", "Orange"],
    datasets: [{ label: "# of Votes", data: [12, 19, 3, 5, 2, 3], borderColor: "red" }],
  },
  // No DOM to resize against, and no rAF loop worth waiting for.
  options: { responsive: false, animation: false, devicePixelRatio: 1 },
});

const svg = ctx.getSerializedSvg(true);
chart.destroy();

if (OUTPUT === "svg") {
  console.log(svg);
} else {
  // resvg compiled to WebAssembly rasterizes the SVG. It ships with no system
  // fonts, so a TTF is fetched and handed to it explicitly.
  const { initWasm, Resvg } = await import("npm:@resvg/resvg-wasm@2.6.2");
  await initWasm(
    await (await fetch("https://unpkg.com/@resvg/resvg-wasm@2.6.2/index_bg.wasm")).arrayBuffer(),
  );
  const font = new Uint8Array(
    await (await fetch("https://unpkg.com/@expo-google-fonts/roboto@0.2.3/Roboto_400Regular.ttf"))
      .arrayBuffer(),
  );

  const png = new Resvg(svg, {
    font: { fontBuffers: [font], defaultFontFamily: "Roboto", loadSystemFonts: false },
    background: "white",
  }).render().asPng();

  await fs.writeFile(PNG_PATH, png);
  console.log(`wrote ${PNG_PATH} (${png.length} bytes)`);
}
