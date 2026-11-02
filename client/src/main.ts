import {
  ACT_DIG, ACT_PLACE, ACT_WALL, AIR, ActResult, BEDROCK, CHUNK, ITEM_ROCK, ITEM_RUBBLE, ITEM_SPONGE, ITEM_WATER, ITEM_QUARTZ, ITEM_AMETHYST, ITEM_CORAL, ITEM_EMBER, ITEM_LAVA, ITEM_OPAL, Net, REVEALED, RUBBLE,
  RUBBLE_FALLING, SHADOW, SPONGE, QUARTZ_ORE, UPGRADE_PICKAXE, UPGRADE_WALLS, SPRING_ROCK, SPRING, ROCK_CRACKED,
  AMETHYST_ORE, UPGRADE_STRIKE, KIND_NAMES, ROCK_ITEMS, ROCK_KINDS, rockState,
  SPONGE_ORE, TYPE_MASK, WALL, WALL_CRACKED, WALL_CRUMBLING, WATER, type PlayerState, type Welcome,
  recoveryCode, useRecoveryCode, CORAL_ORE, CORAL, EMBER, STEAM,
  MAGMA, MOLTEN, LAVA_ROCK, LAVA_ROCK_CHIPPED, FIRE_OPAL,
} from "./net";
import { World } from "./world";
import { Renderer } from "./render";
import { Camera, attachInput } from "./camera";

const MAX_SUBSCRIBED = 600;
const OUT_OF_REACH = "You can only reach cells next to open air";
const UNSUPPORTED = "In the open sky, it has to rest against something solid";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const canvas = $<HTMLCanvasElement>("view");

/** Placing tools for the kinds of rock, in ROCK_KINDS order ("rock" is stone). */
const KIND_TOOLS = ["dirt", "rock", "granite", "slate", "basalt", "darkstone"] as const;
type Tool = "dig" | (typeof KIND_TOOLS)[number] | "water" | "rubble" | "wall" | "sponge" | "quartz" | "amethyst" | "coral" | "ember" | "lava" | "opal";
/** The item each placing tool sends with ACT_PLACE (walls use ACT_WALL). */
const ITEMS: Partial<Record<Tool, number>> = {
  ...Object.fromEntries(KIND_TOOLS.map((t, k) => [t, ROCK_ITEMS[k]])), water: ITEM_WATER, rubble: ITEM_RUBBLE, sponge: ITEM_SPONGE,
  quartz: ITEM_QUARTZ, amethyst: ITEM_AMETHYST, coral: ITEM_CORAL, ember: ITEM_EMBER,
  lava: ITEM_LAVA, opal: ITEM_OPAL,
};
/** Stone per rubble block (server: sim.rs). */
const ROCK_COST = 1;
const kindOf = (t: Tool) => (KIND_TOOLS as readonly string[]).indexOf(t);
/** Upgrades (server: WALLS_UNLOCK_COST, PICKAXE_MAX, PICKAXE_STEP). Pickaxe level n to n + 1 costs 2^(n + 1) - 1 quartz. */
const WALLS_UNLOCK_COST = 10;
const PICKAXE_MAX = 5;
const PICKAXE_STEP = 0.1;
/** Amethyst per strike level (server: STRIKE_COSTS). */
const STRIKE_COSTS = [3, 10];
const isWall = (t: number) => t === WALL || t === WALL_CRACKED || t === WALL_CRUMBLING;
interface Action { kind: number; x: number; y: number; arg: number }

let world: World | null = null;
let welcome: Welcome | null = null;
let player: PlayerState | null = null;
let tool: Tool = "dig";
let cooldownEnd = 0;
let cooldownTotal = 1;
/** An action clicked during the cooldown; it goes out as soon as the cooldown is over. */
let queued: Action | null = null;
let lastSent: Action | null = null;
let hover: [number, number] | null = null;
let rain = 0;
/** How dark the sky should get: 0 dry up to 1 for heavy rain. */
let rainTarget = 0;
const RAIN_TEXT = ["", "light rain", "rain", "heavy rain!"];
const UPWELLING_TEXT = ["", "the deep is stirring…", "Upwelling!", "eruption!", "veins cooling"];
/** The Upwelling phase (see Stats); the view trembles when the deep starts stirring and when the volcano erupts. */
let upwelling = 0;
let eruptedAt = 0;
let stirredAt = 0;
const ERUPTION_SHAKE_MS = 2500;
const STIR_SHAKE_MS = 8000;
const RAIN_DARKNESS = [0, 0.45, 0.75, 1];
/** Current action speed from the server; cooldowns are divided by it. */
let speed = 1;
/** Deepest dig so far, from the server's stats. */
let recordDepth = 0;
let subscribedKey = "";
let lastSubscribe = 0;
let cameraPlaced = false;
/** Dev mode (?dev=KEY): the server skips cooldowns, costs and reach rules, and applies actions to a
 * brush x brush square. The key is checked by the server; without it these are just ordinary clicks. */
