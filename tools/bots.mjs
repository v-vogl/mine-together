// Load test and playtest helpers: simulated players with different roles.
//
//   node tools/bots.mjs [bots=100] [seconds=60] [server=ws://localhost:8080] [--roles=digger:6,explorer:2,...] [--key=KEY]
//
// Against a server on another machine, all bots share one internet address and would be limited like three
// players; --key=KEY (the server's dev key) lifts that for load tests. Locally no key is needed.
//
// Roles (default mix in DEFAULT_ROLES; --roles gives weights, e.g. --roles=flooder:1,builder:1):
//   digger    digs down near the crater and follows its own dig
//   explorer  digs anywhere in the bowl, wandering to a new spot now and then
//   builder   defends: repairs cracked walls, walls off spreading water, places spongerock next to water,
//             and digs for rock otherwise
//   capper    griefer: builds walls over the tops of shafts and tunnels to seal them off
//   flooder   griefer: digs a shaft by a shore, tunnels inland below sea level until it meets someone
//             else's dig, then goes back and breaks through to the ocean
//
// The server prints tick time and bandwidth while they play; this script prints what the bots did.

const args = process.argv.slice(2).filter((a) => !a.startsWith("--"));
const BOTS = Number(args[0] ?? 100);
const SECONDS = Number(args[1] ?? 60);
const SERVER = args[2] ?? "ws://localhost:8080";
if (!Number.isFinite(BOTS) || !Number.isFinite(SECONDS) || !/^wss?:\/\//.test(SERVER)) {
  console.log("usage: node tools/bots.mjs [bots] [seconds] [ws://server] [--roles=digger:6,flooder:1]");
  process.exit(1);
}
const DEFAULT_ROLES = { digger: 6, explorer: 2, builder: 1, capper: 0.5, flooder: 0.5 };
const rolesArg = process.argv.find((a) => a.startsWith("--roles="));
const keyArg = process.argv.find((a) => a.startsWith("--key="));
const BOT_KEY = keyArg ? `&bot=${encodeURIComponent(keyArg.slice(6))}` : "";
const ROLES = rolesArg
  ? Object.fromEntries(rolesArg.slice(8).split(",").map((r) => { const [n, w] = r.split(":"); return [n, Number(w ?? 1)]; }))
  : DEFAULT_ROLES;

const CHUNK = 64;
const AIR = 0, ROCK = 1, WATER = 2, WALL = 3, WALL_CRACKED = 4, WALL_CRUMBLING = 5, SPONGE_ORE = 6, QUARTZ_ORE = 11;
const SPRING_ROCK = 12, ROCK_CRACKED = 14;
// Plain rock as far as digging goes (damp rock opens a spring, which is the point of a playtest).
const rocky = (t) => t === ROCK || t === SPRING_ROCK || t === ROCK_CRACKED || (t >= 26 && t <= 29) || t === 31 ||
  (t >= 32 && t <= 55);
const REVEALED = 0x80;
const ACT_DIG = 0, ACT_WALL = 1, ACT_PLACE = 2;
const WALL_COST = 2;
const WALLS_UNLOCK_COST = 10;
const run = Date.now().toString(36);
const NEIGHBOURS = [[1, 0], [-1, 0], [0, 1], [0, -1]];

const stats = { bytes: 0, messages: 0, actions: 0, ok: 0, open: 0, closed: 0 };
const roleStats = {};

/** Pick a role for bot number n so the whole crowd matches the weights in ROLES. */
function roleFor(n) {
  const entries = Object.entries(ROLES).filter(([, w]) => w > 0);
  const total = entries.reduce((s, [, w]) => s + w, 0);
  let pick = ((n * 0.6180339887) % 1) * total;
  for (const [name, w] of entries) {
    if ((pick -= w) < 0) return name;
  }
  return entries[0][0];
}

