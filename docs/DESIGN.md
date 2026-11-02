# Mine Together - design

Status: first prototype built (server, client, bot load test), not yet playtested (2026-11-01).

## 1. Concept

A shared pixel world seen from the side. Thousands of players look at the same world in the browser, like r/place, and
each player can act on one cell every few seconds. Together they dig down as far as possible.

### Visibility

- Visible: every cell next to air (4-neighbourhood), plus the cells that are already open. Sight goes only 4 cells deep
  into water: a lake bed under 3 cells of water is visible, deeper parts stay hidden until the water is gone.
- Reach: players can act only on visible cells that touch open air (decided 2026-11-01). Only the top layer of a lake can
  be scooped; the lake bed and cells under water are out of reach.
- Shadow: cells 1-2 steps behind a visible cell are drawn darkened; their type is known but dimmed.
- Hidden: everything else is black. The server does not send hidden cells.

### Cell types

| Cell | Behaviour | Player action |
|---|---|---|
| Air | empty | - |
| Rock | static | mine: removes it, +1 rock; place on an air or water cell (1 rock, dig cooldown; needs support in the open sky) |
| Water | falls, spreads sideways every tick | collect: removes it, +1 water; pour out on an air cell (1 water, collect cooldown) |
| Wall | static, blocks water; intact, cracked, crumbling | build on an air or water cell (3 rock); dig cracks it one stage, the third dig breaks it (+1 rock); the wall tool repairs a cracked wall (1 rock) |
| Spongestone ore | static, in veins from 15 below the surface | mine: removes it, +1 spongestone |
| Spongestone | placed block; soaks up one neighbouring water cell every 2 s | place on an air or water cell (1 spongestone); mine: +1 spongestone |
| Rubble | falls 2 cells per tick through air, sinks 1 cell every other tick through water (swapping places with it, so no water is lost), slides off heaps, now and then sticks on a slope; solid once it rests, falls again when undermined | drop on an air or water cell, anywhere (1 rock, dig cooldown); mine: +1 rock |
| Bedrock | static, unbreakable; the island's side walls and the bottom | none ("too hard to dig") |

**Materials by depth** (decided 2026-11-01): instead of gems, new materials with a real use are introduced one at a
time, each first found at a certain depth, so digging deeper means discovering something new. Gems (and gems as paint)
were removed for now. A material's toolbar slot appears once the player has mined some.

1. **Spongestone** (from 15 below the surface, so a lone digger finds some soon): porous yellow-ochre rock. Inert in the ground (otherwise pockets next to
   it would silently drain); mined and placed as a block, it soaks up one neighbouring water cell every 2 seconds. The
   flood fixer. About one vein per 3150 cells, radius 1.5 to 4.55.
2. **Quartz** (from 30 below the surface): the lowest tier of upgrade crystals (higher tiers, up to diamond, may follow
   deeper). Pale pink crystal clusters: a plus-shaped core with 4 to 6 straight or diagonal spikes of 2 to 4 cells, unlike
   spongestone's round blobs; about one per 7000 cells. Mined: +1 quartz, spent on pickaxe upgrades.

**Springs** (decided 2026-11-02): damp rock (wet-looking, from 30 below the surface, about one cell per 35,000) opens
a spring when dug; a few more springs lie hidden at the bottom of water pockets. A spring cannot be dug; it pushes out
a water cell every 0.3 s into open air next to it, or through the water around it up to 12 rows above itself, until
every side is plugged (walls, rubble, spongestone). A flood with a clear target the crowd can fix. Like rain, it scales
with the players online: 20% of that alone (one cell every 1.5 s, slower than one player scoops), 60% at 10 players,
full strength from 100.

**Cave-ins** (decided 2026-11-02): a ceiling cell is unstable when the open space under it reaches 15 cells or more to
both sides, so rooms up to 30 wide hold. After a dig nearby, an unstable ceiling cracks after 3-10 s (visible warning)
and falls as rubble 5-15 s later if nothing was built underneath; each fall checks the ceiling above, so wide caverns
crumble into arches. Walls, rubble and rock under a ceiling hold it: pillars matter. Pending checks are not saved.

