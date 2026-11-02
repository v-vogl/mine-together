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
| Wall | static, blocks water; intact, cracked, crumbling | build on an air or water cell (2 rock); dig cracks it one stage, the third dig breaks it (+2 rock, the full cost); the wall tool repairs a cracked wall (free) |
| Spongerock ore | static, in veins from 15 below the surface | mine: removes it, +1 spongerock |
| Quartz | static, in spiky clusters from 30 below the surface; also a placed block | mine: +1 quartz; place on an air or water cell (1 quartz, place cooldown; needs support in the open sky), to build with or hand to others |
| Amethyst | static, in columns from 140 m; also a placed block | mine: +1 amethyst; place like quartz (1 amethyst) |
| Spongerock | placed block; soaks up one neighbouring water cell every 2 s | place on an air or water cell (1 spongerock); mine: +1 spongerock |
| Rubble | falls 2 cells per tick through air, sinks 1 cell every other tick through water (swapping places with it, so no water is lost), slides off heaps, now and then sticks on a slope; solid once it rests, falls again when undermined | drop on an air or water cell, anywhere (1 rock, dig cooldown); mine: +1 rock |
| Bedrock | static, unbreakable; the island's side walls and the bottom | none ("too hard to dig") |

**Materials by depth** (decided 2026-11-01): instead of gems, new materials with a real use are introduced one at a
time, each first found at a certain depth, so digging deeper means discovering something new. Gems (and gems as paint)
were removed for now. A material's toolbar slot appears once the player has mined some.

1. **Spongerock** (from 15 below the surface, so a lone digger finds some soon): porous yellow-ochre rock. Inert in the ground (otherwise pockets next to
   it would silently drain); mined and placed as a block, it soaks up one neighbouring water cell every 2 seconds. The
   flood fixer. About one vein per 3150 cells, radius 1.5 to 4.55, plus one per 80 columns in the 60 rows where it starts.
2. **Quartz** (from 30 below the surface): the lowest tier of upgrade crystals (higher tiers, up to diamond, may follow
   deeper). Pale pink crystal clusters: a plus-shaped core with 4 to 6 straight or diagonal spikes of 2 to 4 cells, unlike
   spongerock's round blobs; about one per 7000 cells, plus one per 150 columns in the 60 rows where it starts. Mined: +1 quartz, spent on pickaxe upgrades.
3. **Amethyst** (from 140 below the surface, just above the second layer): the second tier of upgrade crystals. A
   druse of 3 to 5 upright purple crystal columns, 2 to 5 cells tall; about one cluster per 12,000 cells. Spent on
   heavy strikes: 3 amethyst for level 1 (every hit on rock does 2 damage), 10 more for level 2 (3 damage).
4. **Swellcoral** (decided 2026-11-02, from 60 below the surface): fleshy pink coral with breathing polyps. Found as
   small branching reefs (3 to 7 cells tall, forking) standing on the floor of water pockets, about one per 50,000
   cells: unlike everything else it lies inside water, so players drain a pocket to get at it. Reef coral sleeps.
   Planted by a player it is alive: every second a third of the living coral grows, into a water cell next to it
   (the water becomes coral) or, if there is none, one cell through air towards water at most 4 cells away; far
   from water it rests. Turns a flood into solid ground, grows a flooded shaft shut, builds bridges over lakes.
   Mined (reef or grown): +1 swellcoral, so a pool of water and one coral make a coral farm.
5. **Emberstone** (decided 2026-11-02, from 90 below the surface): near-black rock with glowing orange cracks, in long
   thin seams that run roughly sideways (8 to 24 cells, now and then stepping a row), about one per 40,000 cells,
   never generated next to water. Every second each emberstone boils one water cell next to it into **steam**.
   Steam rises through air and water (the water drops into its place), creeps under ceilings, and each tick one
   steam cell in 120 wears down a wall (one stage) or rock (one hit; what breaks falls as rubble) next to it.
   It condenses back into water after about 15 s, or drifts off into the sky above the surface. A flood that reaches
   a seam turns into a steam pocket that eats its own ceiling; placed emberstone drains water but endangers the
   walls around it.

**Springs** (decided 2026-11-02): damp rock (wet-looking, from 30 below the surface, about one cell per 35,000) opens
a spring when dug; a few more springs lie hidden at the bottom of water pockets. A spring cannot be dug; it pushes out
a water cell every 0.3 s into open air next to it, or through the water around it up to 12 rows above itself, until
every side is plugged (walls, rubble, spongerock). A flood with a clear target the crowd can fix. Like rain, it scales
with the players online: 20% of that alone (one cell every 1.5 s, slower than one player scoops), 60% at 10 players,
full strength from 100.

**Cave-ins** (decided 2026-11-02): a ceiling cell is unstable when the open space under it reaches 15 cells or more to
both sides, so rooms up to 30 wide hold. After a dig nearby, an unstable ceiling cracks after 3-10 s (visible warning)
and falls as rubble 5-15 s later if nothing was built underneath; each fall checks the ceiling above, so wide caverns
crumble into arches. Walls, rubble and rock under a ceiling hold it: pillars matter. Pending checks are not saved.

