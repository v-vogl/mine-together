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
uniform bool uCarved;  // carved edges on (see below), switched with the E key
out vec4 outColour;

// The rock layers, top first: stone (a warm sandstone), granite, slate, basalt, darkstone (see kindColour).
const vec3 LAYERS[5] = vec3[5](
  vec3(0.60, 0.52, 0.42), vec3(0.64, 0.50, 0.47), vec3(0.30, 0.33, 0.41),
  vec3(0.20, 0.20, 0.23), vec3(0.25, 0.11, 0.16));
const vec3 MOSS = vec3(0.46, 0.58, 0.30);

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

// Smooth value noise over the world, for colour that flows across many cells instead of repeating per cell.
float vnoise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}

// Each kind of rock (0 dirt, 1 stone, 2 granite, 3 slate, 4 basalt, 5 darkstone) has its own colour and pattern.
// Every cell is one flat colour, and the patterns run across many cells (wavy strata, stripes, columns, veins),
// so the ground reads as one painted mass of rock, not as a wall of textured blocks.
vec3 kindColour(int k, ivec2 c, float n) {
  vec2 p = vec2(c);
  float soft = vnoise(p * 0.09);
  if (k == 0) return vec3(0.43, 0.28, 0.18) * (0.84 + 0.22 * soft + 0.05 * n);
  vec3 col = LAYERS[k - 1] * (0.86 + 0.16 * soft + 0.04 * n);
  if (k == 1) {
    // Stone: soft wavy strata.
    float strata = 0.5 + 0.5 * sin(p.y * 0.45 + 4.0 * vnoise(p * 0.03));
    return col * (0.93 + 0.1 * strata);
  }
  // Granite: a few dark and pale grains.
  if (k == 2) return n < 0.07 ? col * 0.45 : n > 0.95 ? mix(col, vec3(0.92, 0.88, 0.85), 0.6) : col;
  // Slate: thin dark stripes every third row, gently stepping up and down.
  if (k == 3) return col * (mod(p.y + floor(vnoise(p * 0.05) * 6.0), 3.0) < 1.0 ? 0.8 : 1.0);
  // Basalt: upright columns, a dark joint every third column.
  if (k == 4) return col * (mod(p.x + floor(vnoise(p * vec2(0.05, 0.02)) * 6.0), 3.0) < 1.0 ? 0.72 : 1.0 + 0.06 * n);
  // Darkstone: long glowing red veins winding through it.
  float vein = abs(sin(p.x * 0.19 + p.y * 0.11 + 5.0 * vnoise(p * 0.06)));
  float glow = 0.6 + 0.4 * sin(uTime * 1.5 + p.x * 0.05);
  return mix(col, vec3(0.62, 0.14, 0.09) * glow, step(vein, 0.09));
}

// Open space a solid cell can border: visible air or water.
bool openCell(uint v) {
  uint t = v & 63u;
  return (v >> 6) != 0u && (t == 0u || t == 2u);
}

// What shows through where a corner of rock is rounded off: the open cell next to it.
vec3 openColour(uint v, ivec2 nc) {
  float n = hash(vec2(nc));
  if ((v & 63u) == 2u) return waterColour(nc, n);
  return float(nc.y) < surfaceAt(nc.x) ? sky(float(nc.y)) : backWall(nc, n);
}

// The kind of rock a cell type is (see kindColour), or -1; left is set for rock worn down by hits.
int rockKind(uint t, out float left) {
  left = 0.0;
  if (t == 31u) return 0;
  if (t == 1u) return 1;
  if (t >= 26u && t <= 29u) return int(t) - 24;
  if (t >= 32u && t <= 55u) {
    int i = int(t) - 32;
    if (i < 2) { left = float(i + 1); return 2; }
    if (i < 6) { left = float(i - 1); return 3; }
    if (i < 13) { left = float(i - 5); return 4; }
    left = float(i - 12);
    return 5;
  }
  return -1;
}