**Upgrades** (decided 2026-11-01): an Upgrades panel (key U) appears once a player has 10 rock or has found quartz.
- Walls are locked for new players; unlocking them costs 10 rock. Rubble is always available.
- Pickaxe levels 1 to 5: level n digs (1 + 0.1 n) times as fast (not scooping or building); level n to n + 1 costs
  n + 1 quartz (15 quartz in all for x1.5).

**Walls take three hits** (decided 2026-11-01): griefers can break walls, but defenders can keep up. Each dig cracks a
wall one stage (intact, cracked, crumbling, gone); the wall tool repairs a cracked wall for 1 rock with the dig cooldown,
so one defender holds a wall against one attacker, but not against a group.

### Water

- Pockets of water are generated inside the rock and released when a player digs into them.
- Rain events add water at the surface.
- Players can collect water cell by cell.
- Water pockets are finite, so a flood ends and can be cleared; the world must never get stuck in a state players
  cannot fix.

### Actions and cooldowns

- Mine / collect water: about 1-3 seconds.
- Build or place: longer (exact value to tune).
- The server validates every action: cell visible and of the right type, cooldown over, enough resources.

### Intended moments

- **Flood panic:** someone breaks into a water pocket or breaks a wall, water pours into the dig, players rush to wall it
  off and scoop it out.
- **Discovery:** the crowd reaches a new material's depth, players swarm it, dig carelessly, and floods follow.
- **Wall fights:** griefers hammer at a wall holding back water while defenders repair it.

### Prototype values (to tune after playtests)

- Cooldown: dig 1.5 s, collect water 2.5 s (slower than digging, so floods last), place spongestone 3 s, build a wall 4 s
  (repairing one takes the dig cooldown, so does dropping rubble).
- These cooldowns are for 100 players. Action speed scales with the players online (decided 2026-11-01):
  speed = 2 - 0.5 * log10(players), clamped to 0.5-2, and cooldowns are divided by it: 1 player x2, 10 players x1.5,
  100 players x1, 300 players x0.76, 1000 or more x0.5. Several tabs of one guest count once. The HUD shows the speed.
  Water, rain and the ocean are not affected. One cooldown per player, shared by all actions and
  by all tabs of the same guest.
- A wall costs 3 rock and takes three digs to break, giving 1 rock back; repairing a cracked wall costs 1 rock.
- In the open sky (above the original surface) a wall or block must touch something solid: no floating blocks or pictures
  in the sky, but towers and bridges built out from the ground are fine. Underground anything goes.
- Shadow: cells within 2 steps of a revealed cell (a 5x5 square without its corners).
- Only open space (air and water) connected to the sky or the ocean is lit (decided 2026-11-01). A tunnel sealed off
  with a wall or a block goes dark: everything inside is hidden and out of reach until someone breaks the seal. So
  nobody can hide a covered bunker from the rain. When a wall closes a space, the server searches upwards for the sky
  (giving up and assuming "connected" after 300,000 cells).
- Sight is calculated from the world as it is now (decided 2026-11-01), not from what was ever seen: when a hole floods,
  the cells deep under the water go dark again and the server stops sending them. Visible: lit air, water up to 4 cells
  from lit air, solid cells touching air or touching water less than 4 deep. Shadow: up to 2 cells behind visible air or
  rock; water only gets shadow next to rock that touches air, so a water pocket behind a thin wall shows up dimmed as a
  warning while deep water stays black. Every change marks the chunks within 7 cells for a sight update at the end of
  the tick.
- Water: falls up to 2 cells per tick, slides diagonally only if the side cell is free too (diagonal walls hold
  water), flows sideways up to 2 cells per tick towards a drop within 64 cells, otherwise spreads only when pushed by
  water above. A settled pool stops moving and its chunk sleeps.
- Water pressure (like Dwarf Fortress): when the top cell of a water column cannot move, it searches the connected water
  (up to 400 cells, within 32 cells) for an air cell lower than itself and jumps there, on every other tick only. Connected shafts fill up to the
  same level and trapped air bubbles get filled; water never rises above its source level.
- Measured: a flood through three world-wide tunnels in a 1024 x 1024 world costs 2.4 ms per tick on average (water,
  sight and seas), 20 ms at worst (budget 100 ms), and settles after about 40 seconds.