**One address, three players** (decided 2026-11-02): players are identified by a browser token, so one person with a
script could open many connections. All players on one internet address share 3 cooldowns (the address acts like at
most 3 players), count at most 3 times towards the players online (speed and rain scaling), and may open at most 10
connections. The real address comes from the proxy's header (TRUST_IP_HEADER: cf-connecting-ip behind the tunnel,
x-forwarded-for behind Caddy). Players on the server's own machine, the dev client and bots with the dev key are exempt.

**Milestones** (decided 2026-11-02): announced to everyone as a gold notice: every new record depth of 25, 50, 75, 100 m,
then every 50 m; the first spongerock, the first quartz, the first spring opened. Saved per world in milestones.json.
New players see "Click a cell to dig" until their first dig (once per browser).

**Art style** (decided 2026-11-02): away from a block-game look. Each cell is one flat colour; patterns run across
many cells (strata, stripes, columns, veins) so the ground reads as one painted mass. Carved edges (rounded corners
next to open space, light top edges) were tried and are off by default; the E key switches them on while the user
decides. No grass blocks: dirt with a mossy rim
and tufts growing into the air. Walls stay crisp bricks, being the one thing players build.

**Kinds of rock** (decided 2026-11-02): a crust of dirt (2-3 cells, 1 hit) lies under the surface; below it the
layers are stone, granite, slate, basalt and darkstone. Each kind looks different (dirt brown under a rim of moss and grass tufts, stone
warm sandstone in wavy strata, granite pink-grey with grains, slate blue-grey stripes, basalt black columns, darkstone
purple-black with red veins), is
its own resource with its own hotbar slot (the deeper ones appear once found), and a placed block keeps its kind's
hardness, so deep rock makes strong defences. Walls, rubble and the wall unlock cost stone.

**Deeper is harder** (decided 2026-11-02): the rock layers (160 rows each) get harder: rock takes 1, 3, 5, 8 and 12 hits
in layers 1 to 5 (a curve; upgrades make it easier); a hit rock shows as chipped with the hits it has left. Each rock
cell's layer is its own cell type, fixed when the world is made, and layers blend in patches: from 30 rows above a
boundary to 30 below it a growing share of the rock is the deeper kind (half of it at the boundary). The colour shows
the layer, so players can see the hard patches coming. Cave-ins come sooner deeper
down: the widest room that holds shrinks from 30 cells in the top layer to 24, 20, 16 and 14, so a deep dig needs walls
as props.

**Walls see and grow** (decided 2026-11-02): a wall and the cells next to it are always visible, even deep under water,
and a wall can be built (or repaired) on any open cell (air or water) next to another wall, out of reach of open air:
a crowd can wall its way down into a flooded shaft. Never into rock, spongerock, quartz or other solid ground.

**Leaderboard** (decided 2026-11-02): one category for now, blocks mined (all time). Opened from a small trophy in the
top bar (or L), so it costs no screen space otherwise: the top 10 and your own place. Players have no accounts yet, so
each gets a stable made-up name from their browser token ("Dusty Mole 42").

**Saving progress** (decided 2026-11-02): play stays anonymous, no login in front of the game. The menu (Esc, or the
☰ in the top bar) shows the player's recovery code (their browser token in groups of four) to copy, and a field to
enter a code from another browser, which switches that browser to the code's player. Later, once the game runs on
its own domain: optional sign-in with Twitch or Discord linked to the same player, and Cloudflare Turnstile against
bots.

**Upgrades** (decided 2026-11-01): an Upgrades panel (key U) appears once a player has 10 rock or has found quartz.
- Walls are locked for new players; unlocking them costs 10 rock. Rubble is always available.
- Pickaxe levels 1 to 5: level n digs (1 + 0.1 n) times as fast (not scooping or building); level n to n + 1 costs
  2^(n + 1) - 1 quartz: 1, 3, 7, 15, 31 (57 quartz in all for x1.5; changed 2026-11-02 from n + 1).

**Walls take three hits** (decided 2026-11-01): griefers can break walls, but defenders can keep up. Each dig cracks a
wall one stage (intact, cracked, crumbling, gone); the wall tool repairs a cracked wall for free with the dig cooldown,
so one defender holds a wall against one attacker, but not against a group.

### Water

- Pockets of water are generated inside the rock and released when a player digs into them.
- Rain events add water at the surface.
- Players can collect water cell by cell.
- Water pockets are finite, so a flood ends and can be cleared; the world must never get stuck in a state players
  cannot fix.

### The Upwelling (decided 2026-11-02)

The second event after rain, and the one that adds to the world instead of taking from it. Every 30 to 60 minutes
(console: `upwell`), strength scaled with the players online like rain (40% alone, full from 100 players).

