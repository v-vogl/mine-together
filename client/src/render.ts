import { CHUNK } from "./net";
import type { World } from "./world";
import type { Camera } from "./camera";

const VERT = `#version 300 es
in vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }
`;

// Every screen pixel looks up its cell and colours it here: palette per type, per-cell shade noise,
// rock layers by depth, darkened shadow cells, black hidden cells and water shimmer.
const FRAG = `#version 300 es
precision highp float;
precision highp int;
precision highp usampler2D;

uniform usampler2D uCells;
uniform usampler2D uSurface;
uniform vec2 uWorld;
uniform vec2 uView;    // world position of the top-left screen corner, in cells
uniform float uScale;  // screen pixels per cell
uniform float uHeight; // canvas height in pixels
uniform float uTime;
uniform float uRain;
uniform float uSeaLevel; // row of the ocean surface around the island
uniform ivec2 uHover;
uniform vec3 uHoverColour;
uniform ivec2 uQueued;
uniform int uBrush;    // half the size of the dev brush; 0 for a single cell
out vec4 outColour;

const vec3 LAYERS[5] = vec3[5](
  vec3(0.47, 0.33, 0.21), vec3(0.58, 0.40, 0.25), vec3(0.45, 0.43, 0.42),
  vec3(0.33, 0.33, 0.38), vec3(0.36, 0.22, 0.24));

float hash(vec2 p) {
  p = fract(p * vec2(123.34, 456.21));
  p += dot(p, p + 45.32);
  return fract(p.x * p.y);
}

bool outside(ivec2 c) {
  return c.x < 0 || float(c.x) >= uWorld.x;
}

float surfaceAt(int x) {
  if (x < 0 || float(x) >= uWorld.x) return 1e9; // beyond the edges, all open space is sky
  return float(texelFetch(uSurface, ivec2(x, 0), 0).r);
}

uint cellAt(ivec2 c) {
  if (c.y < 0) return 128u;                   // open sky
  if (float(c.y) >= uWorld.y) return 0u;      // below the world: looks like unexplored rock
  if (outside(c)) {
    // The endless ocean beyond the edges looks exactly like the island's edge column on that row: the sea
    // surface lines up even while a breach drains the sea, and deep water is black on both sides.
    // Above sea level it is always sky: a raindrop falling down the edge column must not smear a line of
    // water across the whole ocean.
    if (float(c.y) < uSeaLevel) return 128u;
    int edge = c.x < 0 ? 0 : int(uWorld.x) - 1;
    uint e = texelFetch(uCells, ivec2(edge, c.y), 0).r;
    if ((e >> 6) == 2u && ((e & 63u) == 0u || (e & 63u) == 2u)) return e;
    return float(c.y) < uSeaLevel ? 128u : 0u;
  }
  return texelFetch(uCells, c, 0).r;
}

vec3 waterColour(ivec2 c, float n) {
  float wave = sin(uTime * 2.5 + float(c.x) * 0.55 + float(c.y) * 0.3) * 0.5 + 0.5;
  vec3 col = mix(vec3(0.12, 0.33, 0.78), vec3(0.2, 0.48, 0.92), wave * 0.6 + n * 0.2);
  if ((cellAt(c - ivec2(0, 1)) & 63u) == 0u) col = mix(col, vec3(0.6, 0.8, 1.0), 0.2);
  return col;
}

vec3 sky(float y) {
  vec3 col = mix(vec3(0.40, 0.66, 0.95), vec3(0.78, 0.90, 1.0), clamp(y / uSeaLevel, 0.0, 1.0));
  return mix(col, vec3(0.42, 0.47, 0.55), uRain * 0.6);
}

vec3 rockColour(ivec2 c, float depth, float n) {
  float band = clamp(depth / 160.0 + (hash(vec2(c.x / 9, c.y / 7)) - 0.5) * 0.12, 0.0, 3.999);
  int i = int(band);
  vec3 col = mix(LAYERS[i], LAYERS[i + 1], smoothstep(0.75, 1.0, fract(band)));
  return col * (0.82 + 0.3 * n);
}

// Dug-out space: a cool blue-grey back wall with a blocky texture, so it never looks like the warm, darkened
// rock in the shadow rows.
vec3 backWall(ivec2 c, float n) {
  float block = hash(vec2(c.x / 3, c.y / 2) + 17.0);
  return vec3(0.19, 0.20, 0.26) * (0.85 + 0.12 * block + 0.08 * n);
}

bool outline(vec2 f, float px) {
  return f.x < px || f.y < px || f.x > 1.0 - px || f.y > 1.0 - px;
}

void main() {
  vec2 screen = vec2(gl_FragCoord.x, uHeight - gl_FragCoord.y);
  vec2 wp = uView + screen / uScale;
  ivec2 c = ivec2(floor(wp));
  vec2 f = fract(wp);

  uint v = cellAt(c);
  uint t = v & 63u;
  uint vis = v >> 6;
  float n = hash(vec2(c));
  float surface = surfaceAt(c.x);
  float depth = float(c.y) - surface;
  vec3 col;

  if (vis == 0u) {
    // Unexplored ground. Nothing about the cell is known here, only where the island's surface is (which every
    // client gets), so draw a faint hint of the rock layers: the island reads as land when zoomed out instead
    // of a black hole. It fades with depth and towards the shores, so the sea and the deep stay black.
    float land = smoothstep(0.0, 40.0, uSeaLevel - surface);
    float glow = float(c.y) < uWorld.y ? land * exp(-max(depth, 0.0) / 260.0) : 0.0;
    col = rockColour(c, depth, 0.5) * 0.2 * glow;
  } else if (t == 0u) {
    col = float(c.y) < surface ? sky(float(c.y)) : backWall(c, n);
  } else if (t == 1u) {
    col = rockColour(c, depth, n);
    // A grass top on untouched surface rock.
    if (vis == 2u && depth < 0.5 && (cellAt(c - ivec2(0, 1)) & 63u) == 0u) col = vec3(0.30, 0.62, 0.22) * (0.85 + 0.3 * n);
  } else if (t == 2u) {
    col = waterColour(c, n);
  } else if (t >= 3u && t <= 5u) {
    // Walls: grey blocks; cracked and crumbling ones get darker with visible crack lines.
    col = vec3(0.56, 0.55, 0.58) * (0.9 + 0.15 * n) * (1.0 - 0.12 * float(t - 3u));
    if (uScale >= 6.0 && outline(f, 1.0 / uScale)) col *= 0.6;
    float px = 1.2 / uScale;
    if (t >= 4u && abs(f.y - (0.3 + 0.4 * f.x)) < px) col *= 0.35;
    if (t == 5u && abs(f.y - (0.9 - 0.7 * f.x)) < px) col *= 0.35;
  } else if (t == 6u || t == 7u) {
    // Spongestone: yellow ochre full of dark pores. As ore it is rock with yellow patches; placed as a
    // block it is all yellow with an outline.
    vec3 ochre = vec3(0.86, 0.68, 0.22) * (0.88 + 0.2 * n);
    vec2 cellPos = floor(f * 4.0);
    float pore = step(0.72, hash(vec2(c) * 4.0 + cellPos));
    col = mix(ochre, ochre * 0.45, pore);
    if (t == 6u) col = mix(rockColour(c, depth, n), col, step(0.45, hash(vec2(c) * 4.0 + cellPos + 9.0)));
    if (t == 7u && uScale >= 6.0 && outline(f, 1.0 / uScale)) col *= 0.65;
  } else if (t == 8u || t == 9u) {
    // Rubble: a jumble of loose grey-brown stones with dark gaps between them, clearly not solid rock or a
    // neat wall.
    vec2 q = f * 3.0;
    vec2 stone = floor(q);
    float s = hash(vec2(c) * 3.0 + stone);
    vec2 g = fract(q) - 0.5;
    float gap = step(0.36 + 0.08 * s, max(abs(g.x), abs(g.y)));
    col = mix(vec3(0.42, 0.38, 0.33), vec3(0.62, 0.57, 0.50), s) * (1.0 - 0.55 * gap);
  } else if (t == 11u) {
    // Quartz: rock with pale pink crystal shards, long thin diagonal facets with a bright edge, and a glint
    // that wanders over them, so it reads as a gem and never as spongestone's round porous blobs.
    col = rockColour(c, depth, n);
    float lean = hash(vec2(c) + 5.0) < 0.5 ? 1.0 : -1.0;
    float u = fract((f.x * lean + f.y) * 2.0 + n);
    float shard = step(0.25, u) * step(u, 0.85);
    vec3 crystal = mix(vec3(0.93, 0.72, 0.82), vec3(1.0, 0.93, 0.97), smoothstep(0.65, 0.85, u));
    col = mix(col, crystal, shard);
    float glint = pow(max(0.0, sin(uTime * 1.7 + n * 40.0)), 24.0);
    col += glint * shard * 0.5;
  } else if (t == 12u) {
    // Damp rock: darker rock with wet streaks running down it and a drop that glistens now and then, so a
    // spring behind it can be spotted before it is dug open.
    col = rockColour(c, depth, n) * 0.75;
    float streak = step(0.8, hash(vec2(floor(f.x * 4.0), float(c.x) + float(c.y) * 7.0)));
    col = mix(col, vec3(0.16, 0.26, 0.36), streak * 0.7);
    float drop = pow(max(0.0, sin(uTime * 1.3 + n * 30.0)), 30.0) * step(length(f - vec2(0.5, 0.8)), 0.15);
    col += vec3(0.4, 0.6, 0.8) * drop;
  } else if (t == 13u) {
    // An open spring: water welling up from a hole in the back wall, drawn as pixel art (8 x 8 dots per cell)
    // with a ring that steps outwards one frame at a time, trailed by a fainter one.
    col = backWall(c, n);
    vec2 p = floor(f * 8.0) - 3.5;
    float r = floor(length(p));
    float wave = mod(floor(uTime * 6.0 + n * 3.0), 3.0) + 1.0;
    if (r < 1.0) col = vec3(0.2, 0.48, 0.92);
    else if (r == wave) col = vec3(0.4, 0.7, 1.0);
    else if (r == wave - 1.0) col = mix(col, vec3(0.2, 0.48, 0.92), 0.5);
  } else if (t == 14u) {
    // A ceiling about to cave in: rock split by jagged dark cracks, flickering slightly as it settles.
    col = rockColour(c, depth, n);
    float zig = abs(fract(f.y * 3.0 + n) - 0.5) * 0.5;
    float crack = step(abs(f.x - 0.35 - zig), 0.06) + step(abs(f.y - 0.6 + zig * 0.6), 0.05);
    col *= 1.0 - 0.7 * min(crack, 1.0);
    col *= 0.9 + 0.1 * sin(uTime * 20.0 + n * 50.0);
  } else if (t == 10u) {
    // Bedrock: dark, cold and cracked, so it reads as "this does not break".
    float crack = step(0.82, hash(vec2(c.x / 2, c.y / 2) + 3.0)) * step(0.5, fract(f.x + f.y));
    col = vec3(0.16, 0.16, 0.20) * (0.85 + 0.25 * n) * (1.0 - 0.35 * crack);
  } else {
    col = vec3(1.0, 0.0, 1.0); // unknown type: bright pink, so it gets noticed
  }

  if (vis == 1u) col *= 0.32;

  float px = max(1.0 / uScale, 0.08);
  if (c == uQueued && outline(f, px)) col = vec3(1.0, 0.85, 0.2);
  // The hover outline goes around the whole brush (a single cell for normal players).
  ivec2 d = c - uHover;
  if (max(abs(d.x), abs(d.y)) <= uBrush) {
    bool edge = (d.x == -uBrush && f.x < px) || (d.x == uBrush && f.x > 1.0 - px) ||
      (d.y == -uBrush && f.y < px) || (d.y == uBrush && f.y > 1.0 - px);
    if (edge) col = uHoverColour;
  }

  outColour = vec4(col, 1.0);
}
`;

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const s = gl.createShader(type)!;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader error");
  return s;
}

