// Binary protocol with the game server. The message layout is documented in server/src/protocol.rs.

export const CHUNK = 64;

export const AIR = 0;
export const ROCK = 1;
export const WATER = 2;
export const WALL = 3;
export const WALL_CRACKED = 4;
export const WALL_CRUMBLING = 5;
export const SPONGE_ORE = 6;
export const SPONGE = 7;
export const RUBBLE = 8;
export const RUBBLE_FALLING = 9;
export const BEDROCK = 10;
export const QUARTZ_ORE = 11;
export const SPRING_ROCK = 12;
export const SPRING = 13;
export const ROCK_CRACKED = 14;

export const TYPE_MASK = 0x3f;
export const SHADOW = 0x40;
export const REVEALED = 0x80;

export const ACT_DIG = 0;
export const ACT_WALL = 1;
export const ACT_PLACE = 2;
export const ITEM_SPONGE = 0;
export const ITEM_RUBBLE = 1;
export const ITEM_ROCK = 2;
export const ITEM_WATER = 3;
export const UPGRADE_WALLS = 0;
export const UPGRADE_PICKAXE = 1;

export enum ActResult {
  None = 0,
  Ok = 1,
  Cooldown = 2,
  NotVisible = 3,
  WrongTarget = 4,
  NoResources = 5,
  OutOfReach = 6,
  Unsupported = 7,
  Locked = 8,
}

export interface Welcome {
  width: number;
  height: number;
  chunk: number;
  spawnX: number;
  spawnY: number;
  tickMs: number;
  cooldowns: { dig: number; collect: number; wall: number; place: number };
  wallCost: number;
  repairCost: number;
  /** Row of the ocean surface around the island. */
  seaLevel: number;
  surface: Uint16Array;
}

export interface PlayerState {
  result: ActResult;
  cooldownLeft: number;
  cooldownTotal: number;
  rock: number;
  water: number;
  sponge: number;
  quartz: number;
  /** Pickaxe level: digging is (1 + 0.1 * level) times as fast. */
  pickaxe: number;
  wallsUnlocked: boolean;
}

export interface Stats {
  online: number;
  recordDepth: number;
  /** 0 dry, 1 light, 2 normal, 3 heavy. */
  rain: number;
  /** Action speed: 1 = normal cooldowns, 2 = twice as fast (few players), 0.5 = half as fast (crowds). */
  speed: number;
}

export interface Handlers {
  welcome(w: Welcome): void;
  /** Run-length encoded chunk contents. */
  chunk(cx: number, cy: number, rle: Uint8Array): void;
  /** Changed cells of one chunk: pairs of (cell index in chunk, value). */
  diff(cx: number, cy: number, entries: DataView, offset: number, count: number): void;
  player(p: PlayerState): void;
  stats(s: Stats): void;
  connected(open: boolean): void;
}

function token(): string {
  const make = () => crypto.randomUUID().replace(/-/g, "");
  try {
    let t = localStorage.getItem("mt-token");
    if (!t) {
      t = make();
      localStorage.setItem("mt-token", t);
    }
    return t;
  } catch {
    return make();
  }
}

/** Connection to the server; reconnects by itself when the connection drops. */
export class Net {
  private ws: WebSocket | null = null;
  private retryMs = 500;

  constructor(private h: Handlers) {
    this.connect();
  }

  get open(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  private connect() {
    const params = new URLSearchParams(location.search);
    const base = params.get("server") ?? `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}`;
    // Dev mode: ?dev=KEY (the key the server printed) is passed on; the server checks it.
    const dev = params.get("dev");
    const ws = new WebSocket(`${base}/ws?token=${token()}${dev ? `&dev=${encodeURIComponent(dev)}` : ""}`);
    ws.binaryType = "arraybuffer";
    ws.onopen = () => {
      this.retryMs = 500;
      this.h.connected(true);
    };
    ws.onmessage = (e) => this.receive(new DataView(e.data as ArrayBuffer));
    ws.onclose = () => {
      this.ws = null;
      this.h.connected(false);
      setTimeout(() => this.connect(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 8000);
    };
    this.ws = ws;
  }

  private receive(d: DataView) {
    let o = 1;
    const u8 = () => d.getUint8(o++);
    const u16 = () => ((o += 2), d.getUint16(o - 2, true));
    const u32 = () => ((o += 4), d.getUint32(o - 4, true));
    switch (d.getUint8(0)) {
      case 1: {
        const width = u32(), height = u32(), chunk = u16(), spawnX = u32(), spawnY = u32(), tickMs = u16();
        const cooldowns = { dig: u16(), collect: u16(), wall: u16(), place: u16() };
        const wallCost = u16(), repairCost = u16();
        const seaLevel = u16();
        const surface = new Uint16Array(width);
        for (let x = 0; x < width; x++) surface[x] = u16();
        this.h.welcome({ width, height, chunk, spawnX, spawnY, tickMs, cooldowns, wallCost, repairCost, seaLevel, surface });
        break;
      }
      case 2: {
        const n = u16();
        for (let k = 0; k < n; k++) {
          const cx = u16(), cy = u16(), len = u16();
          this.h.chunk(cx, cy, new Uint8Array(d.buffer, d.byteOffset + o, len));
          o += len;
        }
        break;
      }
      case 3: {
        const n = u16();
        for (let k = 0; k < n; k++) {
          const cx = u16(), cy = u16(), count = u16();
          this.h.diff(cx, cy, d, o, count);
          o += count * 3;
        }
        break;
      }
      case 4: {
        const result = u8() as ActResult;
        const cooldownLeft = u32(), cooldownTotal = u32(), rock = u32(), water = u32(), sponge = u32();
        const quartz = u32(), pickaxe = u8(), flags = u8();
        this.h.player({ result, cooldownLeft, cooldownTotal, rock, water, sponge, quartz, pickaxe,
          wallsUnlocked: (flags & 1) !== 0 });
        break;
      }
      case 5:
        this.h.stats({ online: u16(), recordDepth: u32(), rain: u8(),
          // Older servers send no speed; never let that break the HUD.
          speed: d.byteLength >= o + 2 ? u16() / 100 : 1 });
        break;
    }
  }

  subscribe(chunks: [number, number][]) {
    const d = new DataView(new ArrayBuffer(3 + chunks.length * 4));
    d.setUint8(0, 1);
    d.setUint16(1, chunks.length, true);
    chunks.forEach(([cx, cy], k) => {
      d.setUint16(3 + k * 4, cx, true);
      d.setUint16(5 + k * 4, cy, true);
    });
    this.ws?.send(d.buffer);
  }

  /** Dev clients only: actions cover size x size cells. */
  brush(size: number) {
    this.ws?.send(new Uint8Array([4, size]));
  }

  buy(upgrade: number) {
    this.ws?.send(new Uint8Array([3, upgrade]));
  }

  action(kind: number, x: number, y: number, arg = 0) {
    const d = new DataView(new ArrayBuffer(11));
    d.setUint8(0, 2);
    d.setUint8(1, kind);
    d.setUint32(2, x, true);
    d.setUint32(6, y, true);
    d.setUint8(10, arg);
    this.ws?.send(d.buffer);
  }
}