const DEV = new URLSearchParams(location.search).has("dev");
let brush = 1;

const cam = new Camera();
const renderer = new Renderer(canvas);

const net = new Net({
  welcome(w) {
    welcome = w;
    world = new World(w.width, w.height, w.surface, w.seaLevel);
    renderer.setWorld(world);
    subscribedKey = "";
    $("wall-cost").textContent = String(w.wallCost);
    if (DEV) net.brush(brush);
    selectTool(tool);
    if (!recalled(HINT_DONE)) $("hint").hidden = false;
    // The leaderboard starts open, unless this browser minimized it last time.
    toggleLeaders(!recalled(LEADERS_HIDDEN), false);
    if (!cameraPlaced) {
      cam.scale = Math.min(20, Math.max(4, canvas.clientWidth / 110));
      cam.centreOn(w.spawnX + 0.5, w.spawnY, canvas);
      cameraPlaced = true;
    }
  },
  chunk: (cx, cy, rle) => world?.setChunk(cx, cy, rle),
  diff: (cx, cy, d, o, n) => world?.applyDiff(cx, cy, d, o, n),
  player(p) {
    player = p;
    if (p.cooldownLeft > 0) {
      cooldownEnd = performance.now() + p.cooldownLeft;
      cooldownTotal = Math.max(p.cooldownTotal, 1);
    } else if (p.result === ActResult.Ok || p.result === ActResult.None) {
      cooldownEnd = 0;
    }
    if (p.result === ActResult.Cooldown && !queued) queued = lastSent;
    if (p.result === ActResult.Ok && lastSent?.kind === ACT_DIG && !$("hint").hidden) {
      $("hint").hidden = true;
      remember(HINT_DONE);
    }
    if (p.result === ActResult.NoResources) toast(poorMessage(tool));
    if (p.result === ActResult.NotVisible) toast("You can't see that cell yet");
    if (p.result === ActResult.WrongTarget) toast("Can't do that there");
    if (p.result === ActResult.OutOfReach) toast(OUT_OF_REACH);
    if (p.result === ActResult.Unsupported) toast(UNSUPPORTED);
    if (p.result === ActResult.Locked) toast("Unlock walls in Upgrades first");
    updateInventory();
  },
  stats(s) {
    $("online").textContent = `${s.online} online`;
    speed = s.speed || 1;
    $("speed").textContent = `speed ×${speed.toFixed(2).replace(/\.?0+$/, "")}`;
    // The record depth is not shown (the top bar stays small; new records are announced), but it still limits
    // how far down the camera can go.
    recordDepth = s.recordDepth;
    rainTarget = RAIN_DARKNESS[s.rain] ?? 1;
    $("rain").textContent = RAIN_TEXT[s.rain] ?? "rain";
    $("rain").hidden = s.rain === 0;
    if (s.upwelling === 3 && upwelling !== 3) eruptedAt = performance.now();
    if (s.upwelling === 1 && upwelling !== 1) stirredAt = performance.now();
    upwelling = s.upwelling;
    $("upwelling").textContent = UPWELLING_TEXT[s.upwelling] ?? "Upwelling!";
    $("upwelling").hidden = s.upwelling === 0;
  },
  announce: (text) => notice(text),
  leaders(l) {
    const row = (rank: number, name: string, mined: number, me: boolean) => {
      const li = document.createElement("li");
      li.className = me ? "me" : "";
      for (const [cls, text] of [["rank", `${rank}.`], ["n", name], ["", String(mined)]]) {
        const span = document.createElement("span");
        span.className = cls;
        span.textContent = text;
        li.append(span);
      }
      return li;
    };
    $("leaders-list").replaceChildren(...l.top.map((p, k) => row(k + 1, p.name, p.mined, p.name === l.name)));
    if (l.top.length === 0) $("leaders-list").textContent = "Nobody has dug yet.";
    // Your own line, unless you are in the top list already.
    const inTop = l.top.some((p) => p.name === l.name);
    $("leaders-me").replaceChildren(...(inTop ? [] : row(l.rank, `${l.name} (you)`, l.mined, true).childNodes));
    $("leaders-me").hidden = inTop;
  },
  connected(open) {
    $("overlay").hidden = open;
    $("overlay").textContent = "Connection lost, reconnecting…";
    if (!open) queued = null;
  },
});