export class Renderer {
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private u: Record<string, WebGLUniformLocation | null> = {};
  private cellsTex: WebGLTexture | null = null;
  private surfaceTex: WebGLTexture | null = null;

  constructor(private canvas: HTMLCanvasElement) {
    const gl = canvas.getContext("webgl2", { antialias: false });
    if (!gl) throw new Error("WebGL2 is not available in this browser");
    this.gl = gl;

    const prog = gl.createProgram()!;
    gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, VERT));
    gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, FRAG));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? "link error");
    this.prog = prog;
    for (const name of ["uCells", "uSurface", "uWorld", "uView", "uScale", "uHeight", "uTime", "uRain", "uSeaLevel", "uHover",
      "uHoverColour", "uQueued", "uBrush"]) {
      this.u[name] = gl.getUniformLocation(prog, name);
    }

    // One triangle that covers the whole screen.
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(prog, "aPos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  }

  private texture(): WebGLTexture {
    const gl = this.gl;
    const tex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    return tex;
  }

  setWorld(world: World) {
    const gl = this.gl;
    if (this.cellsTex) gl.deleteTexture(this.cellsTex);
    if (this.surfaceTex) gl.deleteTexture(this.surfaceTex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);

    this.cellsTex = this.texture();
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8UI, world.width, world.height, 0, gl.RED_INTEGER, gl.UNSIGNED_BYTE,
      world.cells);
    this.surfaceTex = this.texture();
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R16UI, world.width, 1, 0, gl.RED_INTEGER, gl.UNSIGNED_SHORT, world.surface);
    world.dirty.clear();
  }

  /** Copy changed chunks into the texture. */
  private upload(world: World) {
    if (world.dirty.size === 0 || !this.cellsTex) return;
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.cellsTex);
    gl.pixelStorei(gl.UNPACK_ROW_LENGTH, world.width);
    for (const c of world.dirty) {
      const cx = c % world.chunksWide, cy = Math.floor(c / world.chunksWide);
      gl.pixelStorei(gl.UNPACK_SKIP_PIXELS, cx * CHUNK);
      gl.pixelStorei(gl.UNPACK_SKIP_ROWS, cy * CHUNK);
      gl.texSubImage2D(gl.TEXTURE_2D, 0, cx * CHUNK, cy * CHUNK, CHUNK, CHUNK, gl.RED_INTEGER, gl.UNSIGNED_BYTE,
        world.cells);
    }
    gl.pixelStorei(gl.UNPACK_ROW_LENGTH, 0);
    gl.pixelStorei(gl.UNPACK_SKIP_PIXELS, 0);
    gl.pixelStorei(gl.UNPACK_SKIP_ROWS, 0);
    world.dirty.clear();
  }

  draw(world: World, cam: Camera, time: number, opts: {
    hover: [number, number] | null; hoverColour: [number, number, number];
    queued: [number, number] | null; rain: number; brush?: number;
  }) {
    const gl = this.gl, u = this.u;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.round(this.canvas.clientWidth * dpr), h = Math.round(this.canvas.clientHeight * dpr);
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    this.upload(world);

    gl.viewport(0, 0, w, h);
    gl.useProgram(this.prog);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.cellsTex);
    gl.uniform1i(u.uCells, 0);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.surfaceTex);
    gl.uniform1i(u.uSurface, 1);
    gl.uniform2f(u.uWorld, world.width, world.height);
    gl.uniform2f(u.uView, cam.x, cam.y);
    gl.uniform1f(u.uScale, cam.scale * dpr);
    gl.uniform1f(u.uHeight, h);
    gl.uniform1f(u.uTime, time);
    gl.uniform1f(u.uRain, opts.rain);
    gl.uniform1f(u.uSeaLevel, world.seaLevel);
    // "No cell" must be far outside anything the camera can show, the ocean beyond the edges included.
    const none: [number, number] = [-1e6, -1e6];
    const hover = opts.hover ?? none, queued = opts.queued ?? none;
    gl.uniform2i(u.uHover, hover[0], hover[1]);
    gl.uniform3f(u.uHoverColour, ...opts.hoverColour);
    gl.uniform2i(u.uQueued, queued[0], queued[1]);
    gl.uniform1i(u.uBrush, Math.floor((opts.brush ?? 1) / 2));
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}
