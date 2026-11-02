// Draws the whole saved world as a PNG picture, including everything players cannot see yet.
//
//   node tools/map.mjs [scale=2] [world=server/data/world.bin] [out=server/data/map.png] [area=x,y,width,height]
//
// The optional area draws only part of the world, for example 0,0,200,150 for the top-left corner.
//
// Areas players can currently see are drawn bright, shadow cells a bit darker, hidden cells dark.
// The server saves the world every minute, so the picture can be up to a minute old.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { deflateSync, crc32 } from "node:zlib";

const scale = Number(process.argv[2] ?? 2);
const worldPath = process.argv[3] ?? "server/data/world.bin";
const outPath = process.argv[4] ?? "server/data/map.png";

// World file layout: see World::to_bytes in server/src/world.rs.
if (!existsSync(worldPath)) {
  console.log(`${worldPath} does not exist yet. Start the server (it saves a new world right away), or type save in its window.`);
  process.exit(1);
}
const buf = readFileSync(worldPath);
if (buf.toString("ascii", 0, 4) !== "MTW1") throw new Error(`${worldPath} is not a world file`);
const w = buf.readUInt32LE(4), h = buf.readUInt32LE(8);
const surface = new Uint16Array(w);
for (let x = 0; x < w; x++) surface[x] = buf.readUInt16LE(20 + x * 2);
const cells = buf.subarray(20 + w * 2);

// Stone, granite, slate, basalt, darkstone (as in the game's shader).
const LAYERS = [[153, 133, 107], [163, 128, 120], [77, 84, 105], [51, 51, 59], [64, 28, 41]];
const DIRT = [110, 71, 46];
// Rock hit but not broken keeps its kind: runs of types from 32 for granite, slate, basalt and darkstone.
const WORN = [[32, 2, 1], [34, 4, 2], [38, 7, 3], [45, 11, 4]];

function colour(x, y, t) {
  if (t === 0) return y < surface[x] ? [140, 195, 245] : [48, 51, 66];
  if (t === 2) return [40, 100, 220];
  if (t >= 3 && t <= 5) return [143, 140, 148]; // walls
  if (t === 6 || t === 7) return [219, 173, 56]; // spongerock
  if (t === 8 || t === 9) return [140, 128, 112]; // rubble
  if (t === 11) return [255, 170, 215]; // quartz
  if (t === 12 || t === 13) return [60, 200, 255]; // damp rock and springs
  if (t === 14) return [200, 60, 40]; // cracked ceiling
  if (t >= 26 && t <= 29) return LAYERS[t - 25]; // rock of the deeper layers
  if (t === 30) return [170, 90, 230]; // amethyst
  if (t === 56 || t === 57) return [240, 110, 95]; // swellcoral
  if (t === 58) return [255, 120, 30]; // emberstone
  if (t === 15 || t === 16) return [255, 90, 20]; // magma and molten veins
  if (t === 17 || t === 18) return [70, 62, 66]; // lava rock
  if (t === 19) return [120, 255, 200]; // fire opal
  if (t === 59) return [225, 230, 235]; // steam
  if (t === 1) return LAYERS[0];
  if (t === 31) return DIRT;
  for (const [base, n, layer] of WORN) if (t >= base && t < base + n) return LAYERS[layer];
  if (t === 10) return [40, 40, 52];
  const band = Math.min(Math.max((y - surface[x]) / 160, 0), 3.999);
  const i = Math.floor(band), f = band - i < 0.75 ? 0 : (band - i - 0.75) / 0.25;
  return LAYERS[i].map((c, k) => c + (LAYERS[i + 1][k] - c) * f);
}

const [ax, ay, aw, ah] = (process.argv[5] ?? `0,0,${w},${h}`).split(",").map(Number);
const W = aw * scale, H = ah * scale;
const raw = Buffer.alloc((W * 3 + 1) * H);
for (let y = ay; y < ay + ah; y++) {
  const row = Buffer.alloc(W * 3);
  for (let x = ax; x < ax + aw; x++) {
    const v = cells[y * w + x];
    const t = v & 0x3f, vis = v >> 6;
    const light = vis === 2 ? 1 : vis === 1 ? 0.7 : 0.45;
    const [r, g, b] = colour(x, y, t);
    for (let s = 0; s < scale; s++) {
      const o = ((x - ax) * scale + s) * 3;
      row[o] = r * light;
      row[o + 1] = g * light;
      row[o + 2] = b * light;
    }
  }
  for (let s = 0; s < scale; s++) row.copy(raw, ((y - ay) * scale + s) * (W * 3 + 1) + 1);
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}
const header = Buffer.alloc(13);
header.writeUInt32BE(W, 0);
header.writeUInt32BE(H, 4);
header[8] = 8; // bits per channel
header[9] = 2; // RGB
writeFileSync(outPath, Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", header),
  chunk("IDAT", deflateSync(raw)),
  chunk("IEND", Buffer.alloc(0)),
]));
console.log(`wrote ${outPath} (${W}x${H}, world ${w}x${h})`);