/** The first-dig hint is shown until a player's first dig, once per browser. */
const HINT_DONE = "mt-dug";
/** Set while the player keeps the leaderboard minimized. */
const LEADERS_HIDDEN = "mt-leaders-hidden";
/** Set while the player has carved (rounded) rock edges switched on with the E key. */
const CARVED = "mt-carved";

function recalled(key: string): boolean {
  try {
    return localStorage.getItem(key) !== null;
  } catch {
    return false;
  }
}

function remember(key: string, on = true) {
  try {
    if (on) localStorage.setItem(key, "1");
    else localStorage.removeItem(key);
  } catch {
    // Private windows may refuse; the hint then shows again next time.
  }
}

// ---- Actions -------------------------------------------------------------------------------------

function cellName(v: number): string {
  if ((v & (REVEALED | SHADOW)) === 0) return "Unknown";
  const t = v & TYPE_MASK;
  if (t === AIR) return "Air";
  const rock = rockState(t);
  if (rock) {
    const name = KIND_NAMES[rock.kind];
    const hits = `${rock.left} ${rock.left === 1 ? "hit" : "hits"}`;
    if (ROCK_KINDS.includes(t)) return rock.left > 1 ? `${name} (${hits})` : name;
    return `Chipped ${name.toLowerCase()} (${hits} left)`;
  }
  if (t === AMETHYST_ORE) return "Amethyst";
  if (t === WATER) return "Water";
  if (t === WALL) return "Wall";
  if (t === WALL_CRACKED) return "Cracked wall";
  if (t === WALL_CRUMBLING) return "Crumbling wall";
  if (t === SPONGE_ORE) return "Spongerock ore";
  if (t === SPONGE) return "Spongerock";
  if (t === RUBBLE || t === RUBBLE_FALLING) return "Rubble";
  if (t === BEDROCK) return "Bedrock";
  if (t === QUARTZ_ORE) return "Quartz";
  if (t === SPRING_ROCK) return "Damp rock";
  if (t === SPRING) return "Spring";
  if (t === ROCK_CRACKED) return "Cracked ceiling";
  if (t === CORAL_ORE) return "Swellcoral reef";
  if (t === CORAL) return "Swellcoral";
  if (t === EMBER) return "Emberstone";
  if (t === STEAM) return "Steam";
  if (t === MAGMA) return "Magma";
  if (t === MOLTEN) return "Molten vein";
  if (t === LAVA_ROCK) return "Lava rock (2 hits)";
  if (t === LAVA_ROCK_CHIPPED) return "Chipped lava rock (1 hit left)";
  if (t === FIRE_OPAL) return "Fire opal";
  return "Unknown";
}

/** Visible air: hidden cells also read as 0, so check the revealed bit too. */
function isOpenAir(x: number, y: number): boolean {
  const v = world!.get(x, y);
  return v === REVEALED || v === (REVEALED | STEAM);
}

/** Same rule as the server: a cell can be acted on only if it touches visible air. */
function reachable(x: number, y: number): boolean {
  const v = world!.get(x, y);
  if ((v & REVEALED) === 0) return false;
  return (v & TYPE_MASK) === AIR || (v & TYPE_MASK) === STEAM || isOpenAir(x + 1, y) || isOpenAir(x - 1, y) || isOpenAir(x, y + 1) ||
    isOpenAir(x, y - 1);
}

function touchesWall(x: number, y: number): boolean {
  const wallAt = (nx: number, ny: number) => (world!.get(nx, ny) & REVEALED) !== 0 && isWall(world!.get(nx, ny) & TYPE_MASK);
  return wallAt(x + 1, y) || wallAt(x - 1, y) || wallAt(x, y + 1) || wallAt(x, y - 1);
}

/** Same rule as the server: in the open sky (above the surface) a block must touch something solid. */
function supported(x: number, y: number): boolean {
  if (y >= world!.surface[x]) return true;
  const solid = (nx: number, ny: number) => {
    const v = world!.get(nx, ny);
    return (v & REVEALED) !== 0 && ![AIR, WATER, STEAM, MAGMA, RUBBLE_FALLING].includes(v & TYPE_MASK);
  };
  return solid(x + 1, y) || solid(x - 1, y) || solid(x, y + 1) || solid(x, y - 1);
}