function bot(n) {
  const role = roleFor(n);
  roleStats[role] ??= { bots: 0, ok: 0, walls: 0, sponges: 0, repairs: 0, breaches: 0 };
  const mine = roleStats[role];
  mine.bots++;
  const ws = new WebSocket(`${SERVER}/ws?token=bot-${run}-${n}${BOT_KEY}`);
  ws.binaryType = "arraybuffer";
  const chunks = new Map();
  let world = null, player = null, readyAt = 0, last = null, subscribedKey = "";
  // Where the bot is working.
  let focusX = 0, focusY = 0;
  // Flooder plan: cells to dig in order, then the breach towards the ocean.
  let plan = null;
  // Diggers: tries in a row that found nothing to dig.
  let misses = 0;

  const cell = (x, y) => {
    if (!world || x < 0 || y < 0 || x >= world.w || y >= world.h) return 0;
    const c = chunks.get(`${Math.floor(x / CHUNK)},${Math.floor(y / CHUNK)}`);
    return c ? c[(y % CHUNK) * CHUNK + (x % CHUNK)] : 0;
  };
  const type = (x, y) => cell(x, y) & 0x3f;
  const visible = (x, y) => (cell(x, y) & REVEALED) !== 0;
  const openAir = (x, y) => cell(x, y) === REVEALED;
  const solid = (x, y) => visible(x, y) && type(x, y) !== AIR && type(x, y) !== WATER;
  // Same rules as the server: only visible cells that touch visible air, and support in the open sky.
  const reachable = (x, y) => visible(x, y) && (openAir(x, y) || NEIGHBOURS.some(([dx, dy]) => openAir(x + dx, y + dy)));
  const supported = (x, y) => y >= world.surface[x] || NEIGHBOURS.some(([dx, dy]) => solid(x + dx, y + dy));
  const underground = (x, y) => y > world.surface[x];

  function subscribe() {
    const cx = Math.floor(focusX / CHUNK), cy = Math.floor(focusY / CHUNK);
    const key = `${cx},${cy}`;
    if (key === subscribedKey) return;
    subscribedKey = key;
    const list = [];
    for (let y = cy - 1; y <= cy + 1; y++) for (let x = cx - 2; x <= cx + 2; x++) {
      if (x >= 0 && y >= 0 && x < world.w / CHUNK && y < world.h / CHUNK) list.push([x, y]);
    }
    const d = new DataView(new ArrayBuffer(3 + list.length * 4));
    d.setUint8(0, 1);
    d.setUint16(1, list.length, true);
    list.forEach(([x, y], k) => (d.setUint16(3 + k * 4, x, true), d.setUint16(5 + k * 4, y, true)));
    ws.send(d.buffer);
  }

  /** Random reachable cells around the focus that pass `test`. */
  function sample(radiusX, radiusY, tries, test) {
    const found = [];
    for (let k = 0; k < tries; k++) {
      const x = focusX + Math.round((Math.random() - 0.5) * 2 * radiusX);
      const y = focusY + Math.round((Math.random() - 0.5) * 2 * radiusY);
      if (reachable(x, y) && test(x, y)) found.push([x, y]);
    }
    return found;
  }

  const dig = ([x, y]) => ({ kind: ACT_DIG, x, y });
  const deepest = (cells) => cells.reduce((best, c) => (!best || c[1] + Math.random() * 12 > best[1] ? c : best), null);
  const isWall = (x, y) => type(x, y) >= WALL && type(x, y) <= WALL_CRUMBLING;
  // Builders never hit walls; everyone else digs through whatever is in the way.
  const diggable = (x, y) => rocky(type(x, y)) || type(x, y) === SPONGE_ORE || type(x, y) === QUARTZ_ORE || (isWall(x, y) && role !== "builder");

  function wander() {
    const margin = 200;
    focusX = margin + Math.floor(Math.random() * (world.w - 2 * margin));
    focusY = world.surface[focusX] + Math.floor(Math.random() * 40);
  }

  // ---- Roles -------------------------------------------------------------------------------------

  function digger() {
    const ore = sample(30, 30, 60, (x, y) => type(x, y) === SPONGE_ORE || type(x, y) === QUARTZ_ORE || type(x, y) === 30);
    if (ore.length) return dig(ore[0]);
    // Rain water on the floor of the pit blocks the rock under it, so scoop it out too.
    const c = deepest(sample(30, 30, 80, (x, y) => rocky(type(x, y)) || (type(x, y) === WATER && underground(x, y))));
    if (c) {
      misses = 0;
      return dig(c);
    }
    // Nothing in reach here (the dig drowned, or the pit grew away): look a bit higher up and to the side.
    if (++misses > 10) {
      misses = 0;
      focusX = Math.max(0, Math.min(world.w - 1, focusX + Math.round((Math.random() - 0.5) * 60)));
      focusY = Math.max(world.surface[focusX], focusY - 15);
    }
    return null;
  }

  function explorer() {
    if (Math.random() < 0.01) wander();
    const c = sample(25, 25, 80, diggable)[0];
    if (!c) wander();
    return c && dig(c);
  }

  function builder() {
    // 1. Repair cracked walls.
    if (player.walls) {
      const cracked = sample(30, 30, 120, (x, y) => type(x, y) === WALL_CRACKED || type(x, y) === WALL_CRUMBLING)[0];
      if (cracked) return mine.repairs++, { kind: ACT_WALL, x: cracked[0], y: cracked[1] };
    }
    // 2. Water that touches dug-out air beside or below it: soak it up, or wall off the air.
    const front = sample(30, 30, 160, (x, y) => type(x, y) === WATER && underground(x, y) &&
      NEIGHBOURS.some(([dx, dy]) => dy >= 0 && openAir(x + dx, y + dy)));
    if (front.length) {
      const [wx, wy] = front[0];
      const gaps = NEIGHBOURS.map(([dx, dy]) => [wx + dx, wy + dy]).filter(([x, y]) => openAir(x, y) && y >= wy && supported(x, y));
      if (gaps.length && player.sponge > 0) return mine.sponges++, { kind: ACT_PLACE, x: gaps[0][0], y: gaps[0][1], arg: 0 };
      if (gaps.length && player.walls && player.rock >= WALL_COST) return mine.walls++, { kind: ACT_WALL, x: gaps[0][0], y: gaps[0][1] };
    }
    // 3. Otherwise dig for rock and spongerock.
    return explorer();
  }

  function capper() {
    if (!player.walls || player.rock < WALL_COST) return explorer();
    // The top of a shaft: open air just below the surface, with solid sides.
    const tops = sample(40, 8, 160, (x, y) => openAir(x, y) && underground(x, y) && y <= world.surface[x] + 4 &&
      solid(x - 1, y) && solid(x + 1, y) && supported(x, y));
    if (tops.length) return mine.walls++, { kind: ACT_WALL, x: tops[0][0], y: tops[0][1] };
    if (Math.random() < 0.05) wander();
    return explorer();
  }

  function flooder() {
    if (!plan) {
      // A shaft somewhere inland of one shore, down below sea level, then a tunnel inland. Every flooder
      // (and every new attempt) picks its own spot, so they do not all queue up in one tunnel.
      const right = Math.random() < 0.5;
      const dir = right ? -1 : 1;
      let shore = right ? world.w - 1 : 0;
      while (world.surface[shore] >= world.seaLevel && shore !== world.w / 2) shore += dir;
      const x0 = shore + (4 + Math.floor(Math.random() * 160)) * dir;
      const row = world.seaLevel + 3 + Math.floor(Math.random() * 25);
      const cells = [];
      for (let y = world.surface[x0]; y <= row; y++) cells.push([x0, y]);
      for (let k = 1; k <= 250; k++) cells.push([x0 + k * dir, row]);
      plan = { dir, x0, row, cells, step: 0, breaching: false, stuck: 0 };
    }
    // Dig the next cell (scooping it if it is water). A cell that stays out of reach for about 3 seconds,
    // under a flood for example, means this tunnel is a lost cause: try somewhere else.
    const next = (c) => {
      if (reachable(...c)) {
        plan.stuck = 0;
        return dig(c);
      }
      if (++plan.stuck > 30) plan = null;
      return null;
    };
    if (!plan.breaching) {
      const done = ([x, y]) => type(x, y) === AIR && visible(x, y);
      while (plan.step < plan.cells.length && done(plan.cells[plan.step])) plan.step++;
      const target = plan.cells[plan.step];
      // Stop tunnelling at the end, or when someone else's open space is right ahead.
      const metOthers = target && plan.step > 10 && openAir(target[0] + plan.dir, target[1]);
      if (target && !metOthers) {
        [focusX, focusY] = target;
        return next(target);
      }
      plan.breaching = true;
      plan.breachX = plan.x0 - plan.dir;
    }
    // Breach: dig from the shaft bottom back towards the ocean until water pours in.
    const [bx, by] = [plan.breachX, plan.row];
    [focusX, focusY] = [bx, by];
    if (type(bx, by) === WATER || type(bx - plan.dir, by) === WATER) {
      mine.breaches++;
      plan = null; // flooded: start another tunnel
      return null;
    }
    if (type(bx, by) === AIR && visible(bx, by)) {
      plan.breachX -= plan.dir;
      return null;
    }
    return next([bx, by]);
  }

  const behaviours = { digger, explorer, builder, capper, flooder };

  function act() {
    if (!world || !player || ws.readyState !== WebSocket.OPEN || Date.now() < readyAt) return;
    subscribe();
    const a = (behaviours[role] ?? digger)();
    if (!a) return;
    const d = new DataView(new ArrayBuffer(11));
    d.setUint8(0, 2);
    d.setUint8(1, a.kind);
    d.setUint32(2, a.x, true);
    d.setUint32(6, a.y, true);
    d.setUint8(10, a.arg ?? 0);
    ws.send(d.buffer);
    last = a;
    stats.actions++;
    readyAt = Date.now() + 4000;
  }

  ws.onopen = () => stats.open++;
  ws.onclose = () => stats.closed++;
  ws.onmessage = (e) => {
    const d = new DataView(e.data);
    stats.bytes += d.byteLength;
    stats.messages++;
    let o = 1;
    const u8 = () => d.getUint8(o++);
    const u16 = () => ((o += 2), d.getUint16(o - 2, true));
    const u32 = () => ((o += 4), d.getUint32(o - 4, true));
    switch (d.getUint8(0)) {
      case 1: {
        // WELCOME, see server/src/protocol.rs.
        const w = u32(), h = u32();
        u16();
        const spawnX = u32(), spawnY = u32();
        u16();
        for (let k = 0; k < 4; k++) u16();
        u16();
        u16();
        const seaLevel = u16();
        const surface = new Uint16Array(w);
        for (let x = 0; x < w; x++) surface[x] = u16();
        world = { w, h, spawnX, spawnY, seaLevel, surface };
        // Diggers mostly crowd around the crater; the others start further out.
        const spread = role === "digger" ? (Math.random() < 0.8 ? 40 : 200) : 300;
        focusX = Math.max(0, Math.min(w - 1, spawnX + Math.round((Math.random() - 0.5) * 2 * spread)));
        focusY = surface[focusX];
        break;
      }
      case 2:
      case 3: {
        const isDiff = d.getUint8(0) === 3;
        const count = u16();
        for (let k = 0; k < count; k++) {
          const cx = u16(), cy = u16(), len = u16();
          const key = `${cx},${cy}`;
          if (!chunks.has(key)) chunks.set(key, new Uint8Array(CHUNK * CHUNK));
          const c = chunks.get(key);
          if (isDiff) {
            for (let j = 0; j < len; j++) c[u16()] = u8();
          } else {
            let p = 0;
            for (let j = 0; j < len; j += 2) {
              const v = u8(), runLength = u8();
              c.fill(v, p, p + runLength);
              p += runLength;
            }
          }
        }
        break;
      }
      case 4: {
        const result = u8(), left = u32();
        u32();
        player = { rock: u32(), water: u32(), sponge: u32(), quartz: u32(), pickaxe: u8(), walls: (u8() & 1) !== 0,
          amethyst: u32(), strike: u8() };
        // Spend on upgrades like a player would: walls for the roles that build, pickaxe levels for everyone.
        const builds = role === "builder" || role === "capper";
        if (builds && !player.walls && player.rock >= WALLS_UNLOCK_COST) ws.send(new Uint8Array([3, 0]));
        else if (player.pickaxe < 5 && player.quartz >= 2 ** (player.pickaxe + 1) - 1) ws.send(new Uint8Array([3, 1]));
        else if (player.amethyst >= [3, 10][player.strike]) ws.send(new Uint8Array([3, 2]));
        if (result === 1) {
          stats.ok++;
          mine.ok++;
          // Diggers follow their dig down, a little behind the deepest cell so the pit stays wide.
          if (role === "digger" && last?.kind === ACT_DIG && last.y - 3 > focusY) focusY = last.y - 3;
        }
        // After a failed action (someone else got there first, say) wait a moment before trying again.
        readyAt = Date.now() + left + (result === 1 ? 50 : 500);
        break;
      }
    }
  };
  return { ws, act };
}