- Rain: a 20 second shower every 10-20 minutes, or on demand with the server console command `rain [light|normal|heavy]`.
  Strength is random: light (30%), normal (50%) or heavy (20%), at 0.7, 1 and 2 drops per tick per 128 columns (about
  12, 17 and 34 on the 2048 wide world). The HUD shows the strength and the sky darkens with it.
- Rain scales with the players online (decided 2026-11-02), so a handful of players is not drowned: normal rain needs
  10 players, heavy rain 50 (the console command can still force any strength), and only part of a shower falls
  in small groups: 40% alone, 70% at 10 players, all of it from 100 players.
- Evaporation (decided 2026-11-02): between showers, rain water lying on the land (above the original surface line, where
  the sky shows behind it) dries up, the top cell of each column with a 2% chance per second, so a flooded island
  drains within a few minutes. Lakes, the sea and water in dug-out space (grey back wall) never evaporate.
- Water pockets: fewer but bigger deeper down. Measured on a 1024 x 1024 world: average pocket about 200 cells near the
  top, about 720 at the bottom, the biggest a few thousand; about 5-7% of the underground is water. Spongestone veins replace
  rock only, so they never split a pocket and often sit right next to one.
- World: 2048 x 1024 (WORLD_W / WORLD_H to change), a small crater at the bottom of the bowl as the starting point.
  Generating it takes 0.3 s; the server needs about 3 bytes per cell (6 MB).
- The island (decided 2026-11-01, inspired by Made in Abyss): the world is an island in an endless ocean. At each edge
  lies a strip of sea 48 columns wide, its seabed sloping from the shore down to 30 rows deep at the edge. From the
  shore (one row above sea level, row 100) a ramp of 110 columns rises to a rim at row 40; between the rims lies a wide
  hilly bowl, about 80 rows deep in the middle, with the starting crater at its bottom. The bowl brings the crowd
  together and sends rain water down into the dig.
- Rubble (decided 2026-11-01): a flooded tunnel could not be fixed, because players only reach the top layer of the
  water. Rubble is dropped on the surface and sinks to the bottom, so a crowd fills a flooded shaft from the bottom up
  until it covers the leak. Falling rubble counts as open space for light, so it never darkens the shaft below it.
- The ocean is endless but can be breached (decided 2026-11-01): it pours into any open space at the island's edge
  columns below sea level and into open cells of the sea strip that touch water (a dry tunnel under the seabed stays
  dry), lowest cells first, at most 2 cells per tick per side (20 per
  second), so a drained sea fills back up to exactly sea level. A griefer can dig a tunnel from the sea
  into the main dig: the sea strip gushes in first, then a steady stream follows, and the crowd must wall off the leak
  where it enters or scoop faster than it flows. Water above sea level in the seas runs off. The coast and seabed are
  ordinary rock; bedrock is only the bottom two rows of the world. Past the edges the client draws each row exactly like
  the island's edge column on that row (sky, visible water, or black), so the ocean lines up even while a breach drains
  the sea, and deep water is black on both sides.
- Surface lakes: one small lake right next to the crater, so new players can play with water at once, and up to four
  more where the bowl is fairly level. Lakes are generated full and still; rain makes them overflow.
- Guests: the browser keeps a random token; inventory is stored per token on the server.

## 2. Technical plan

### Client

- TypeScript, plain WebGL2, built with Vite. No game engine: the world is a texture, one byte per cell.
- Fragment shader: palette per cell type, per-cell shade noise (from the cell position), shadow rows, water animation,
  material textures.
- Pan and zoom like r/place; click to act; cooldown, inventory and depth in HTML.
- The world's edges stay invisible: past the left and right edges the client draws the ocean like deep water (4 rows
  visible, then black), and below the bottom everything is black, so nobody can see how wide or deep the world goes.
  The camera cannot drift far outside the world.
- Keeps only chunks around the viewport.

### Server

- Rust (tokio + axum or similar WebSocket stack). One simulation thread owns the world; network tasks pass actions to it
  through a queue and receive per-tick diffs.
- World: array of chunks, 64x64 cells, one byte per cell (type plus a few flag bits, e.g. "revealed").
- Water: falling-sand rules (down, then down-diagonal, then sideways), only in active chunks; a chunk sleeps when nothing
  in it moved during a tick and wakes when a neighbour changes. Tick rate about 10 Hz.
