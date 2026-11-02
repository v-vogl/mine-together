import { CHUNK } from "./net";

/** The client's copy of the visible world: one byte per cell, as the server sends it. */
export class World {
  readonly cells: Uint8Array;
  readonly chunksWide: number;
  readonly chunksHigh: number;
  /** Chunks changed since the renderer last uploaded them. */
  readonly dirty = new Set<number>();

  constructor(readonly width: number, readonly height: number, readonly surface: Uint16Array,
    readonly seaLevel: number) {
    this.cells = new Uint8Array(width * height);
    this.chunksWide = width / CHUNK;
    this.chunksHigh = height / CHUNK;
  }

  get(x: number, y: number): number {
    if (x < 0 || y < 0 || x >= this.width || y >= this.height) return 0;
    return this.cells[y * this.width + x];
  }

  setChunk(cx: number, cy: number, rle: Uint8Array) {
    let k = 0;
    for (let p = 0; p + 1 < rle.length; p += 2) {
      const value = rle[p];
      for (let n = rle[p + 1]; n > 0; n--, k++) {
        const x = cx * CHUNK + (k % CHUNK), y = cy * CHUNK + Math.floor(k / CHUNK);
        this.cells[y * this.width + x] = value;
      }
    }
    this.dirty.add(cy * this.chunksWide + cx);
  }

  applyDiff(cx: number, cy: number, d: DataView, offset: number, count: number) {
    for (let k = 0; k < count; k++) {
      const local = d.getUint16(offset + k * 3, true);
      const x = cx * CHUNK + (local % CHUNK), y = cy * CHUNK + Math.floor(local / CHUNK);
      this.cells[y * this.width + x] = d.getUint8(offset + k * 3 + 2);
    }
    this.dirty.add(cy * this.chunksWide + cx);
  }
}