const bots = [];
// Everyone arrives within two seconds, like viewers clicking a link on stream.
for (let n = 0; n < BOTS; n++) setTimeout(() => bots.push(bot(n)), Math.random() * 2000);

const timer = setInterval(() => bots.forEach((b) => b.act()), 100);
let lastReport = Date.now();
const report = setInterval(() => {
  const secs = (Date.now() - lastReport) / 1000;
  console.log(`${stats.open - stats.closed}/${BOTS} connected | ${(stats.bytes / 1024 / secs).toFixed(0)} KB/s received, ` +
    `${(stats.messages / secs).toFixed(0)} msg/s | ${stats.actions} actions sent, ${stats.ok} succeeded`);
  console.log("  " + Object.entries(roleStats).map(([r, s]) => [
    `${r} x${s.bots}: ${s.ok} ok`,
    s.walls && `${s.walls} walls`, s.repairs && `${s.repairs} repairs`, s.sponges && `${s.sponges} sponges`,
    s.breaches && `${s.breaches} breaches`,
  ].filter(Boolean).join(", ")).join(" | "));
  stats.bytes = stats.messages = stats.actions = stats.ok = 0;
  lastReport = Date.now();
}, 10000);

setTimeout(() => {
  clearInterval(timer);
  clearInterval(report);
  bots.forEach((b) => b.ws.close());
  setTimeout(() => process.exit(0), 500);
}, SECONDS * 1000);