bool outline(vec2 f, float px) {
  return f.x < px || f.y < px || f.x > 1.0 - px || f.y > 1.0 - px;
}

void main() {
  vec2 screen = vec2(gl_FragCoord.x, uHeight - gl_FragCoord.y);
  vec2 wp = uView + screen / uScale;
  ivec2 c = ivec2(floor(wp));
  vec2 f = fract(wp);

  if (outside(c) && float(c.y) >= uSeaLevel && float(c.y) < uWorld.y) {
    // The endless ocean beyond the edges is only drawn, never simulated: water all the way down, bright near
    // the surface and fading into the dark deep. Where a breach lets it into the island, the water just
    // outside it is lit too, so it reads as the ocean pouring in rather than as a stripe.
    float below = float(c.y) - uSeaLevel;
    float light = below < 4.5 ? 1.0 : 0.55 * exp(-(below - 4.0) / 14.0);
    int edge = c.x < 0 ? 0 : int(uWorld.x) - 1;
    uint e = texelFetch(uCells, ivec2(edge, c.y), 0).r;
    if ((e >> 6) == 2u && (e & 63u) == 2u) {
      float dist = float(c.x < 0 ? -c.x : c.x - edge);
      light = max(light, 1.0 - dist / 7.0);
    }
    outColour = vec4(waterColour(c, hash(vec2(c))) * light, 1.0);
    return;
  }

  uint v = cellAt(c);
  uint t = v & 63u;
  uint vis = v >> 6;
  float n = hash(vec2(c));
  float surface = surfaceAt(c.x);
  float depth = float(c.y) - surface;
  float worn;
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
    // Tufts of grass growing up from the surface dirt below: thin blades of uneven height.
    uint below = cellAt(c + ivec2(0, 1));
    if ((below & 63u) == 31u && (below >> 6) == 2u && float(c.y + 1) - surfaceAt(c.x) < 0.5) {
      float blade = floor(f.x * 8.0);
      float h = hash(vec2(float(c.x) * 8.0 + blade, 7.0));
      if (h > 0.4 && f.y > 1.0 - (0.12 + 0.4 * h)) col = MOSS * (0.8 + 0.35 * h);
    }
  } else if (rockKind(t, worn) >= 0) {
    // Rock of every kind: its look comes from its type, so where layers blend every cell shows its kind (and so
    // how hard it is). Rock that has been hit gets more and darker gouges the fewer hits it has left.
    int k = rockKind(t, worn);
    col = kindColour(k, c, n);
    if (worn > 0.0) {
      float chip = step(1.0 - max(0.75 - 0.06 * worn, 0.12), hash(vec2(c) * 5.0 + floor(f * 4.0) + 3.0));
      col *= 1.0 - 0.55 * chip;
    }
    // Untouched surface dirt has a thin, uneven rim of moss (the tufts grow in the air cell above).
    if (k == 0 && vis == 2u && depth < 0.5 && (cellAt(c - ivec2(0, 1)) & 63u) == 0u &&
        f.y < 0.18 + 0.1 * sin(f.x * 11.0 + n * 20.0)) col = MOSS * (0.85 + 0.25 * n);
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
    // Spongerock: yellow ochre full of dark pores. As ore it is rock with yellow patches; placed as a
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
    // that wanders over them, so it reads as a gem and never as spongerock's round porous blobs.
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
  } else if (t == 30u) {
    // Amethyst: upright purple crystal columns in the rock, each with a pale tip and a glint now and then,
    // unlike quartz's slanted pink shards.
    col = rockColour(c, depth, n);
    float u = fract(f.x * 3.0 + n * 0.37);
    float column = step(0.18, u) * step(u, 0.82);
    vec3 crystal = mix(vec3(0.42, 0.22, 0.66), vec3(0.78, 0.6, 0.98), (1.0 - smoothstep(0.0, 0.55, f.y)) * 0.8);
    crystal = mix(crystal, vec3(0.3, 0.15, 0.5), step(0.62, u) * 0.5);
    col = mix(col, crystal, column);
    col += pow(max(0.0, sin(uTime * 1.4 + n * 50.0)), 26.0) * column * 0.45;
  } else if (t == 56u || t == 57u) {
    // Swellcoral: fleshy coral pink, covered in polyps (dots on a 4 x 4 grid) that slowly swell and shrink. Living
    // coral is a solid mass; reef coral is branches standing in the pocket's water, paler while it sleeps.
    vec2 q = floor(f * 4.0);
    float h = hash(vec2(c) * 4.0 + q);
    vec3 coral = vec3(0.91, 0.44, 0.38) * (0.85 + 0.25 * vnoise(wp * 1.5));
    float polyp = step(0.55, h) * step(length(fract(f * 4.0) - 0.5), 0.2 + 0.12 * sin(uTime * 1.6 + h * 20.0));
    col = mix(coral, vec3(1.0, 0.8, 0.68), polyp);
    if (t == 56u) {
      float branch = max(step(abs(q.x - 1.5), 0.6), step(0.6, h));
      col = mix(waterColour(c, n), mix(col, vec3(0.75, 0.68, 0.66), 0.35), branch);
    }
  } else if (t == 58u) {
    // Emberstone: near-black rock split by glowing orange cracks that run on across cells and pulse slowly.
    vec2 p = floor(wp * 4.0) / 4.0;
    float crack = abs(vnoise(p * 1.3) - 0.5) + 0.6 * abs(vnoise(p * 2.7 + 9.0) - 0.5);
    float pulse = 0.65 + 0.35 * sin(uTime * 1.8 + vnoise(p * 0.5) * 6.0);
    col = vec3(0.11, 0.08, 0.08) * (0.9 + 0.2 * n);
    col = mix(col, vec3(1.0, 0.48, 0.12) * pulse, step(crack, 0.08));
    col = mix(col, vec3(0.55, 0.16, 0.06) * pulse, step(crack, 0.14) * step(0.08, crack));
  } else if (t == 59u) {
    // Steam: puffs of white drifting upwards over whatever is behind it.
    col = float(c.y) < surface ? sky(float(c.y)) : backWall(c, n);
    vec2 p = floor(wp * 4.0) / 4.0;
    float puff = vnoise((p + vec2(0.0, uTime * 0.8)) * 1.4);
    col = mix(col, vec3(0.92, 0.94, 0.96), 0.35 + 0.45 * smoothstep(0.3, 0.8, puff));
  } else if (t == 15u) {
    // Magma: glowing orange and yellow, slowly churning, with dark crust drifting on it.
    vec2 p = floor(wp * 4.0) / 4.0;
    float churn = vnoise(p * 0.9 + vec2(uTime * 0.25, -uTime * 0.12));
    col = mix(vec3(1.0, 0.42, 0.08), vec3(1.0, 0.82, 0.32), smoothstep(0.35, 0.8, churn));
    col = mix(col, vec3(0.45, 0.1, 0.04), step(vnoise(p * 1.7 - vec2(uTime * 0.1, 0.0)), 0.22));
  } else if (t == 16u) {
    // A molten vein: dark red rock glowing from inside, pulsing as it pushes upwards.
    vec2 p = floor(wp * 4.0) / 4.0;
    float glow = 0.55 + 0.45 * sin(uTime * 3.0 - p.y * 0.8 + vnoise(p) * 4.0);
    col = mix(vec3(0.3, 0.06, 0.04), vec3(1.0, 0.5, 0.12), glow * smoothstep(0.2, 0.7, vnoise(p * 1.6)));
  } else if (t == 17u || t == 18u) {
    // Lava rock: dark grey with a hint of red, full of round gas holes; chipped lava rock has a gouge.
    vec2 q = fract(f * 3.0) - 0.5;
    float h = hash(vec2(c) * 3.0 + floor(f * 3.0));
    col = vec3(0.25, 0.22, 0.23) * (0.85 + 0.25 * vnoise(wp * 0.7) + 0.08 * n);
    col *= 1.0 - 0.55 * step(0.6, h) * step(length(q), 0.22 + 0.1 * h);
    if (t == 18u) col *= 1.0 - 0.5 * step(abs(f.y - 0.35 - 0.3 * f.x), 0.08);
  } else if (t == 19u) {
    // Fire opal: a rounded gem in lava rock whose colours shift between fire and green-blue flashes.
    col = vec3(0.25, 0.22, 0.23) * (0.85 + 0.15 * n);
    vec2 g = floor(f * 6.0) / 6.0 + 1.0 / 12.0;
    float gem = step(length(g - 0.5), 0.34);
    float hue = vnoise(floor(wp * 6.0) * 0.5 + uTime * 0.3);
    vec3 fire = 0.5 + 0.5 * cos(6.2832 * (hue + vec3(0.0, 0.33, 0.67)));
    col = mix(col, mix(vec3(1.0, 0.45, 0.15), fire, 0.45), gem);
  } else if (t == 10u) {
    // Bedrock: dark, cold and cracked, so it reads as "this does not break".
    float crack = step(0.82, hash(vec2(c.x / 2, c.y / 2) + 3.0)) * step(0.5, fract(f.x + f.y));
    col = vec3(0.16, 0.16, 0.20) * (0.85 + 0.25 * n) * (1.0 - 0.35 * crack);
  } else {
    col = vec3(1.0, 0.0, 1.0); // unknown type: bright pink, so it gets noticed
  }

  // Carved edges: where solid ground meets open space, the corners are rounded off and the top edge catches the
  // light, so tunnels look dug out of one mass of rock rather than stacked from cubes. Walls stay crisp bricks.
  // Zoomed in only; far out the cells are too small to show it. Off unless switched on with the E key.
  if (uCarved && vis == 2u && uScale >= 5.0 && t != 0u && t != 2u && t != 9u && t != 15u && t != 59u && !(t >= 3u && t <= 5u)) {
    uint up = cellAt(c - ivec2(0, 1)), dn = cellAt(c + ivec2(0, 1));
    bool oU = openCell(up), oD = openCell(dn);
    bool oL = openCell(cellAt(c - ivec2(1, 0))), oR = openCell(cellAt(c + ivec2(1, 0)));
    float e = 0.12;
    if (oU && f.y < e) col *= 1.22;
    if (oD && f.y > 1.0 - e) col *= 0.72;
    if (oL && f.x < e) col *= 1.08;
    if (oR && f.x > 1.0 - e) col *= 0.88;
    const float r = 0.42;
    if (oU && oL && length(f - vec2(r)) > r && f.x < r && f.y < r) col = openColour(up, c - ivec2(0, 1));
    if (oU && oR && length(f - vec2(1.0 - r, r)) > r && f.x > 1.0 - r && f.y < r) col = openColour(up, c - ivec2(0, 1));
    if (oD && oL && length(f - vec2(r, 1.0 - r)) > r && f.x < r && f.y > 1.0 - r) col = openColour(dn, c + ivec2(0, 1));
    if (oD && oR && length(f - vec2(1.0 - r)) > r && f.x > 1.0 - r && f.y > 1.0 - r) col = openColour(dn, c + ivec2(0, 1));
  }

  // In shadow everything is darkened, except glowing magma and veins: a vein coming up shows before it breaks in.
  if (vis == 1u) col *= (t == 15u || t == 16u) ? 0.75 : 0.32;

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
      "uHoverColour", "uQueued", "uBrush", "uCarved"]) {
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
    queued: [number, number] | null; rain: number; brush?: number; carved?: boolean;
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
    gl.uniform1i(u.uCarved, opts.carved ? 1 : 0);
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