/** The action the current tool does on a cell, or null when it can't act there. */
function actionAt(x: number, y: number): Action | null {
  if (!world) return null;
  if (DEV) {
    // The server decides what each cell of the brush does; anything inside the world goes.
    if (x < 0 || y < 0 || x >= world.width || y >= world.height) return null;
    if (tool === "dig") return { kind: ACT_DIG, x, y, arg: 0 };
    if (tool === "wall") return { kind: ACT_WALL, x, y, arg: 0 };
    return { kind: ACT_PLACE, x, y, arg: ITEMS[tool]! };
  }
  if (tool === "wall" && !player?.wallsUnlocked) return null;
  const t = world.get(x, y) & TYPE_MASK;
  // Walls can also be built on from an existing wall, into open cells next to it (same rule as the server).
  const wallReach = tool === "wall" && (world.get(x, y) & REVEALED) !== 0 && touchesWall(x, y) &&
    (t === AIR || t === WATER || isWall(t));
  if (!reachable(x, y) && !wallReach) return null;
  if (tool === "dig") return [AIR, STEAM, BEDROCK, SPRING, MAGMA, MOLTEN].includes(t) ? null : { kind: ACT_DIG, x, y, arg: 0 };
  // The wall tool on a cracked wall repairs it.
  if (tool === "wall" && (t === WALL_CRACKED || t === WALL_CRUMBLING)) return { kind: ACT_WALL, x, y, arg: 0 };
  if (t !== AIR && t !== WATER) return null;
  // Water goes into air only. Water and rubble need no support: they fall until they land.
  if (tool === "water") return t === AIR ? { kind: ACT_PLACE, x, y, arg: ITEM_WATER } : null;
  if (tool === "rubble") return { kind: ACT_PLACE, x, y, arg: ITEM_RUBBLE };
  if (!supported(x, y)) return null;
  if (tool === "wall") return { kind: ACT_WALL, x, y, arg: 0 };
  return { kind: ACT_PLACE, x, y, arg: ITEMS[tool]! };
}

/** Whether the player has what the tool costs (the wall tool's repair cost is checked when clicking). */
function canAfford(t: Tool): boolean {
  if (!player || DEV) return true;
  if (kindOf(t) >= 0) return player.kinds[kindOf(t)] > 0;
  if (t === "rubble") return player.rock >= ROCK_COST;
  if (t === "water") return player.water >= 1;
  if (t === "wall") return player.rock >= (welcome?.wallCost ?? 3);
  if (t === "sponge") return player.sponge > 0;
  if (t === "quartz") return player.quartz > 0;
  if (t === "amethyst") return player.amethyst > 0;
  if (t === "coral") return player.coral > 0;
  if (t === "ember") return player.ember > 0;
  if (t === "lava") return player.lava > 0;
  if (t === "opal") return player.opal > 0;
  return true;
}

function poorMessage(t: Tool): string {
  if (t === "water") return "No water left";
  if (t === "sponge") return "No spongerock left";
  if (t === "quartz") return "No quartz left";
  if (t === "amethyst") return "No amethyst left";
  if (t === "coral") return "No swellcoral left";
  if (t === "ember") return "No emberstone left";
  if (t === "lava") return "No lava rock left";
  if (t === "opal") return "No fire opal left";
  if (kindOf(t) >= 0) return `No ${KIND_NAMES[kindOf(t)].toLowerCase()} left`;
  return "Not enough stone";
}

/** What the dig tool would do on the hovered cell, for its button and the cell info. */
function digVerb(): { label: string; icon: string; hint: string } {
  const plain = { label: "Dig", icon: "⛏", hint: "" };
  if (tool !== "dig" || !hover || !world || !actionAt(hover[0], hover[1])) return plain;
  const t = world.get(hover[0], hover[1]) & TYPE_MASK;
  if (t === WATER) return { label: "Scoop", icon: "🪣", hint: "click to scoop it up" };
  if (t === WALL || t === WALL_CRACKED || t === WALL_CRUMBLING) return { ...plain, label: "Break", hint: "click to crack it" };
  return { ...plain, hint: "click to dig" };
}