- Visibility is updated locally when a cell changes; newly revealed cells go into the diff.
- World generation from seeded noise: rock layers, water pockets, material veins by depth.
- Persistence: snapshot of changed chunks to disk every few seconds; accounts and inventories in SQLite or Postgres.

### Network protocol (binary over WebSocket)

The exact message layout is documented at the top of `server/src/protocol.rs`.

- Client -> server: `subscribe(chunk list)`, `action(type, x, y)`.
- Server -> client: chunk snapshot (compressed, only visible information), per-tick diff per subscribed chunk (cell index +
  new value), action result / cooldown, player inventory.
- Size estimate: a large flood changes a few thousand cells per tick, a few KB; idle areas send nothing.

### Anti-abuse

- Accounts (e.g. Discord or Google login) instead of anonymous play; server-side rate limits per account and per IP.
- Possibly longer cooldowns for new accounts.

### Scale targets

- World: start with 1000x1000 for the prototype; 4000 wide x 10000 deep (40 MB) is realistic later.
- 1000+ concurrent players on one server; measured with a bot load test before launch.

## 3. Prototype plan

1. Server: world generation, chunk storage, water simulation, actions with cooldowns, WebSocket protocol.
2. Client: rendering with pan/zoom and fog, chunk streaming, mine / collect / place, HUD.
3. Bot load test: N simulated clients digging and flooding; measure tick time, bandwidth and memory.
4. Playtest locally with a few browser windows.

## 4. Launch scenario

First real test: a streamer with about 500 viewers opens the link on stream and 100+ viewers join at once. This drives
some priorities:

- Zero friction: link -> world -> click a pixel. Play as a guest without login (session cookie, rate limits per
  session and IP); login only for leaderboards and keeping stats.
- Works on phones (touch: tap to act, drag to pan, pinch to zoom); many viewers watch on mobile.
- The crowd must meet: a narrow starting area or a single spawn view, so 100 players end up in the same dig instead of
  spreading over a huge map.
- Moderation tools before any public link: an admin can erase an area and ban a session, because placed pixels (and
  coloured gems) will be used for offensive drawings on stream.
- The streamer's view: readable at stream resolution, maybe a spectator mode that follows the busiest area.

## 5. Open questions

- Exact cooldowns and structure cost.
- Permanent world or seasons with a reset and a final depth leaderboard.
- Which material comes next, and at which depth.
- Login provider.

## 6. Gameplay proposals (not decided)

Ideas to make the game fun beyond the first minutes. Each needs the user's agreement before it is built.

1. ~~Gems are coloured paint.~~ Built, then removed again in favour of materials by depth (section 1).
2. **Glints in the shadow.** A gem in the shadow rows sparkles before it is fully revealed, so players notice a cluster
   one or two digs early and the rush starts before the reveal.
3. **Trolling must be cheaper to fix than to cause, or at least equal.** Breaking a wall takes one click, a flood can take
   hundreds of clicks to clear. Counterweights: finite pockets, water that slowly drains into porous rock or evaporates at
   the surface, structures that absorb water (sponge), a warning outline when the clicked cell touches water (accidental
   floods), longer cooldowns for new accounts.
4. **Harder rock deeper down.** Deep rock needs several mines per cell, so depth records come from cooperation and the
   descent lasts weeks, not hours.
5. **Shared depth milestones.** Reaching certain depths unlocks something for everyone: a new layer with new colours, a
   new structure type, a rain storm, a giant cluster. Gives the crowd a common goal.
6. **Personal progress.** Per-player stats (cells mined, water cleared, gems found, deepest cell mined) and leaderboards,
   so a single player's clicks count for something.
7. **Find the action.** A minimap with an activity heatmap and a "jump to hotspot" button; small flashes where other
   players click. In r/place much of the fun is seeing others act right next to you.
8. **Structures with a purpose.** Only a few kinds: wall (blocks water), ladder or support (cosmetic at first), sign or
   flag (shows a player or group name). Keep the set tiny.
9. **Later:** loose sand that falls (cave-ins), lava deep down, groups or factions with a colour.