1. **Stirring** (30 s): announcement "The deep is stirring…", the view trembles slightly, "the deep is stirring…" in
   the top bar.
2. **Veins rising**: 3 to 8 roots start just above bedrock and grow upwards one cell per tick, wandering and forking
   (each branch with part of its parent's remaining length, at most 48 tips), so the deep layers get a web of veins
   and the stone near the top one or two. Molten veins glow, also in shadow, so one coming up shows before it breaks
   in. A branch that breaks into a tunnel spews magma there for up to 40 s. The **main vein** always reaches the
   surface (about 100 s for a 900-row world); crossing a tunnel it spews into it and carries on above it.
3. **Eruption**: where the main vein reaches the sky, a volcano pours magma for 90 to 180 s, 1.75 to 2.5 cells per
   tick; a third of it is thrown up as lava bombs up to 14 cells away. Magma buried in its own pool is pushed out
   through it, so the pool overflows down the sides, and the vent climbs through its own cooled rock: a mountain
   about 65 cells tall for one player, bigger for a crowd. The view shakes for 2.5 s when it breaks through.
4. **Cooling**: the veins cool over about a minute into lava rock, with materials of their depth scattered along
   them (spongerock, quartz, emberstone, amethyst) and, rarely, deep down, a fire opal.

**Cataclysm** (console `cataclysm`, never on its own): an Upwelling at full strength with four times the roots and
fork limit, and five main veins whose columns are spread evenly over the island, each erupting as a supervolcano
(three times the pour rate, twice the time, lava bombs 1.7 times as far). Measured on a 2048 x 1024 world: five
mountains 50 to 120 cells tall, about 26,000 cells of new land, 93,000 cells of veins; at most 36 ms per tick.

Magma: a thick liquid (moves every third tick, piles up). Water touching it turns it into lava rock at once and
boils into steam. It falls freely and hardens on its own after a while (not while falling; slowly in the open
and in tunnels, very slowly inside a pool). Its heat breaks walls stage by stage, melts rubble into more magma, and burns spongerock and coral: built
blocks only hold it back for a while, water is what stops it (unlike floods, which walls stop). Lava rock: two hits
(one with heavier strikes), its own resource, placeable. Fire opal: rare, only collected and placed for now.
Magma and veins cannot be dug. An Upwelling is not saved: after a restart veins still molten are lava rock.

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

- Cooldown: dig 1.5 s, collect water 2.5 s (slower than digging, so floods last), place spongerock 3 s, build a wall 4 s
  (repairing one takes the dig cooldown, so does dropping rubble).
- These cooldowns are for 100 players. Action speed scales with the players online (decided 2026-11-01):
  speed = 2 - 0.5 * log10(players), clamped to 0.5-2, and cooldowns are divided by it: 1 player x2, 10 players x1.5,
  100 players x1, 300 players x0.76, 1000 or more x0.5. Several tabs of one guest count once. The HUD shows the speed.
  Water, rain and the ocean are not affected. One cooldown per player, shared by all actions and
  by all tabs of the same guest.
- A wall costs 2 rock and takes three digs to break, giving the 2 rock back; repairing a cracked wall is free.
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
  top, about 720 at the bottom, the biggest a few thousand; about 5-7% of the underground is water. Spongerock veins replace
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
- The ocean is endless but can be breached (decided 2026-11-01, changed 2026-11-03): only the island's two edge
  columns act as the ocean; everything beyond them is drawn, never simulated. Below sea level every open cell of an
  edge column is water, and it never runs empty: on one tick in three it flows on like pushed water (sideways, down,
  or through the water it touches into air up to sea level) and leaves water behind. A breach gushes in through its
  whole height at once (a cave of 4800 cells fills to sea level in about 25 s) and fills what it reaches up to sea
  level. Stopping it means walling off the openings. The sea strip fills back up to sea
  level, 2 cells per tick per side, through open cells that touch water (a dry tunnel under the seabed stays dry).
  Water above sea level in the seas runs off. The coast and seabed are ordinary rock; bedrock is only the bottom two
  rows of the world. Past the edges the client draws one continuous ocean: bright for 4 rows below sea level, then
  fading into the dark deep, and lit within a few cells of any breach where the edge column holds visible water.
- Surface lakes: one small lake right next to the crater, so new players can play with water at once, and up to four
  more where the bowl is fairly level. Lakes are generated full and still; rain makes them overflow.
- Guests: the browser keeps a random token; inventory is stored per token on the server.

## 2. Technical plan

### Client

- TypeScript, plain WebGL2, built with Vite. No game engine: the world is a texture, one byte per cell.
- Fragment shader: palette per cell type, per-cell shade noise (from the cell position), shadow rows, water animation,
  material textures.
- Pan and zoom like r/place; click to act; cooldown, inventory and depth in HTML.
- The world's edges stay invisible: past the left and right edges the client draws the ocean (bright near the surface,
  fading into darkness), and below the bottom everything is black, so nobody can see how wide or deep the world goes.
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