function send(a: Action) {
  net.action(a.kind, a.x, a.y, a.arg);
  lastSent = a;
  if (DEV) return;
  // Assume the action works; the server's answer corrects the cooldown if not.
  const c = welcome!.cooldowns;
  const t = world!.get(a.x, a.y) & TYPE_MASK;
  const repair = t === WALL_CRACKED || t === WALL_CRUMBLING;
  const place = a.kind === ACT_PLACE ? a.arg : -1;
  cooldownTotal = a.kind === ACT_WALL ? (repair ? c.dig : c.wall)
    : place === ITEM_RUBBLE || place === ITEM_ROCK ? c.dig
    : place === ITEM_WATER ? c.collect
    : [ITEM_SPONGE, ITEM_QUARTZ, ITEM_AMETHYST, ITEM_CORAL, ITEM_EMBER, ITEM_LAVA, ITEM_OPAL].includes(place) ? c.place
    : t === WATER ? c.collect : c.dig;
  cooldownTotal /= speed;
  if (a.kind === ACT_DIG && t !== WATER) cooldownTotal /= 1 + (player?.pickaxe ?? 0) * PICKAXE_STEP;
  cooldownEnd = performance.now() + cooldownTotal;
}

function cellAtScreen(sx: number, sy: number): [number, number] {
  const [wx, wy] = cam.toWorld(sx, sy);
  return [Math.floor(wx), Math.floor(wy)];
}

attachInput(canvas, cam, {
  tap(sx, sy) {
    const [x, y] = cellAtScreen(sx, sy);
    const a = actionAt(x, y);
    if (!a) {
      if (!world) return;
      if ((world.get(x, y) & REVEALED) === 0) toast("You can't see that cell yet");
      else if ((world.get(x, y) & TYPE_MASK) === BEDROCK) toast("Bedrock is too hard to dig");
      else if ((world.get(x, y) & TYPE_MASK) === SPRING) toast("A spring can't be dug out: plug its sides");
      else if (!reachable(x, y)) toast(OUT_OF_REACH);
      else if (tool !== "dig" && tool !== "rubble" && tool !== "water" && !supported(x, y)) toast(UNSUPPORTED);
      return;
    }
    if (DEV) return send(a);
    const t = world!.get(x, y) & TYPE_MASK;
    const repair = tool === "wall" && (t === WALL_CRACKED || t === WALL_CRUMBLING);
    if (repair ? (player?.rock ?? 0) < welcome!.repairCost : !canAfford(tool)) return toast(poorMessage(tool));
    if (performance.now() < cooldownEnd || !net.open) queued = a;
    else send(a);
  },
  hover(sx, sy) {
    hover = sx === null || sy === null ? null : cellAtScreen(sx, sy);
  },
});

// ---- HUD -----------------------------------------------------------------------------------------

let toastTimer = 0;
function toast(text: string) {
  const el = $("toast");
  el.textContent = text;
  el.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => el.classList.remove("show"), 1400);
}

/** Good news (a new material, an unlock): a gold card in the bottom-right corner that stays long enough
 * to read; clicking it closes it. Several notices stack. */
function notice(text: string) {
  const el = document.createElement("div");
  el.className = "notice";
  el.textContent = text;
  el.onclick = () => el.remove();
  $("notices").append(el);
  setTimeout(() => el.classList.add("fade"), 7000);
  setTimeout(() => el.remove(), 7600);
}

function selectTool(t: Tool) {
  if (t === "sponge" && $("sponge-tool").hidden) return;
  if (t === "quartz" && $("quartz-tool").hidden) return;
  if (t === "amethyst" && $("amethyst-tool").hidden) return;
  if (["coral", "ember", "lava", "opal"].includes(t) && $(`${t}-tool`).hidden) return;
  if (t === "wall" && $("wall-tool").hidden) return;
  tool = t;
  queued = null;
  document.querySelectorAll<HTMLElement>(".tool").forEach((b) => b.classList.toggle("selected", b.dataset.tool === t));
}

function updateInventory() {
  if (!player) return;
  KIND_TOOLS.forEach((t, k) => {
    $(t).textContent = String(player!.kinds[k]);
    // The deeper kinds show up in the toolbar once you have found some.
    const slot = document.querySelector<HTMLElement>(`.slot[data-tool="${t}"]`)!;
    if (player!.kinds[k] > 0) slot.hidden = false;
  });
  $("water").textContent = String(player.water);
  $("sponge").textContent = String(player.sponge);
  // Spongerock shows up in the toolbar once you have found some.
  if (player.sponge > 0 && $("sponge-tool").hidden) {
    $("sponge-tool").hidden = false;
    notice("You found spongerock!");
  }
  // Slots you cannot pay for right now are dimmed, their price in red.
  document.querySelectorAll<HTMLElement>(".slot").forEach((b) => b.classList.toggle("poor", !canAfford(b.dataset.tool as Tool)));
  $("quartz").textContent = String(player.quartz);
  $("amethyst").textContent = String(player.amethyst);
  const knowsAmethyst = player.amethyst > 0 || player.strike > 0;
  if (knowsAmethyst && $("amethyst-tool").hidden) {
    $("amethyst-tool").hidden = false;
    notice("You found amethyst!");
  }
  const knowsQuartz = player.quartz > 0 || player.pickaxe > 0;
  if (knowsQuartz && $("quartz-tool").hidden) {
    $("quartz-tool").hidden = false;
    notice("You found quartz!");
  }
  if (player.wallsUnlocked && $("wall-tool").hidden) $("wall-tool").hidden = false;
  $("coral").textContent = String(player.coral);
  if (player.coral > 0 && $("coral-tool").hidden) {
    $("coral-tool").hidden = false;
    notice("You found swellcoral!");
  }
  $("ember").textContent = String(player.ember);
  if (player.ember > 0 && $("ember-tool").hidden) {
    $("ember-tool").hidden = false;
    notice("You found emberstone!");
  }
  $("lava").textContent = String(player.lava);
  if (player.lava > 0) $("lava-tool").hidden = false;
  $("opal").textContent = String(player.opal);
  if (player.opal > 0 && $("opal-tool").hidden) {
    $("opal-tool").hidden = false;
    notice("You found a fire opal!");
  }
  // The Upgrades button appears once there is something to buy, and then stays.
  const canShop = player.rock >= WALLS_UNLOCK_COST || player.wallsUnlocked || knowsQuartz || knowsAmethyst;
  if (canShop && $("shop-button").hidden) {
    $("shop-button").hidden = false;
    if (!knowsQuartz && !player.wallsUnlocked) notice("10 stone: you can unlock walls in Upgrades");
  }
  updateShop();
}

/** A price as a number and the material's swatch, like the hotbar badges. */
function price(amount: number, material: "stone" | "quartz" | "amethyst"): Node[] {
  const swatch = document.createElement("span");
  swatch.className = `swatch ${material === "stone" ? "rock" : material}-swatch`;
  swatch.title = material;
  return [document.createTextNode(`${amount} `), swatch];
}

function updateShop() {
  if (!player) return;
  const walls = $<HTMLButtonElement>("buy-walls");
  if (player.wallsUnlocked) walls.textContent = "Unlocked ✓";
  else walls.replaceChildren(...price(WALLS_UNLOCK_COST, "stone"));
  walls.disabled = player.wallsUnlocked || player.rock < WALLS_UNLOCK_COST;

  $("offer-pickaxe").hidden = player.quartz === 0 && player.pickaxe === 0;
  const level = player.pickaxe, cost = 2 ** (level + 1) - 1;
  const pick = $<HTMLButtonElement>("buy-pickaxe");
  $("pickaxe-name").textContent = `Pickaxe level ${level}`;
  $("pickaxe-text").textContent = level >= PICKAXE_MAX
    ? `Digs ×${(1 + level * PICKAXE_STEP).toFixed(1)} as fast. Fully upgraded.`
    : `Digs ×${(1 + level * PICKAXE_STEP).toFixed(1)} as fast; next level ×${(1 + (level + 1) * PICKAXE_STEP).toFixed(1)}.`;
  if (level >= PICKAXE_MAX) pick.textContent = "Max level";
  else pick.replaceChildren(...price(cost, "quartz"));
  pick.disabled = level >= PICKAXE_MAX || player.quartz < cost;

  $("offer-strike").hidden = player.amethyst === 0 && player.strike === 0;
  const strike = $<HTMLButtonElement>("buy-strike"), next = STRIKE_COSTS[player.strike];
  $("strike-name").textContent = `Heavy strikes level ${player.strike}`;
  $("strike-text").textContent = next === undefined
    ? `Every hit does ${player.strike + 1} damage. Fully upgraded.`
    : `Every hit does ${player.strike + 1} damage; next level ${player.strike + 2}.`;
  if (next === undefined) strike.textContent = "Max level";
  else strike.replaceChildren(...price(next, "amethyst"));
  strike.disabled = next === undefined || player.amethyst < next;
}

/** The leaderboard asks the server again every few seconds while it is open. */
let leadersTimer = 0;
function toggleLeaders(open = $("leaders").hidden, save = true) {
  $("leaders").hidden = !open;
  if (save) remember(LEADERS_HIDDEN, !open);
  clearInterval(leadersTimer);
  if (open) {
    net.leaders();
    leadersTimer = window.setInterval(() => net.leaders(), 5000);
  }
}
$("leaders-button").onclick = () => toggleLeaders();

function toggleShop(open = $("shop").hidden) {
  if ($("shop-button").hidden) return;
  $("shop").hidden = !open;
}

$("shop-button").onclick = () => toggleShop();

/** The menu (Esc): the recovery code to take progress to another browser, and a field to enter one. */
function toggleMenu(open = $("menu").hidden) {
  $("menu").hidden = !open;
  if (!open) return;
  showCode(false);
  $<HTMLInputElement>("code-enter").value = "";
  $("code-error").hidden = true;
  $("code-copy").textContent = "Copy";
}
/** The recovery code is hidden unless asked for, so it does not end up on a stream or a screenshot. */
function showCode(show: boolean) {
  const mine = $<HTMLInputElement>("code-mine");
  mine.value = show ? recoveryCode() : recoveryCode().replace(/[0-9a-f]/g, "•");
  mine.dataset.shown = show ? "1" : "";
  $("code-show").textContent = show ? "Hide" : "Show";
}
$("menu-button").onclick = () => toggleMenu();
$("code-show").onclick = () => showCode(!$("code-mine").dataset.shown);
$("menu-close").onclick = () => toggleMenu(false);
$("code-copy").onclick = () => {
  navigator.clipboard?.writeText(recoveryCode()).then(() => ($("code-copy").textContent = "Copied"), () => {});
};
const useCode = () => {
  const code = $<HTMLInputElement>("code-enter").value;
  if (code.trim() === "") return;
  if (useRecoveryCode(code)) location.reload();
  else $("code-error").hidden = false;
};
$("code-use").onclick = useCode;
$("code-enter").onkeydown = (e) => {
  if (e.key === "Enter") useCode();
};
$("shop-close").onclick = () => toggleShop(false);
$("buy-walls").onclick = () => {
  net.buy(UPGRADE_WALLS);
  notice("Walls unlocked! The Wall tool is in your toolbar");
};
$("buy-pickaxe").onclick = () => {
  net.buy(UPGRADE_PICKAXE);
  notice("Pickaxe upgraded!");
};
$("buy-strike").onclick = () => {
  net.buy(UPGRADE_STRIKE);
  notice("Your strikes got heavier!");
};

document.querySelectorAll<HTMLElement>(".tool").forEach((b) => (b.onclick = () => selectTool(b.dataset.tool as Tool)));

function setBrush(size: number) {
  brush = Math.max(1, Math.min(41, size));
  $<HTMLInputElement>("brush").value = String(brush);
  $("brush-size").textContent = `${brush}×${brush}`;
  net.brush(brush);
}

if (DEV) {
  $("dev").hidden = false;
  $("wall-tool").hidden = false;
  $("sponge-tool").hidden = false;
  $("quartz-tool").hidden = false;
  $("amethyst-tool").hidden = false;
  $("coral-tool").hidden = false;
  $("ember-tool").hidden = false;
  $("lava-tool").hidden = false;
  $("opal-tool").hidden = false;
  $<HTMLInputElement>("brush").oninput = (e) => setBrush(Number((e.target as HTMLInputElement).value));
}

function goHome() {
  if (welcome) cam.centreOn(welcome.spawnX + 0.5, welcome.spawnY, canvas);
}

/** Carved rock edges (rounded corners, light on top edges): a look still being tried out, off by default. */
let carved = recalled(CARVED);

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") return toggleMenu();
  // Typing a recovery code is not a shortcut.
  if (e.target instanceof HTMLInputElement && e.target.type !== "range") return;
  const keys: Record<string, Tool> = { "1": "dig", "2": "dirt", "3": "rock", "4": "water", "5": "rubble", "6": "wall", "7": "sponge", "8": "quartz", "9": "amethyst" };
  if (keys[e.key]) selectTool(keys[e.key]);
  else if (e.key === "h" || e.key === "H") goHome();
  else if (e.key === "u" || e.key === "U") toggleShop();
  else if (e.key === "l" || e.key === "L") toggleLeaders();
  else if (e.key === "e" || e.key === "E") {
    carved = !carved;
    remember(CARVED, carved);
  }
  else if (DEV && (e.key === "[" || e.key === "]")) setBrush(brush + (e.key === "]" ? 2 : -2));
});

// ---- Chunk subscription --------------------------------------------------------------------------

/** Ask the server for the chunks on screen plus a margin of one chunk. */
function updateSubscription(now: number) {
  if (!world || !net.open || now - lastSubscribe < 150) return;
  const [x0, y0] = cam.toWorld(0, 0);
  const [x1, y1] = cam.toWorld(canvas.clientWidth, canvas.clientHeight);
  const cx0 = Math.max(0, Math.floor(x0 / CHUNK) - 1), cx1 = Math.min(world.chunksWide - 1, Math.floor(x1 / CHUNK) + 1);
  const cy0 = Math.max(0, Math.floor(y0 / CHUNK) - 1), cy1 = Math.min(world.chunksHigh - 1, Math.floor(y1 / CHUNK) + 1);
  const key = `${cx0},${cy0},${cx1},${cy1}`;
  if (key === subscribedKey) return;

  let chunks: [number, number][] = [];
  for (let cy = cy0; cy <= cy1; cy++) for (let cx = cx0; cx <= cx1; cx++) chunks.push([cx, cy]);
  if (chunks.length > MAX_SUBSCRIBED) {
    const mx = (cx0 + cx1) / 2, my = (cy0 + cy1) / 2;
    chunks.sort((a, b) => Math.hypot(a[0] - mx, a[1] - my) - Math.hypot(b[0] - mx, b[1] - my));
    chunks = chunks.slice(0, MAX_SUBSCRIBED);
  }
  net.subscribe(chunks);
  subscribedKey = key;
  lastSubscribe = now;
}

// ---- Frame loop ----------------------------------------------------------------------------------

function frame(now: number) {
  requestAnimationFrame(frame);
  if (!world) return;
  // How far down the camera goes depends on how deep players have dug, never on the world's real depth,
  // so scrolling until it stops does not reveal where the bottom is.
  cam.clamp(canvas, world.width, world.seaLevel + recordDepth + 400);
  updateSubscription(now);

  if (queued && now >= cooldownEnd && net.open) {
    const a = queued;
    queued = null;
    if (actionAt(a.x, a.y)) send(a);
  }

  const left = Math.max(0, cooldownEnd - now);
  $("cooldown-fill").style.width = `${(left / cooldownTotal) * 100}%`;

  let info = "";
  if (hover) {
    const [x, y] = hover;
    // No limit downwards: below the world's bottom it reads "Unknown" like any unexplored cell, so the info box
    // does not give away how deep the world goes.
    if (x >= 0 && y >= 0 && x < world.width) {
      const depth = y - world.surface[x];
      info = `${cellName(world.get(x, y))} · ${depth > 0 ? `depth ${depth} m` : "surface"}`;
    }
  }
  // The dig tool also scoops water: say so while the pointer is over water it can reach.
  const verb = digVerb();
  if ($("dig-label").textContent !== verb.label) {
    $("dig-label").textContent = verb.label;
    $("dig-icon").textContent = verb.icon;
  }
  if (info && verb.hint) info += ` · ${verb.hint}`;
  const infoEl = $("cell-info");
  if (infoEl.textContent !== info) infoEl.textContent = info;

  rain += (rainTarget - rain) * 0.02;
  const ok = hover !== null && actionAt(hover[0], hover[1]) !== null;
  // A slight tremble, a fraction of a screen pixel to two, so it is felt rather than seen.
  const erupting = upwelling === 3 && now - eruptedAt < ERUPTION_SHAKE_MS;
  const stirring = upwelling === 1 && now - stirredAt < STIR_SHAKE_MS;
  const shake = stirring || erupting ? 1.5 / cam.scale : 0;
  const [dx, dy] = [shake * Math.sin(now / 23), shake * Math.sin(now / 31)];
  cam.x += dx;
  cam.y += dy;
  renderer.draw(world, cam, now / 1000, {
    hover,
    hoverColour: ok ? [1, 1, 1] : [1, 0.35, 0.35],
    queued: queued ? [queued.x, queued.y] : null,
    rain,
    brush: DEV ? brush : 1,
    carved,
  });
  cam.x -= dx;
  cam.y -= dy;
}
requestAnimationFrame(frame);
