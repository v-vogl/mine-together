//! The world grid: cell storage, generation, visibility and the water simulation.
//!
//! One byte per cell. The low 6 bits are the cell type, the top 2 bits the visibility:
//! hidden (0), shadow (SHADOW) or revealed (REVEALED). Visibility always follows the world as it is now:
//! when water floods a tunnel, what is deep under the water goes dark again.

use crate::noise::{self, Rng};
use std::io::{self, Read, Write};
use std::path::Path;

mod upwelling;
pub use upwelling::UpwellingNews;

pub const CHUNK: usize = 64;

pub const AIR: u8 = 0;
pub const ROCK: u8 = 1;
pub const WATER: u8 = 2;
/// Walls built by players. Each dig cracks a wall one stage further; the third dig breaks it.
pub const WALL: u8 = 3;
pub const WALL_CRACKED: u8 = 4;
pub const WALL_CRUMBLING: u8 = 5;
/// Spongerock, the first material players discover (from SPONGE_DEPTH down): porous yellow rock.
/// In the ground it does nothing; mined and placed as a block, it soaks up a neighbouring water cell
/// every few seconds.
pub const SPONGE_ORE: u8 = 6;
pub const SPONGE: u8 = 7;
/// Rubble dropped by players (1 rock). It falls through air, sinks through water and slides off heaps
/// like gravel, so a crowd can fill a flooded shaft from the bottom up and plug a leak. While it moves it
/// is RUBBLE_FALLING, which still counts as open space for light, so a falling stone never darkens a shaft;
/// once it comes to rest it is RUBBLE, solid like rock.
pub const RUBBLE: u8 = 8;
pub const RUBBLE_FALLING: u8 = 9;
/// Unbreakable stone: the world's bottom.
pub const BEDROCK: u8 = 10;
/// Quartz, the lowest tier of the upgrade crystals (from QUARTZ_DEPTH down): small pale pink crystal
/// clusters, rarer than spongerock. Players spend it on pickaxe upgrades.
pub const QUARTZ_ORE: u8 = 11;
/// Damp rock: rock with a spring behind it, wet-looking so players can see it coming. Digging it opens
/// a SPRING, which pours out water (from its open sides) until players plug every side.
pub const SPRING_ROCK: u8 = 12;
/// An open spring: solid, cannot be dug, and pushes water into the open space next to it. Some lie hidden
/// at the bottom of water pockets and keep a pocket full once it is opened.
pub const SPRING: u8 = 13;
/// A ceiling about to cave in (see CAVE_SPAN): cracked rock that falls as rubble soon, unless something
/// is built underneath to hold it.
pub const ROCK_CRACKED: u8 = 14;
/// Rock of each layer, top first: stone (plain ROCK), granite, slate, basalt, darkstone.
pub const ROCK_LAYERS: [u8; 5] = [ROCK, 26, 27, 28, 29];
/// Dirt: a thin crust, DIRT_ROWS deep, right under the surface.
pub const DIRT: u8 = 31;
/// Every kind of rock, each its own resource: dirt, then the layers. Players collect them separately, and a
/// placed block keeps its kind's hardness.
pub const ROCK_KINDS: [u8; 6] = [DIRT, ROCK, 26, 27, 28, 29];
/// Hits it takes to break each kind: deeper rock is much harder.
const KIND_HITS: [u8; 6] = [1, 1, 3, 5, 8, 12];
/// Rock that has been hit but not broken yet keeps its kind: each kind that takes several hits has a run of
/// types from here on, one per number of hits left (see worn).
const WORN_BASE: u8 = 32;
/// Amethyst, the second tier of upgrade crystals (from AMETHYST_DEPTH down, a little above layer 2):
/// clusters of upright purple crystal columns. Spent on heavier strikes (more damage per hit).
pub const AMETHYST_ORE: u8 = 30;

/// Magma, poured out by an Upwelling (see upwelling.rs): a thick liquid that flows slowly and piles up. It
/// hardens into lava rock where it meets air, and at once where it meets water (boiling that water into
/// steam). Its heat slowly breaks walls and other built blocks next to it: they hold it back for a while,
/// but only water stops it.
pub const MAGMA: u8 = 15;
/// A molten vein of an Upwelling, growing through the rock. Cools into lava rock (and now and then a
/// material) when the Upwelling is over.
pub const MOLTEN: u8 = 16;
/// Lava rock: cooled magma, dark and porous, its own resource. Two hits to break (chipped after one).
pub const LAVA_ROCK: u8 = 17;
pub const LAVA_ROCK_CHIPPED: u8 = 18;
/// Fire opal: a rare gem left in cooled veins deep down. For now only collected and placed.
pub const FIRE_OPAL: u8 = 19;

/// Swellcoral, found as small branching reefs on the floor of water pockets (from CORAL_DEPTH down), so it
/// is only reached by draining a pocket. Reef coral sleeps. Coral a player plants (CORAL) is alive: it grows
/// into water next to it, turning that water into coral, and reaches through air towards water a few cells
/// away (see grow_coral). Planted in a flooded shaft, it grows the shaft shut.
pub const CORAL_ORE: u8 = 56;
pub const CORAL: u8 = 57;
/// Emberstone, in long thin seams (from EMBER_DEPTH down): hot rock that boils the water next to it into
/// steam (see boil).
pub const EMBER: u8 = 58;
/// Steam: rises through air and water, gathers under ceilings, and slowly wears down the walls and rock it
/// touches, until it condenses back into water or drifts off into the sky (see move_steam).
pub const STEAM: u8 = 59;

/// The rock layers (the colour bands players see) are this many rows thick.
pub const LAYER_ROWS: usize = 160;
/// Layers blend into each other: from this many rows above a boundary to as many below it, a growing share
/// of the rock belongs to the deeper layer (half of it right at the boundary), in patches.
/// Cave-ins (see CAVE_SPAN): how far open space under a ceiling may reach to both sides in each layer before
/// it caves in. Deeper rock holds smaller rooms, so the deeper the dig, the more it needs walls as props.
const CAVE_SPANS: [i64; 5] = [15, 12, 10, 8, 7];

/// Which rock layer `depth` cells below the surface lies in (0 at the top).
pub fn layer(depth: usize) -> usize {
    (depth / LAYER_ROWS).min(ROCK_LAYERS.len() - 1)
}

fn worn_base(kind: usize) -> u8 {
    WORN_BASE + KIND_HITS[..kind].iter().map(|h| h - 1).sum::<u8>()
}

/// The type of rock of this kind (index into ROCK_KINDS) worn down to `left` hits (at least 1, fewer than
/// the kind takes fresh).
pub fn worn(kind: usize, left: u8) -> u8 {
    worn_base(kind) + left - 1
}

/// For rock of any kind, fresh or worn down: its kind (index into ROCK_KINDS) and the hits still needed.
pub fn rock_state(t: u8) -> Option<(usize, u8)> {
    if let Some(k) = ROCK_KINDS.iter().position(|&r| r == t) {
        return Some((k, KIND_HITS[k]));
    }
    (0..ROCK_KINDS.len()).find_map(|k| {
        let base = worn_base(k);
        (t >= base && t < base + KIND_HITS[k] - 1).then(|| (k, t - base + 1))
    })
}

/// Plain rock in any state: of any kind, worn by hits, or cracked by a cave-in.
pub fn is_rock(t: u8) -> bool {
    t == ROCK_CRACKED || rock_state(t).is_some()
}

pub fn is_wall(t: u8) -> bool {
    (WALL..=WALL_CRUMBLING).contains(&t)
}

pub const TYPE_MASK: u8 = 0x3F;
pub const SHADOW: u8 = 0x40;
pub const REVEALED: u8 = 0x80;
const VIS_MASK: u8 = 0xC0;

/// Water falls up to this many cells per tick.
const FALL_SPEED: i64 = 2;
/// Water moves at most this many cells sideways per tick.
const SPREAD: i64 = 2;
/// How far a water cell looks sideways for a drop to flow towards.
const DROP_SEARCH: i64 = 64;
/// Pressure: the top cell of a water column may jump through connected water to an air cell at least
/// two rows lower than itself. The search looks at most this many water cells, within this distance.
const PRESSURE_SEARCH: usize = 400;
const PRESSURE_RANGE: i64 = 32;
const PRESSURE_SIZE: usize = (2 * PRESSURE_RANGE + 1) as usize;
/// How many cells deep players can see into water.
const WATER_SIGHT: u8 = 4;
/// Water deeper than this counts as "too deep to see" in the sight calculation.
const TOO_DEEP: u8 = WATER_SIGHT + 1;
/// A change can alter what is visible up to this many cells away: water sight, the bed under the
/// water, and two cells of shadow.
const SIGHT_REACH: usize = WATER_SIGHT as usize + 3;
/// Cells around a chunk the sight calculation looks at.
const SIGHT_MARGIN: usize = SIGHT_REACH + 1;

/// Water pocket shape: noise scale in cells near the surface, how much bigger it gets at the bottom
/// of the world, and the noise level above which a cell is water (higher means rarer pockets).
const POCKET_WIDTH: f64 = 26.0;
const POCKET_HEIGHT: f64 = 14.0;
const POCKET_GROWTH: f64 = 2.5;
const POCKET_THRESHOLD: f64 = 0.71;

/// The island: the world is land in an endless ocean. At each edge lies a strip of sea SHELF columns
/// wide, its seabed sloping from the shore down to SEA_DEPTH at the edge. From the shore (one row above
/// sea level) a ramp of RAMP columns rises to a rim at RIM_ROW; between the rims lies a hilly bowl,
/// BOWL_DEPTH rows deep in the middle, where the crowd starts digging.
const SEA_LEVEL: usize = 100;
const SHELF: usize = 48;
const SEA_DEPTH: f64 = 30.0;
const RAMP: usize = 110;
const RIM_ROW: f64 = 40.0;
const BOWL_DEPTH: f64 = 80.0;
/// The ocean beyond each edge is endless and presses on every opening at once: below sea level the edge
/// columns are the ocean. Every open cell there is water, and it never runs empty: when it flows somewhere
/// (on one tick in OCEAN_REFILL) it leaves water behind, so a breach gushes in through its whole height. The
/// sea itself fills back up to sea level, OCEAN_INFLOW cells per tick per side.
const OCEAN_REFILL: u64 = 3;
const OCEAN_INFLOW: usize = 2;
/// Checking whether a space is still connected to the sky gives up (and assumes it is) after this many
/// cells, so one wall can never stall the server.
const LIT_SEARCH_LIMIT: usize = 300_000;

/// Spongerock starts this many cells below the surface, so a crowd finds it after a while of digging.
const SPONGE_DEPTH: usize = 15;
/// Quartz starts this many cells below the surface.
const QUARTZ_DEPTH: usize = 30;
/// Extra clusters of spongerock and quartz in a band this many rows deep right below where each starts, so
/// even a lone digger near the surface soon finds some.
const SHALLOW_BAND: usize = 60;
/// Amethyst starts this many cells below the surface, a little above the second rock layer.
const AMETHYST_DEPTH: usize = 140;
/// Coral reefs start this many cells below the surface, emberstone seams this many.
const CORAL_DEPTH: usize = 60;
const EMBER_DEPTH: usize = 90;
/// On each turn (see grow_coral) one living coral cell in this many grows.
const CORAL_CHANCE: u64 = 3;
/// Coral reaches through air towards water at most this many cells away.
const CORAL_REACH: usize = 4;
/// Per tick, one steam cell in this many condenses back into water, one in STEAM_WEAR wears down a wall or
/// rock next to it, and out in the open sky one in STEAM_ESCAPE drifts away.
const STEAM_CONDENSE: u64 = 150;
const STEAM_WEAR: u64 = 120;
const STEAM_ESCAPE: u64 = 10;
/// Magma falls every tick but flows only every MAGMA_SLOW ticks. Per tick, one magma cell in MAGMA_COOL hardens if it touches
/// air (one in MAGMA_COOL_SKY out in the open sky; one in MAGMA_COOL_ENCLOSED
/// inside a pool), and one in MAGMA_HEAT scorches a block next to it.
const MAGMA_SLOW: i64 = 3;
const MAGMA_COOL: u64 = 300;
const MAGMA_COOL_SKY: u64 = 200;
const MAGMA_COOL_ENCLOSED: u64 = 1000;
const MAGMA_HEAT: u64 = 20;
/// Rows of blending above and below each layer boundary (see LAYER_ROWS).
const LAYER_BLEND: f64 = 30.0;
/// The dirt crust is 2 or 3 cells deep.
const DIRT_ROWS: usize = 2;
/// Springs start this many cells below the surface.
const SPRING_DEPTH: usize = 30;
/// Evaporation: every EVAPORATE_EVERY ticks each column gets a turn, and the top water cell lying out in the
/// open above the ground has this chance in 1000 to dry up (on average after 50 s), so rain that covered
/// the land drains away within a few minutes. Lakes, seas and everything dug out stay wet.
const EVAPORATE_EVERY: usize = 10;
const EVAPORATE_PER_MILLE: u64 = 20;
/// Water cells a spring pushes out per tick at full strength (a crowd of 100 or more; the simulation scales
/// it down for fewer players, see step_springs).
pub const SPRING_RATE: f64 = 1.0 / 3.0;
/// A spring pushes water through a flooded space up to this many rows above itself (like an artesian
/// well), so filling the shaft over it does not stop it; plugging its sides does.
const SPRING_HEAD: i64 = 12;
/// Cave-ins: a ceiling cell is unstable when the open space under it reaches at least this far to both
/// sides (in the top layer; less deeper down, see CAVE_SPANS), so rooms up to 2 * CAVE_SPAN cells wide
/// hold, wider ones crack in the middle and collapse into an arch. Walls, rubble and rock under the
/// ceiling count as support.
const CAVE_SPAN: i64 = 15;
/// After digging, unstable ceilings crack after 3 to 10 s, and a cracked one falls 5 to 15 s later.
const CAVE_CRACK_DELAY: (u64, u64) = (30, 70);
const CAVE_FALL_DELAY: (u64, u64) = (50, 100);

const FILE_MAGIC: &[u8; 4] = b"MTW1";

/// Open space for light and sight: air, water, steam, magma and rubble on its way down.
pub fn is_open(t: u8) -> bool {
    t == AIR || t == WATER || t == RUBBLE_FALLING || t == STEAM || t == MAGMA
}

/// Air or steam: open space that players see and reach through.
fn airy(t: u8) -> bool {
    t == AIR || t == STEAM
}

pub struct World {
    pub w: usize,
    pub h: usize,
    pub seed: u64,
    /// Per column, the first row below the sky. Air above it is drawn as sky.
    pub surface: Vec<u16>,
    cells: Vec<u8>,
    /// Chunks per row and per column.
    cw: usize,
    ch: usize,
    /// Chunks simulated this tick, and chunks woken for the next one.
    active: Vec<bool>,
    next_active: Vec<bool>,
    /// Tick number in which a water cell last moved, so it moves only once per tick.
    stamp: Vec<u8>,
    tick: u8,
    /// Cells whose visible value changed since the last take_dirty.
    dirty_flag: Vec<bool>,
    dirty: Vec<u32>,
    /// Chunks whose visibility must be recalculated, because something near them changed.
    sight_dirty: Vec<bool>,
    /// Scratch space for the pressure search: cells already looked at.
    seen: Vec<bool>,
    /// Row of the ocean's surface around the island; the shore is the row above it.
    pub sea_level: usize,
    /// Open cells (air and water) connected to the sky or the ocean. Only these let light through: a
    /// tunnel sealed off with a wall goes dark, and nobody can dig inside it.
    lit: Vec<bool>,
    /// Placed spongerock blocks, which soak up water.
    sponges: std::collections::BTreeSet<usize>,
    /// Open springs, which pour out water.
    springs: std::collections::BTreeSet<usize>,
    /// Living coral, which grows towards water, and emberstone, which boils it.
    corals: std::collections::BTreeSet<usize>,
    embers: std::collections::BTreeSet<usize>,
    /// The Upwelling going on, if any. Not saved: after a restart its veins have cooled (see load).
    upwelling: Option<upwelling::Upwelling>,
    /// Ceiling cells to look at for a cave-in: (due tick, cell). Not saved: after a restart, ceilings are
    /// looked at again once someone digs near them.
    cave_checks: std::collections::BinaryHeap<std::cmp::Reverse<(u64, usize)>>,
    /// Ticks since the world was loaded.
    age: u64,
    /// Fraction of a spring's next water cell built up so far (shared by all springs).
    spring_carry: f64,
    /// Scratch space for searches over the world: the search number that last visited each cell.
    visit: Vec<u32>,
    visit_round: u32,
    rng: Rng,
}

impl World {
    fn from_parts(w: usize, h: usize, seed: u64, surface: Vec<u16>, cells: Vec<u8>) -> World {
        assert!(w % CHUNK == 0 && h % CHUNK == 0, "world size must be a multiple of {CHUNK}");
        let (cw, ch) = (w / CHUNK, h / CHUNK);
        let sea_level = surface[0] as usize;
        let sponges = (0..w * h).filter(|&i| cells[i] & TYPE_MASK == SPONGE).collect();
        let springs = (0..w * h).filter(|&i| cells[i] & TYPE_MASK == SPRING).collect();
        let corals = (0..w * h).filter(|&i| cells[i] & TYPE_MASK == CORAL).collect();
        let embers = (0..w * h).filter(|&i| cells[i] & TYPE_MASK == EMBER).collect();
        World {
            w,
            h,
            seed,
            surface,
            cells,
            cw,
            ch,
            active: vec![false; cw * ch],
            next_active: vec![false; cw * ch],
            stamp: vec![0; w * h],
            tick: 0,
            dirty_flag: vec![false; w * h],
            dirty: Vec::new(),
            sight_dirty: vec![false; cw * ch],
            seen: vec![false; PRESSURE_SIZE * PRESSURE_SIZE],
            sea_level,
            lit: vec![false; w * h],
            sponges,
            springs,
            corals,
            embers,
            upwelling: None,
            cave_checks: std::collections::BinaryHeap::new(),
            age: 0,
            spring_carry: 0.0,
            visit: vec![0; w * h],
            visit_round: 0,
            rng: Rng::new(seed ^ 0x5EED),
        }
    }

    /// The ocean beyond the edges is endless (see OCEAN_REFILL): it pours into every opening in the island's
    /// edge columns below sea level at once, and keeps the seas full. Water above sea level in the seas runs off.
    pub fn tend_ocean(&mut self) {
        if self.w < 2 * SHELF {
            return;
        }
        let w = self.w;
        for x in [0, w - 1] {
            for y in self.sea_level..self.h {
                let i = y * w + x;
                if airy(self.ty(i)) {
                    self.set_type(i, WATER);
                }
            }
        }
        // A drained sea fills back up to exactly sea level, lowest cells first. Only cells next to water count:
        // a dry tunnel under the seabed stays dry until it is dug open.
        let sea_bottom = self.sea_level + SEA_DEPTH as usize + 2;
        for right in [false, true] {
            let col = |k: usize| if right { w - 1 - k } else { k };
            let mut inflow = OCEAN_INFLOW;
            'fill: for y in (self.sea_level..sea_bottom.min(self.h)).rev() {
                for k in 1..SHELF {
                    let i = y * self.w + col(k);
                    if self.ty(i) == AIR && self.neighbours(i).any(|n| self.ty(n) == WATER) {
                        self.set_type(i, WATER);
                        inflow -= 1;
                        if inflow == 0 {
                            break 'fill;
                        }
                    }
                }
            }
        }
        for x in (0..SHELF).chain(self.w - SHELF..self.w) {
            for y in self.sea_level.saturating_sub(3)..self.sea_level {
                let i = y * self.w + x;
                if self.ty(i) == WATER && self.ty(i + self.w) == WATER {
                    self.set_type(i, AIR);
                }
            }
        }
    }

    pub fn generate(w: usize, h: usize, seed: u64) -> World {
        let mut rng = Rng::new(seed ^ 0xDEAD_BEEF);

        // The island's surface: sea at each edge, a ramp up from the shore, then a hilly bowl between the
        // rims. For sea columns the surface is the sea level (sky above it).
        let mid = w / 2;
        let bowl_radius = (mid - SHELF - RAMP) as f64;
        let hills = |x: usize| (noise::fbm(seed, x as f64 / 60.0, 0.5, 4) - 0.5) * 16.0;
        let mut surface: Vec<u16> = (0..w)
            .map(|x| {
                let from_edge = x.min(w - 1 - x);
                let row = if from_edge < SHELF {
                    SEA_LEVEL as f64
                } else if from_edge < SHELF + RAMP {
                    // Steep near the shore, flattening towards the rim; bumps fade out at both ends.
                    let t = (from_edge - SHELF) as f64 / RAMP as f64;
                    let shore = (SEA_LEVEL - 1) as f64;
                    shore + (RIM_ROW - shore) * (1.0 - (1.0 - t) * (1.0 - t)) + hills(x) * 0.3 * (t * std::f64::consts::PI).sin()
                } else {
                    let u = (x as f64 - mid as f64) / bowl_radius;
                    RIM_ROW + BOWL_DEPTH * (1.0 - u * u) + hills(x) * (1.0 - u * u)
                };
                row.round() as u16
            })
            .collect();

        let mut cells = vec![ROCK; w * h];
        for x in 0..w {
            for y in 0..surface[x] as usize {
                cells[y * w + x] = AIR;
            }
        }

        // Water pockets: blobs of noise above a threshold. The noise is stretched more deeper down, so
        // pockets there are fewer but much bigger: a real flood when someone breaks in.
        for y in 0..h {
            let deep = y as f64 / h as f64;
            let stretch = 1.0 + POCKET_GROWTH * deep;
            // Rows are squeezed together by 1 / (POCKET_HEIGHT * stretch); summed over depth that gives a
            // logarithm, so the noise stays smooth from row to row.
            let ny = h as f64 / (POCKET_GROWTH * POCKET_HEIGHT) * (1.0 + POCKET_GROWTH * deep).ln();
            for x in 0..w {
                let depth = y as i64 - surface[x] as i64;
                if depth < 14 {
                    continue;
                }
                let n = noise::fbm(seed ^ 0x57A7E5, x as f64 / (POCKET_WIDTH * stretch), ny, 3);
                if n > POCKET_THRESHOLD {
                    cells[y * w + x] = WATER;
                }
            }
        }

        // Spongerock veins, from SPONGE_DEPTH below the surface down; it only replaces rock.
        // Some extra veins only in the shallow band (one per 80 columns).
        let veins = w * h * 99 / 312_500; // one per ~3150 cells
        for n in 0..veins + w / 80 {
            let cx = rng.below(w as u64) as i64;
            let top = surface[cx as usize] as usize + SPONGE_DEPTH;
            if top + 4 >= h {
                continue;
            }
            let span = if n < veins { h - 4 - top } else { SHALLOW_BAND.min(h - 4 - top) };
            let cy = (top + rng.below(span as u64) as usize) as i64;
            let r = 1.5 + rng.unit() * 3.05; // radius 1.5 to 4.55
            let ri = r.ceil() as i64;
            for dy in -ri..=ri {
                for dx in -ri..=ri {
                    let (x, y) = (cx + dx, cy + dy);
                    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                        continue;
                    }
                    let i = y as usize * w + x as usize;
                    let deep_enough = y as usize >= surface[x as usize] as usize + SPONGE_DEPTH;
                    if ((dx * dx + dy * dy) as f64) <= r * r
                        && noise::unit(seed ^ 0x5B09, x, y) > 0.25
                        && cells[i] == ROCK
                        && deep_enough
                    {
                        cells[i] = SPONGE_ORE;
                    }
                }
            }
        }

        // Quartz: rarer than spongerock and a little deeper, and shaped differently: not a round blob but
        // a crystal cluster, a plus-shaped core with 4 to 6 spikes of 2 to 4 cells sticking out in straight
        // or diagonal directions. It only replaces rock.
        const SPIKES: [(i64, i64); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];
        // Some extra clusters only in the shallow band (one per 150 columns).
        let clusters = w * h / 7000;
        for n in 0..clusters + w / 150 {
            let cx = rng.below(w as u64) as i64;
            let top = surface[cx as usize] as usize + QUARTZ_DEPTH;
            if top + 6 >= h {
                continue;
            }
            let span = if n < clusters { h - 6 - top } else { SHALLOW_BAND.min(h - 6 - top) };
            let cy = (top + 3 + rng.below(span as u64) as usize) as i64;
            let mut put = |x: i64, y: i64| {
                if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                    return;
                }
                let i = y as usize * w + x as usize;
                if cells[i] == ROCK && y as usize >= surface[x as usize] as usize + QUARTZ_DEPTH {
                    cells[i] = QUARTZ_ORE;
                }
            };
            for (dx, dy) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
                put(cx + dx, cy + dy);
            }
            for _ in 0..4 + rng.below(3) {
                let (dx, dy) = SPIKES[rng.below(8) as usize];
                for k in 1..=2 + rng.below(3) as i64 {
                    put(cx + dx * k, cy + dy * k);
                }
            }
        }

        // Amethyst: rarer again, and shaped like a druse: 3 to 5 upright crystal columns side by side, each 2 to 5
        // cells tall, so it looks nothing like quartz's spiky stars. It only replaces rock.
        for _ in 0..w * h / 12_000 {
            let cx = rng.below(w as u64) as i64;
            let top = surface[cx as usize] as usize + AMETHYST_DEPTH;
            if top + 10 >= h {
                continue;
            }
            let base = (top + 6 + rng.below((h - 10 - top) as u64) as usize) as i64;
            let columns = 3 + rng.below(3) as i64;
            for k in 0..columns {
                let x = cx - columns / 2 + k;
                for j in 0..2 + rng.below(4) as i64 {
                    let y = base - j;
                    if x < 0 || x >= w as i64 || y < 0 {
                        continue;
                    }
                    let i = y as usize * w + x as usize;
                    if cells[i] == ROCK && y as usize >= surface[x as usize] as usize + AMETHYST_DEPTH {
                        cells[i] = AMETHYST_ORE;
                    }
                }
            }
        }

        // Emberstone: long thin seams running roughly sideways, 8 to 24 cells, now and then stepping up or
        // down a row, unlike the blobs and clusters of the other materials. Never right next to water, so no
        // pocket boils before anyone gets there.
        for _ in 0..w * h / 40_000 {
            let x0 = rng.below(w as u64) as usize;
            let top = surface[x0] as usize + EMBER_DEPTH;
            if top + 4 >= h {
                continue;
            }
            let (mut x, mut y) = (x0 as i64, (top + rng.below((h - 4 - top) as u64) as usize) as i64);
            let dir = if rng.next() & 1 == 0 { -1 } else { 1 };
            for _ in 0..8 + rng.below(17) {
                if x < 1 || x >= w as i64 - 1 || y < 1 || y >= h as i64 - 1 {
                    break;
                }
                let i = y as usize * w + x as usize;
                let dry = [i - 1, i + 1, i - w, i + w].iter().all(|&n| cells[n] != WATER);
                if cells[i] == ROCK && dry && y as usize >= surface[x as usize] as usize + EMBER_DEPTH {
                    cells[i] = EMBER;
                }
                x += dir;
                match rng.below(6) {
                    0 => y -= 1,
                    1 => y += 1,
                    _ => {}
                }
            }
        }

        // Springs: single cells of damp rock, and a few springs hidden at the bottom of water pockets.
        for _ in 0..w * h / 35_000 {
            let x = rng.below(w as u64) as usize;
            let top = surface[x] as usize + SPRING_DEPTH;
            if top + 2 >= h {
                continue;
            }
            let i = (top + rng.below((h - 2 - top) as u64) as usize) * w + x;
            if cells[i] == ROCK {
                cells[i] = SPRING_ROCK;
            }
        }
        let mut pocket_springs = w * h / 100_000;
        for _ in 0..200_000 {
            if pocket_springs == 0 {
                break;
            }
            let x = rng.below(w as u64) as usize;
            let y = surface[x] as usize + SPRING_DEPTH + rng.below((h / 2) as u64) as usize;
            if y + 2 >= h {
                continue;
            }
            let i = y * w + x;
            if cells[i] == WATER && cells[i + w] == ROCK {
                cells[i + w] = SPRING;
                pocket_springs -= 1;
            }
        }

        // Swellcoral reefs: on the floor of water pockets, a little tree of coral branching up into the water.
        // Unlike the materials in the rock, it is found inside water: players drain a pocket to get at it.
        let mut reefs = w * h / 50_000;
        for _ in 0..400_000 {
            if reefs == 0 {
                break;
            }
            let x = rng.below(w as u64) as usize;
            let top = surface[x] as usize + CORAL_DEPTH;
            if top + 2 >= h {
                continue;
            }
            let y = top + rng.below((h - 2 - top) as u64) as usize;
            if cells[y * w + x] != WATER || cells[(y + 1) * w + x] != ROCK {
                continue;
            }
            reefs -= 1;
            // Each step up, every tip grows one cell, drifting to a side now and then; a tip may fork.
            let mut tips = vec![(x as i64, y as i64)];
            for _ in 0..3 + rng.below(5) {
                let mut next = Vec::new();
                for &(tx, ty) in &tips {
                    if tx < 0 || tx >= w as i64 || ty < 0 || cells[ty as usize * w + tx as usize] != WATER {
                        continue;
                    }
                    cells[ty as usize * w + tx as usize] = CORAL_ORE;
                    let drift = [-1, 0, 0, 1][rng.below(4) as usize];
                    next.push((tx + drift, ty - 1));
                    if tips.len() + next.len() < 5 && rng.below(3) == 0 {
                        next.push((tx + if drift > 0 { -1 } else { 1 }, ty - 1));
                    }
                }
                tips = next;
            }
        }

        // A small crater at the bottom of the bowl, so the crowd has an obvious place to start.
        for dx in -7i64..=7 {
            let x = (mid as i64 + dx) as usize;
            let depth = ((64 - dx * dx).max(0) as f64).sqrt() as usize * 3 / 4;
            let top = surface[x] as usize;
            for y in top..top + depth {
                cells[y * w + x] = AIR;
            }
            surface[x] = (top + depth) as u16;
        }

        // Surface lakes: one right next to the crater, so water is there to play with from the start,
        // and a few more spread out, away from the middle.
        let side: i64 = if rng.next() & 1 == 0 { -1 } else { 1 };
        let mut lakes = vec![((mid as i64 + side * 32) as usize, 12)];
        for _ in 0..200 {
            if lakes.len() > w / 256 {
                break;
            }
            let hw = 10 + rng.below(20) as usize;
            let margin = SHELF + RAMP + hw + 10;
            let Some(span) = w.checked_sub(2 * margin).filter(|&span| span > 0) else { continue };
            let x0 = margin + rng.below(span as u64) as usize;
            let clear_of_others = lakes.iter().all(|&(x, r)| x0.abs_diff(x) > hw + r + 20);
            // Only where the ground is fairly level; on a slope the lake would cut a cliff into the hill.
            let level = surface[x0 - hw].abs_diff(surface[x0 + hw]) <= 3;
            if clear_of_others && level && x0.abs_diff(mid) > hw + 60 {
                lakes.push((x0, hw));
            }
        }
        for (k, &(x0, hw)) in lakes.iter().enumerate() {
            let depth = if k == 0 { 5.0 } else { 3.0 + rng.unit() * 6.0 };
            carve_lake(&mut cells, &mut surface, w, x0, hw, depth);
        }

        // The seas: water from sea level down to a seabed that slopes from the shore to the edge.
        for x in (0..SHELF).chain(w - SHELF..w) {
            let t = 1.0 - x.min(w - 1 - x) as f64 / SHELF as f64;
            let seabed = SEA_LEVEL + (SEA_DEPTH * t.powf(1.5) + 1.0).round() as usize;
            for y in SEA_LEVEL..seabed {
                cells[y * w + x] = WATER;
            }
        }
        // Rock layers: each rock cell gets its layer here, by depth, with patchy blends at the boundaries.
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if cells[i] != ROCK || y < surface[x] as usize {
                    continue;
                }
                let depth = (y - surface[x] as usize) as f64;
                let patch = 0.75 * noise::value(seed ^ 0x1A7E, x as f64 / 7.0, y as f64 / 5.0)
                    + 0.25 * noise::unit(seed ^ 0x1A7F, x as i64, y as i64);
                let mut k = 0;
                while k + 1 < ROCK_LAYERS.len() {
                    let start = ((k + 1) * LAYER_ROWS) as f64 - LAYER_BLEND;
                    if patch >= ((depth - start) / (2.0 * LAYER_BLEND)).clamp(0.0, 1.0) {
                        break;
                    }
                    k += 1;
                }
                cells[i] = ROCK_LAYERS[k];
            }
        }
        // A crust of dirt on top, 2 or 3 cells deep.
        for x in 0..w {
            let rows = DIRT_ROWS + (noise::hash(seed ^ 0xD127, x as i64 / 3, 0) % 2) as usize;
            for y in surface[x] as usize..(surface[x] as usize + rows).min(h) {
                if cells[y * w + x] == ROCK {
                    cells[y * w + x] = DIRT;
                }
            }
        }
        for c in &mut cells[(h - 2) * w..] {
            *c = BEDROCK;
        }

        let mut world = World::from_parts(w, h, seed, surface, cells);
        world.relight_all();
        world.update_all_sight();
        world
    }

    pub fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    fn index_of(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            None
        } else {
            Some(y as usize * self.w + x as usize)
        }
    }

    pub fn ty(&self, i: usize) -> u8 {
        self.cells[i] & TYPE_MASK
    }

    pub fn is_revealed(&self, i: usize) -> bool {
        self.cells[i] & REVEALED != 0
    }

    /// The value a client may see: hidden cells are always 0, so their type never leaves the server.
    pub fn wire(&self, i: usize) -> u8 {
        let c = self.cells[i];
        if c & VIS_MASK == 0 {
            0
        } else {
            c
        }
    }

    fn air_at(&self, x: i64, y: i64) -> bool {
        self.index_of(x, y).is_some_and(|i| self.ty(i) == AIR)
    }

    fn mark_dirty(&mut self, i: usize) {
        if !self.dirty_flag[i] {
            self.dirty_flag[i] = true;
            self.dirty.push(i as u32);
        }
    }

    pub fn take_dirty(&mut self) -> Vec<u32> {
        for &i in &self.dirty {
            self.dirty_flag[i as usize] = false;
        }
        std::mem::take(&mut self.dirty)
    }

    /// Wake the chunks around a changed cell, so water next to it gets simulated next tick.
    fn wake(&mut self, i: usize) {
        let (x, y) = (i % self.w, i / self.w);
        let (x0, x1) = (x.saturating_sub(1) / CHUNK, (x + 1).min(self.w - 1) / CHUNK);
        let (y0, y1) = (y.saturating_sub(1) / CHUNK, (y + 1).min(self.h - 1) / CHUNK);
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                self.next_active[cy * self.cw + cx] = true;
            }
        }
    }

    pub fn wake_all(&mut self) {
        self.next_active.fill(true);
    }

    pub fn set_type(&mut self, i: usize, t: u8) {
        let c = self.cells[i];
        if c & TYPE_MASK == t {
            return;
        }
        let was_open = is_open(c & TYPE_MASK);
        if c & TYPE_MASK == SPONGE {
            self.sponges.remove(&i);
        }
        if t == SPONGE {
            self.sponges.insert(i);
        }
        if c & TYPE_MASK == SPRING {
            self.springs.remove(&i);
        }
        if t == SPRING {
            self.springs.insert(i);
        }
        for (kind, set) in [(CORAL, &mut self.corals), (EMBER, &mut self.embers)] {
            if c & TYPE_MASK == kind {
                set.remove(&i);
            }
            if t == kind {
                set.insert(i);
            }
        }
        self.cells[i] = (c & VIS_MASK) | t;
        if c & VIS_MASK != 0 {
            self.mark_dirty(i);
        }
        self.wake(i);
        self.mark_sight(i);
        match (was_open, is_open(t)) {
            (false, true) => {
                self.opened(i);
                self.queue_cave_checks(i);
            }
            (true, false) => self.closed(i),
            _ => {}
        }
        if t == AIR || t == WATER {
            self.loosen_rubble(i);
        }
    }

    /// Cell `i` just opened up: rubble resting on it, or on the slope beside it, starts moving again.
    fn loosen_rubble(&mut self, i: usize) {
        let (x, y) = ((i % self.w) as i64, (i / self.w) as i64);
        let Some(above) = self.index_of(x, y - 1) else { return };
        if self.ty(above) == RUBBLE {
            self.set_type(above, RUBBLE_FALLING);
        }
        // Diagonally above can only slide in if the cell straight above is open (no squeezing past corners).
        if !is_open(self.ty(above)) {
            return;
        }
        for dx in [-1, 1] {
            if let Some(n) = self.index_of(x + dx, y - 1) {
                if self.ty(n) == RUBBLE {
                    self.set_type(n, RUBBLE_FALLING);
                }
            }
        }
    }

    /// Visibility near this cell has to be recalculated.
    fn mark_sight(&mut self, i: usize) {
        let (x, y) = (i % self.w, i / self.w);
        let (x0, x1) = (x.saturating_sub(SIGHT_REACH) / CHUNK, (x + SIGHT_REACH).min(self.w - 1) / CHUNK);
        let (y0, y1) = (y.saturating_sub(SIGHT_REACH) / CHUNK, (y + SIGHT_REACH).min(self.h - 1) / CHUNK);
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                self.sight_dirty[cy * self.cw + cx] = true;
            }
        }
    }

    fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let (x, y) = ((i % self.w) as i64, (i / self.w) as i64);
        [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().filter_map(move |(dx, dy)| self.index_of(x + dx, y + dy))
    }

    /// Open cells in the top row touch the sky, open cells in the edge columns touch the ocean.
    fn is_anchor(&self, i: usize) -> bool {
        let x = i % self.w;
        i < self.w || x == 0 || x == self.w - 1
    }

    fn next_visit_round(&mut self) -> u32 {
        self.visit_round = self.visit_round.wrapping_add(1);
        if self.visit_round == 0 {
            self.visit.fill(0);
            self.visit_round = 1;
        }
        self.visit_round
    }

    /// Light every open space connected to the sky or the ocean, and nothing else.
    fn relight_all(&mut self) {
        self.lit.fill(false);
        let anchors: Vec<usize> = (0..self.w * self.h).filter(|&i| self.is_anchor(i) && is_open(self.ty(i))).collect();
        for i in anchors {
            self.light_from(i);
        }
    }

    /// Light the unlit open space around `start` (which must be open).
    fn light_from(&mut self, start: usize) {
        if self.lit[start] {
            return;
        }
        self.lit[start] = true;
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            self.mark_sight(i);
            let next: Vec<usize> = self.neighbours(i).filter(|&n| !self.lit[n] && is_open(self.ty(n))).collect();
            for n in next {
                self.lit[n] = true;
                stack.push(n);
            }
        }
    }

    /// A closed cell was opened (dug, or water/air appeared): it joins any lit space next to it.
    fn opened(&mut self, i: usize) {
        if self.is_anchor(i) || self.neighbours(i).any(|n| self.lit[n]) {
            self.light_from(i);
        }
    }

    /// An open cell was closed (a wall or gem placed): the space next to it may now be cut off from the
    /// sky. Check each lit neighbour; a space that cannot reach the sky or the ocean goes dark.
    fn closed(&mut self, i: usize) {
        if !self.lit[i] {
            return;
        }
        self.lit[i] = false;
        let starts: Vec<usize> = self.neighbours(i).filter(|&n| self.lit[n]).collect();
        let mut reached_sky: Vec<u32> = Vec::new();
        for start in starts {
            // Already found to be connected through an earlier neighbour's search?
            if reached_sky.contains(&self.visit[start]) {
                continue;
            }
            let round = self.next_visit_round();
            let (connected, visited) = self.search_sky(start, round);
            if connected {
                reached_sky.push(round);
            } else {
                for c in visited {
                    self.lit[c] = false;
                    self.mark_sight(c);
                }
            }
        }
    }

    /// Search the open space from `start` for the sky or the ocean, upwards first so a connected space is
    /// found quickly. Returns whether it got there, and the cells it looked at.
    fn search_sky(&mut self, start: usize, round: u32) -> (bool, Vec<usize>) {
        use std::cmp::Reverse;
        let mut heap = std::collections::BinaryHeap::new();
        let mut visited = vec![start];
        self.visit[start] = round;
        heap.push(Reverse((start / self.w, start)));
        while let Some(Reverse((_, i))) = heap.pop() {
            if self.is_anchor(i) || visited.len() > LIT_SEARCH_LIMIT {
                return (true, visited);
            }
            let next: Vec<usize> =
                self.neighbours(i).filter(|&n| self.visit[n] != round && is_open(self.ty(n))).collect();
            for n in next {
                self.visit[n] = round;
                visited.push(n);
                heap.push(Reverse((n / self.w, n)));
            }
        }
        (false, visited)
    }

    /// Can a wall or gem be placed here? Underground anything goes, but in the open sky (above the
    /// original surface) it must rest against something solid: no floating blocks or pictures in the sky.
    pub fn supported(&self, i: usize) -> bool {
        let (x, y) = (i % self.w, i / self.w);
        y >= self.surface[x] as usize || self.neighbours(i).any(|n| !is_open(self.ty(n)))
    }

    fn lit_air(&self, i: usize) -> bool {
        self.lit[i] && airy(self.ty(i))
    }

    fn touches_lit_air(&self, i: usize) -> bool {
        self.neighbours(i).any(|n| self.lit_air(n))
    }

    /// Is a wall next to this cell? Walls can be built on from there, even out of reach of open air.
    pub fn touches_wall(&self, i: usize) -> bool {
        self.neighbours(i).any(|n| is_wall(self.ty(n)))
    }

    /// Can a player reach this cell? Only visible cells touching lit air can be dug, scooped or built on:
    /// the bottom of a lake is out of reach until the water above it is gone, and so is everything inside
    /// a tunnel that was sealed off.
    pub fn reachable(&self, i: usize) -> bool {
        self.is_revealed(i) && (self.lit_air(i) || self.touches_lit_air(i))
    }

    /// Turn a cell into air (digging or scooping). What is behind it shows up with the next sight update.
    pub fn dig(&mut self, i: usize) {
        self.set_type(i, AIR);
    }

    pub fn update_all_sight(&mut self) {
        self.sight_dirty.fill(true);
        self.update_sight();
        self.take_dirty();
    }

    /// Recalculate visibility in every chunk where something changed nearby.
    pub fn update_sight(&mut self) {
        for c in 0..self.sight_dirty.len() {
            if self.sight_dirty[c] {
                self.sight_dirty[c] = false;
                self.update_chunk_sight(c % self.cw, c / self.cw);
            }
        }
    }

    /// Visibility of one chunk, from the world as it is now. "Air" here means lit air, connected to the sky:
    /// - visible: air, water at most WATER_SIGHT cells from air, and solid cells touching air or touching
    ///   water that is less deep than that (a lake bed under up to 3 cells of water);
    /// - shadow: cells up to 2 behind visible air or solid cells. Water only gets shadow next to rock lit by
    ///   air (a pocket behind a thin wall shows as a warning); deep water and water beside a lake bed stay black;
    /// - hidden: everything else.
    fn update_chunk_sight(&mut self, cx: usize, cy: usize) {
        let w = self.w;
        let (x0, y0) = (cx * CHUNK, cy * CHUNK);
        let (rx0, ry0) = (x0.saturating_sub(SIGHT_MARGIN), y0.saturating_sub(SIGHT_MARGIN));
        let (rx1, ry1) = ((x0 + CHUNK + SIGHT_MARGIN).min(w), (y0 + CHUNK + SIGHT_MARGIN).min(self.h));
        let (rw, rh) = (rx1 - rx0, ry1 - ry0);

        // Water depth in the region around the chunk: 1 next to lit air, growing inwards, capped at TOO_DEEP.
        let mut depth = vec![0u8; rw * rh];
        let mut queue = std::collections::VecDeque::new();
        for ly in 0..rh {
            for lx in 0..rw {
                let i = (ry0 + ly) * w + rx0 + lx;
                if self.ty(i) == WATER {
                    if self.touches_lit_air(i) {
                        depth[ly * rw + lx] = 1;
                        queue.push_back((lx, ly));
                    } else {
                        depth[ly * rw + lx] = TOO_DEEP;
                    }
                }
            }
        }
        while let Some((lx, ly)) = queue.pop_front() {
            let d = depth[ly * rw + lx] + 1;
            if d >= TOO_DEEP {
                continue;
            }
            for (nx, ny) in [(lx + 1, ly), (lx.wrapping_sub(1), ly), (lx, ly + 1), (lx, ly.wrapping_sub(1))] {
                if nx < rw && ny < rh && depth[ny * rw + nx] > d {
                    depth[ny * rw + nx] = d;
                    queue.push_back((nx, ny));
                }
            }
        }
        let depth_at = |x: usize, y: usize| depth[(y - ry0) * rw + (x - rx0)];

        // What is visible in the chunk plus a 2-cell border: 0 no, 1 visible water, 2 air or solid touching air,
        // 3 solid seen only through water.
        const B: usize = 2;
        let size = CHUNK + 2 * B;
        let mut seen = vec![0u8; size * size];
        for sy in 0..size {
            for sx in 0..size {
                let (Some(x), Some(y)) = ((x0 + sx).checked_sub(B), (y0 + sy).checked_sub(B)) else { continue };
                if x >= w || y >= self.h {
                    continue;
                }
                let i = y * w + x;
                // Walls are always visible, and so is everything next to one: players see along a wall they
                // built, even deep under water, and can build it on from there (see touches_wall).
                let t = self.ty(i);
                if is_wall(t) || self.touches_wall(i) {
                    seen[sy * size + sx] = if t == WATER { 1 } else { 2 };
                    continue;
                }
                seen[sy * size + sx] = match t {
                    AIR | STEAM => 2 * self.lit[i] as u8,
                    WATER => (depth_at(x, y) <= WATER_SIGHT) as u8,
                    _ => {
                        let (xi, yi) = (x as i64, y as i64);
                        let mut lit = 0;
                        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                            if let Some(n) = self.index_of(xi + dx, yi + dy) {
                                match self.ty(n) {
                                    AIR | STEAM if self.lit[n] => lit = 2,
                                    WATER if lit == 0 && depth_at(n % w, n / w) < WATER_SIGHT => lit = 3,
                                    _ => {}
                                }
                            }
                        }
                        lit
                    }
                };
            }
        }

        for ly in 0..CHUNK {
            for lx in 0..CHUNK {
                let (sx, sy) = (lx + B, ly + B);
                let i = (y0 + ly) * w + x0 + lx;
                let water = self.ty(i) == WATER;
                let vis = if seen[sy * size + sx] > 0 {
                    REVEALED
                } else {
                    let near = (0..=2 * B).any(|dy| {
                        (0..=2 * B).any(|dx| {
                            let source = seen[(sy + dy - B) * size + sx + dx - B];
                            (dx.abs_diff(B) < 2 || dy.abs_diff(B) < 2) && (source == 2 || source == 3 && !water)
                        })
                    });
                    if near { SHADOW } else { 0 }
                };
                if self.cells[i] & VIS_MASK != vis {
                    self.cells[i] = (self.cells[i] & TYPE_MASK) | vis;
                    self.mark_dirty(i);
                }
            }
        }
    }

    /// Every placed spongerock block soaks up one neighbouring water cell.
    pub fn absorb(&mut self) {
        let sponges: Vec<usize> = self.sponges.iter().copied().collect();
        for i in sponges {
            let wet = self.neighbours(i).find(|&n| self.ty(n) == WATER);
            if let Some(n) = wet {
                self.set_type(n, AIR);
            }
        }
    }

    /// Put a water cell into the world if the spot is air (used for rain).
    pub fn add_water(&mut self, x: usize, y: usize) {
        let i = self.idx(x, y);
        if self.ty(i) == AIR {
            self.set_type(i, WATER);
        }
    }

    /// One step of the falling-water automaton, only in chunks where something changed recently.
    pub fn step_water(&mut self) {
        self.tick = if self.tick == u8::MAX { 1 } else { self.tick + 1 };
        std::mem::swap(&mut self.active, &mut self.next_active);
        self.next_active.fill(false);

        // Bottom-up, so a falling cell is not moved twice. Alternate left/right each tick so water
        // does not drift to one side.
        let flip = self.tick & 1 == 1;
        let (cw, ch) = (self.cw, self.ch);
        for cy in (0..ch).rev() {
            if !self.active[cy * cw..(cy + 1) * cw].iter().any(|&a| a) {
                continue;
            }
            for y in (cy * CHUNK..(cy + 1) * CHUNK).rev() {
                for k in 0..cw {
                    let cx = if flip { cw - 1 - k } else { k };
                    if !self.active[cy * cw + cx] {
                        continue;
                    }
                    for j in 0..CHUNK {
                        let x = cx * CHUNK + if flip { CHUNK - 1 - j } else { j };
                        let i = y * self.w + x;
                        if self.stamp[i] == self.tick {
                            continue;
                        }
                        match self.cells[i] & TYPE_MASK {
                            WATER => self.move_water(x as i64, y as i64),
                            RUBBLE_FALLING => self.move_rubble(x as i64, y as i64),
                            STEAM => self.move_steam(x as i64, y as i64),
                            MAGMA => self.move_magma(x as i64, y as i64),
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    fn move_water(&mut self, x: i64, y: i64) {
        let from = y as usize * self.w + x as usize;

        let mut ny = y;
        while ny - y < FALL_SPEED && self.air_at(x, ny + 1) {
            ny += 1;
        }
        if ny != y {
            return self.move_cell(from, x, ny);
        }

        let d0 = if self.rng.next() & 1 == 0 { -1 } else { 1 };

        // Slide down diagonally, but never squeeze through a diagonal gap between two solid cells:
        // a wall built as a staircase must hold water.
        for d in [d0, -d0] {
            if self.air_at(x + d, y) && self.air_at(x + d, y + 1) {
                return self.move_cell(from, x + d, y + 1);
            }
        }

        // Flow sideways towards a nearby drop. Without a drop, only spread when water above pushes,
        // so a settled pool stops moving and its chunk can sleep. In the edge columns below sea level the
        // endless ocean beyond always pushes.
        let ocean = self.is_ocean(from);
        if ocean && self.rng.below(OCEAN_REFILL) != 0 {
            return self.wake(from);
        }
        let pushed = ocean || self.index_of(x, y - 1).is_some_and(|i| self.ty(i) == WATER);
        for d in [d0, -d0] {
            let mut reach = 0;
            let mut drop = false;
            while reach < DROP_SEARCH && self.air_at(x + d * (reach + 1), y) {
                reach += 1;
                if self.air_at(x + d * reach, y + 1) {
                    drop = true;
                    break;
                }
            }
            if reach > 0 && (drop || pushed) {
                return self.move_cell(from, x + d * reach.min(SPREAD), y);
            }
        }

        // Pressure: the top cell of a column pushes through the water it is connected to, so connected
        // shafts fill up to the same level and trapped air bubbles get filled.
        // Every other tick only, so floods do not fill shafts in the blink of an eye. On a skipped turn the
        // chunk stays awake if there is somewhere to go, so the cell gets its turn next tick.
        if ocean {
            // The ocean's weight: it pushes through the water it touches up to sea level.
            if let Some(to) = self.air_through_water(x, y, self.sea_level as i64 + 1) {
                self.move_to(from, to);
            }
        } else if !pushed {
            if let Some(to) = self.pressure_target(x, y) {
                if (self.tick as i64 + x) & 1 == 0 {
                    self.move_to(from, to);
                } else {
                    self.wake(from);
                }
            }
        }
    }

    /// Search the water connected to (x, y) for an air cell lower than (x, y).
    fn pressure_target(&mut self, x: i64, y: i64) -> Option<usize> {
        // At least two rows lower: a single stray cell on top of a still pool does not jump into a hole
        // dug at the pool's surface, which looked like the game trolling the player.
        self.air_through_water(x, y, y + 2)
    }

    /// Search the water connected to (x, y) for an air cell in row `min_row` or lower.
    fn air_through_water(&mut self, x: i64, y: i64, min_row: i64) -> Option<usize> {
        let local = |px: i64, py: i64| {
            let (lx, ly) = (px - x + PRESSURE_RANGE, py - y + PRESSURE_RANGE);
            (lx >= 0 && ly >= 0 && lx < PRESSURE_SIZE as i64 && ly < PRESSURE_SIZE as i64)
                .then(|| ly as usize * PRESSURE_SIZE + lx as usize)
        };
        self.seen.fill(false);
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((x, y));
        self.seen[local(x, y).unwrap()] = true;
        let mut visited = 0;
        while let Some((cx, cy)) = queue.pop_front() {
            visited += 1;
            if visited > PRESSURE_SEARCH {
                return None;
            }
            // Look downwards first, so water drains towards the lowest gaps.
            for (dx, dy) in [(0, 1), (-1, 0), (1, 0), (0, -1)] {
                let (nx, ny) = (cx + dx, cy + dy);
                let (Some(n), Some(l)) = (self.index_of(nx, ny), local(nx, ny)) else { continue };
                if self.seen[l] {
                    continue;
                }
                self.seen[l] = true;
                match self.ty(n) {
                    AIR if ny >= min_row => return Some(n),
                    WATER => queue.push_back((nx, ny)),
                    _ => {}
                }
            }
        }
        None
    }

    /// Rubble falls fast through air, sinks slowly through water, and slides off the side of a heap.
    fn move_rubble(&mut self, x: i64, y: i64) {
        let from = y as usize * self.w + x as usize;
        let ty_at = |w: &World, x: i64, y: i64| w.index_of(x, y).map(|i| w.ty(i));
        match ty_at(self, x, y + 1) {
            Some(AIR) | Some(STEAM) => {
                let mut ny = y + 1;
                while ny - y < FALL_SPEED && ty_at(self, x, ny + 1) == Some(AIR) {
                    ny += 1;
                }
                return self.swap_rubble(from, ny as usize * self.w + x as usize);
            }
            // Through water only every other tick: it sinks, it does not drop.
            Some(WATER) => {
                if (self.tick as i64 + x) & 1 == 0 {
                    self.swap_rubble(from, from + self.w);
                } else {
                    self.wake(from);
                }
                return;
            }
            // Wait for the stone below to get out of the way.
            Some(RUBBLE_FALLING) => return self.wake(from),
            _ => {}
        }
        // Landed on something solid: roll down the side of the heap. Now and then a stone stays put on a
        // slope, so heaps come out rough rather than perfect triangles.
        if self.rng.next() % 8 != 0 {
            let d0 = if self.rng.next() & 1 == 0 { -1 } else { 1 };
            for d in [d0, -d0] {
                let loose = |t: Option<u8>| t == Some(AIR) || t == Some(WATER);
                if loose(ty_at(self, x + d, y)) && loose(ty_at(self, x + d, y + 1)) {
                    return self.swap_rubble(from, (y + 1) as usize * self.w + (x + d) as usize);
                }
            }
        }
        self.set_type(from, RUBBLE);
    }

    /// Move falling rubble from `from` to `to`; whatever was at `to` (air or water) takes its place.
    fn swap_rubble(&mut self, from: usize, to: usize) {
        let displaced = self.ty(to);
        self.set_type(to, RUBBLE_FALLING);
        self.set_type(from, displaced);
        self.stamp[to] = self.tick;
        self.stamp[from] = self.tick;
    }

    /// Steam rises through air and water (the water drops into its place), creeps sideways under a ceiling,
    /// and now and then wears down a wall or rock next to it. Sooner or later it condenses back into water;
    /// out in the open sky it drifts away.
    fn move_steam(&mut self, x: i64, y: i64) {
        let from = y as usize * self.w + x as usize;
        if (y as usize) < self.surface[x as usize] as usize && self.rng.below(STEAM_ESCAPE) == 0 {
            return self.set_type(from, AIR);
        }
        if self.rng.below(STEAM_CONDENSE) == 0 {
            return self.set_type(from, WATER);
        }
        if self.rng.below(STEAM_WEAR) == 0 {
            let sides: Vec<usize> = self.neighbours(from).collect();
            let n = sides[self.rng.below(sides.len() as u64) as usize];
            self.wear_by_steam(n);
        }
        let ty_at = |w: &World, x: i64, y: i64| w.index_of(x, y).map(|i| w.ty(i));
        let loose = |t: Option<u8>| t == Some(AIR) || t == Some(WATER);
        let w = self.w;
        let at = |x: i64, y: i64| y as usize * w + x as usize;
        if loose(ty_at(self, x, y - 1)) {
            return self.swap_steam(from, at(x, y - 1));
        }
        let d0 = if self.rng.next() & 1 == 0 { -1 } else { 1 };
        for d in [d0, -d0] {
            if loose(ty_at(self, x + d, y)) && loose(ty_at(self, x + d, y - 1)) {
                return self.swap_steam(from, at(x + d, y - 1));
            }
        }
        if ty_at(self, x + d0, y) == Some(AIR) && self.rng.next() & 1 == 0 {
            return self.swap_steam(from, at(x + d0, y));
        }
        // Stuck under a ceiling: keep the chunk awake, so it goes on wearing and condensing.
        self.wake(from);
    }

    /// Move steam from `from` to `to`; whatever was at `to` (air or water) takes its place.
    fn swap_steam(&mut self, from: usize, to: usize) {
        let displaced = self.ty(to);
        self.set_type(to, STEAM);
        self.set_type(from, displaced);
        self.stamp[to] = self.tick;
        self.stamp[from] = self.tick;
    }

    /// Steam wears down a wall by one stage, or rock by one hit; what breaks falls as rubble. Crystals,
    /// coral and other materials resist it.
    fn wear_by_steam(&mut self, i: usize) {
        let t = self.ty(i);
        let to = match t {
            WALL => WALL_CRACKED,
            WALL_CRACKED => WALL_CRUMBLING,
            WALL_CRUMBLING | ROCK_CRACKED | LAVA_ROCK_CHIPPED => RUBBLE_FALLING,
            LAVA_ROCK => LAVA_ROCK_CHIPPED,
            _ => match rock_state(t) {
                Some((kind, left)) if left > 1 => worn(kind, left - 1),
                Some(_) => RUBBLE_FALLING,
                None => return,
            },
        };
        self.set_type(i, to);
    }

    /// Magma: water next to it turns it into lava rock at once (and boils into steam). Otherwise it cools
    /// slowly, scorches the blocks around it, and every MAGMA_SLOW ticks oozes down, down a slope, or now and
    /// then sideways.
    fn move_magma(&mut self, x: i64, y: i64) {
        let from = y as usize * self.w + x as usize;
        let wet: Vec<usize> = self.neighbours(from).filter(|&n| self.ty(n) == WATER).collect();
        if !wet.is_empty() {
            for n in wet {
                self.set_type(n, STEAM);
            }
            return self.set_type(from, LAVA_ROCK);
        }
        // Magma still falling (a lava bomb) does not harden in mid-air.
        let falling = self.index_of(x, y + 1).is_some_and(|n| airy(self.ty(n)));
        let cool = if falling {
            u64::MAX
        } else if (y as usize) < self.surface[x as usize] as usize {
            MAGMA_COOL_SKY
        } else if self.neighbours(from).any(|n| airy(self.ty(n))) {
            MAGMA_COOL
        } else {
            MAGMA_COOL_ENCLOSED
        };
        if self.rng.below(cool) == 0 {
            return self.set_type(from, LAVA_ROCK);
        }
        if self.rng.below(MAGMA_HEAT) == 0 {
            let sides: Vec<usize> = self.neighbours(from).collect();
            let n = sides[self.rng.below(sides.len() as u64) as usize];
            self.scorch(n);
        }
        let open = |w: &World, x: i64, y: i64| w.index_of(x, y).is_some_and(|i| airy(w.ty(i)));
        let w = self.w;
        let at = |x: i64, y: i64| y as usize * w + x as usize;
        // It falls freely; only flowing is slow.
        if open(self, x, y + 1) {
            return self.swap_magma(from, at(x, y + 1));
        }
        if (self.tick as i64 + x) % MAGMA_SLOW != 0 {
            return self.wake(from);
        }
        let d0 = if self.rng.next() & 1 == 0 { -1 } else { 1 };
        for d in [d0, -d0] {
            if open(self, x + d, y) && open(self, x + d, y + 1) {
                return self.swap_magma(from, at(x + d, y + 1));
            }
        }
        if open(self, x + d0, y) && self.rng.below(3) == 0 {
            return self.swap_magma(from, at(x + d0, y));
        }
        self.wake(from);
    }

    /// Move magma from `from` to `to`; the air or steam at `to` takes its place.
    fn swap_magma(&mut self, from: usize, to: usize) {
        let displaced = self.ty(to);
        self.set_type(to, MAGMA);
        self.set_type(from, displaced);
        self.stamp[to] = self.tick;
        self.stamp[from] = self.tick;
    }

    /// Heat from magma or a vein: walls crack a stage at a time and then give way, rubble melts into magma,
    /// spongerock and coral burn away. Natural rock and crystals withstand it.
    fn scorch(&mut self, i: usize) {
        let to = match self.ty(i) {
            WALL => WALL_CRACKED,
            WALL_CRACKED => WALL_CRUMBLING,
            WALL_CRUMBLING | SPONGE | CORAL => AIR,
            RUBBLE | RUBBLE_FALLING => MAGMA,
            _ => return,
        };
        self.set_type(i, to);
    }

    /// Living coral grows: one cell in CORAL_CHANCE turns a water cell next to it into coral, or else grows
    /// one cell through air towards water at most CORAL_REACH cells away. Coral far from water rests.
    pub fn grow_coral(&mut self) {
        let corals: Vec<usize> = self.corals.iter().copied().collect();
        for i in corals {
            if self.rng.below(CORAL_CHANCE) != 0 {
                continue;
            }
            let wet = self.neighbours(i).find(|&n| self.ty(n) == WATER);
            let into = if wet.is_some() { wet } else { self.air_towards_water(i) };
            if let Some(n) = into {
                self.set_type(n, CORAL);
            }
        }
    }

    /// The air cell next to `start` on the shortest way through air to water at most CORAL_REACH away.
    fn air_towards_water(&mut self, start: usize) -> Option<usize> {
        let first: Vec<usize> = self.neighbours(start).filter(|&n| self.ty(n) == AIR).collect();
        if first.is_empty() {
            return None;
        }
        let round = self.next_visit_round();
        self.visit[start] = round;
        let mut queue = std::collections::VecDeque::new();
        for n in first {
            self.visit[n] = round;
            queue.push_back((n, n, 1));
        }
        while let Some((c, step, d)) = queue.pop_front() {
            let next: Vec<usize> = self.neighbours(c).filter(|&n| self.visit[n] != round).collect();
            for n in next {
                self.visit[n] = round;
                match self.ty(n) {
                    WATER => return Some(step),
                    AIR if d < CORAL_REACH => queue.push_back((n, step, d + 1)),
                    _ => {}
                }
            }
        }
        None
    }

    /// Every emberstone boils one water cell next to it into steam.
    pub fn boil(&mut self) {
        let embers: Vec<usize> = self.embers.iter().copied().collect();
        for i in embers {
            let wet = self.neighbours(i).find(|&n| self.ty(n) == WATER);
            if let Some(n) = wet {
                self.set_type(n, STEAM);
            }
        }
    }

    /// Rain water lying above the ground slowly dries up: the top water cell of each column, if it lies above
    /// the column's surface line (out in the open, not in a lake, the sea or a dug-out hole, which all lie at
    /// or below the surface line). One column in EVAPORATE_EVERY gets its turn each tick.
    pub fn evaporate(&mut self) {
        for x in (self.age as usize % EVAPORATE_EVERY..self.w).step_by(EVAPORATE_EVERY) {
            let surface = self.surface[x] as usize;
            let Some(y) = (0..surface).find(|&y| self.ty(y * self.w + x) != AIR) else { continue };
            if self.ty(y * self.w + x) == WATER && self.rng.below(1000) < EVAPORATE_PER_MILLE {
                self.set_type(y * self.w + x, AIR);
            }
        }
    }

    /// Every open spring pushes out a water cell: into open air next to it, or else through the water
    /// around it up to SPRING_HEAD rows above it. A spring with every side plugged stays quiet. `rate` is
    /// water cells per spring per tick.
    pub fn step_springs(&mut self, rate: f64) {
        self.spring_carry += rate;
        if self.spring_carry < 1.0 {
            return;
        }
        self.spring_carry -= 1.0;
        let springs: Vec<usize> = self.springs.iter().copied().collect();
        for i in springs {
            let (x, y) = ((i % self.w) as i64, (i / self.w) as i64);
            let next_to = [(0, 1), (-1, 0), (1, 0), (0, -1)]
                .into_iter()
                .filter_map(|(dx, dy)| self.index_of(x + dx, y + dy))
                .find(|&n| self.ty(n) == AIR);
            let wet = self.neighbours(i).any(|n| self.ty(n) == WATER);
            let to = next_to.or_else(|| if wet { self.air_through_water(x, y, y - SPRING_HEAD) } else { None });
            if let Some(to) = to {
                self.set_type(to, WATER);
            }
        }
    }

    /// Cell `i` just opened up. The ceiling cells next to it on the row above may have lost their support.
    fn queue_cave_checks(&mut self, i: usize) {
        let (x, y) = ((i % self.w) as i64, (i / self.w) as i64);
        for dx in -(CAVE_SPAN + 1)..=CAVE_SPAN + 1 {
            if let Some(c) = self.index_of(x + dx, y - 1) {
                if is_rock(self.ty(c)) {
                    let due = self.age + CAVE_CRACK_DELAY.0 + self.rng.below(CAVE_CRACK_DELAY.1);
                    self.cave_checks.push(std::cmp::Reverse((due, c)));
                }
            }
        }
    }

    /// Is this rock a ceiling with too much open space under it? It holds if something solid stands
    /// under the ceiling within CAVE_SPAN cells on at least one side.
    fn ceiling_unstable(&self, i: usize) -> bool {
        let (x, y) = ((i % self.w) as i64, (i / self.w) as i64);
        if (y as usize) < self.surface[x as usize] as usize {
            return false;
        }
        let open_below = |x: i64| self.index_of(x, y + 1).is_some_and(|n| is_open(self.ty(n)));
        if !open_below(x) {
            return false;
        }
        let span = CAVE_SPANS[layer(y as usize - self.surface[x as usize] as usize)];
        let reach = |d: i64| (1..=span).take_while(|&k| open_below(x + d * k)).count() as i64;
        reach(-1).min(reach(1)) >= span
    }

    /// Cave-ins due this tick: unstable ceiling rock cracks first, and cracked rock that is still unstable
    /// when its time comes falls as rubble. Each fall opens a cell, which checks the ceiling above it, so
    /// a wide cavern crumbles upwards into an arch.
    pub fn step_caveins(&mut self) {
        self.age += 1;
        while let Some(&std::cmp::Reverse((due, i))) = self.cave_checks.peek() {
            if due > self.age {
                break;
            }
            self.cave_checks.pop();
            let t = self.ty(i);
            if !is_rock(t) || !self.ceiling_unstable(i) {
                continue;
            }
            if t == ROCK_CRACKED {
                self.set_type(i, RUBBLE_FALLING);
                continue;
            }
            // Worn rock cracks like fresh rock.
            self.set_type(i, ROCK_CRACKED);
            let due = self.age + CAVE_FALL_DELAY.0 + self.rng.below(CAVE_FALL_DELAY.1);
            self.cave_checks.push(std::cmp::Reverse((due, i)));
        }
    }

    fn move_cell(&mut self, from: usize, x: i64, y: i64) {
        self.move_to(from, y as usize * self.w + x as usize);
    }

    fn move_to(&mut self, from: usize, to: usize) {
        self.set_type(to, WATER);
        // The ocean never runs empty.
        if !self.is_ocean(from) {
            self.set_type(from, AIR);
        }
        self.stamp[to] = self.tick;
    }

    /// The edge columns below sea level are the endless ocean (see OCEAN_REFILL).
    fn is_ocean(&self, i: usize) -> bool {
        let x = i % self.w;
        (x == 0 || x == self.w - 1) && i / self.w >= self.sea_level
    }

    /// Visible contents of one chunk, run-length encoded as (value, count) byte pairs.
    pub fn encode_chunk(&self, cx: usize, cy: usize, out: &mut Vec<u8>) {
        let mut run: Option<(u8, u8)> = None;
        for y in cy * CHUNK..(cy + 1) * CHUNK {
            for x in cx * CHUNK..(cx + 1) * CHUNK {
                let v = self.wire(y * self.w + x);
                run = match run {
                    Some((rv, n)) if rv == v && n < u8::MAX => Some((rv, n + 1)),
                    Some((rv, n)) => {
                        out.extend_from_slice(&[rv, n]);
                        Some((v, 1))
                    }
                    None => Some((v, 1)),
                };
            }
        }
        if let Some((rv, n)) = run {
            out.extend_from_slice(&[rv, n]);
        }
    }

    pub fn chunks_wide(&self) -> usize {
        self.cw
    }

    pub fn chunks_high(&self) -> usize {
        self.ch
    }

    /// Deepest dug cell below the surface, for the depth record.
    pub fn deepest_dig(&self) -> u32 {
        for y in (0..self.h).rev() {
            for x in 0..self.w {
                let i = y * self.w + x;
                if y > self.surface[x] as usize && self.is_revealed(i) && self.ty(i) == AIR {
                    return (y - self.surface[x] as usize) as u32;
                }
            }
        }
        0
    }

    /// The world as a file image; writing it to disk can then happen on another thread.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.cells.len() + self.w * 2 + 32);
        buf.extend_from_slice(FILE_MAGIC);
        buf.extend_from_slice(&(self.w as u32).to_le_bytes());
        buf.extend_from_slice(&(self.h as u32).to_le_bytes());
        buf.extend_from_slice(&self.seed.to_le_bytes());
        for s in &self.surface {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf.extend_from_slice(&self.cells);
        buf
    }

    pub fn load(path: &Path) -> io::Result<World> {
        let mut buf = Vec::new();
        std::fs::File::open(path)?.read_to_end(&mut buf)?;
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a world file");
        if buf.len() < 20 || &buf[0..4] != FILE_MAGIC {
            return Err(bad());
        }
        let w = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
        let seed = u64::from_le_bytes(buf[12..20].try_into().unwrap());
        let cells_at = 20 + w * 2;
        if buf.len() != cells_at + w * h {
            return Err(bad());
        }
        let surface = buf[20..cells_at]
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let mut cells = buf[cells_at..].to_vec();
        // An Upwelling is not saved: veins still molten cool at once.
        for c in &mut cells {
            if *c & TYPE_MASK == MOLTEN {
                *c = (*c & VIS_MASK) | LAVA_ROCK;
            }
        }
        let mut world = World::from_parts(w, h, seed, surface, cells);
        // Water may have been moving when the world was saved.
        world.wake_all();
        world.relight_all();
        world.update_all_sight();
        Ok(world)
    }
}

/// Dig a bowl between the rim columns x0 - hw and x0 + hw and fill it with water up to the lower rim,
/// so the lake is still and cannot spill over when the world starts.
fn carve_lake(cells: &mut [u8], surface: &mut [u16], w: usize, x0: usize, hw: usize, depth: f64) {
    let level = surface[x0 - hw].max(surface[x0 + hw]) as usize;
    for x in x0 - hw + 1..x0 + hw {
        let dx = (x as f64 - x0 as f64) / hw as f64;
        let bottom = level + (depth * (1.0 - dx * dx)).round() as usize;
        let top = surface[x] as usize;
        for y in top.min(level)..bottom.max(top) {
            cells[y * w + x] = if y < level { AIR } else { WATER };
        }
        surface[x] = level as u16;
    }
}

/// Write to a temporary file first, so a crash during saving never leaves a broken file behind.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::File::create(&tmp)?.write_all(data)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn count(world: &World, t: u8) -> usize {
        (0..world.w * world.h).filter(|&i| world.ty(i) == t).count()
    }

    /// A small test world: air above row 8, rock below.
    fn flat(w: usize, h: usize) -> World {
        let mut cells = vec![ROCK; w * h];
        cells[..8 * w].fill(AIR);
        let mut world = World::from_parts(w, h, 1, vec![8; w], cells);
        world.relight_all();
        world.update_all_sight();
        world
    }

    #[test]
    fn rubble_sinks_to_the_bottom_of_a_flooded_shaft_and_heaps_up() {
        let mut world = flat(64, 64);
        // A shaft 5 wide from the surface down to row 39, its bottom 6 rows flooded.
        for y in 8..40 {
            for x in 30..35 {
                world.dig(world.idx(x, y));
                if y >= 34 {
                    world.set_type(world.idx(x, y), WATER);
                }
            }
        }
        let water = count(&world, WATER);
        for _ in 0..12 {
            world.set_type(world.idx(32, 8), RUBBLE_FALLING);
            for _ in 0..60 {
                world.step_water();
            }
        }
        assert_eq!(count(&world, RUBBLE_FALLING), 0, "everything comes to rest");
        assert_eq!(count(&world, RUBBLE), 12);
        // Rubble only takes the place of water, so not a drop is lost: it rises above the rubble.
        assert_eq!(count(&world, WATER), water);
        for y in 34..40 {
            for x in 30..35 {
                assert_ne!(world.ty(world.idx(x, y)), AIR, "a gap under the water at ({x}, {y})");
            }
        }
        // The rubble lies on the floor, below the water.
        for i in 0..world.w * world.h {
            if world.ty(i) == RUBBLE {
                assert!(i / world.w >= 36, "rubble floating at row {}", i / world.w);
            }
        }
        // Digging out the rock under a heap lets it drop again.
        let under = (30..35).find(|&x| world.ty(world.idx(x, 39)) == RUBBLE).unwrap();
        world.dig(world.idx(under, 40));
        assert_eq!(world.ty(world.idx(under, 39)), RUBBLE_FALLING);
    }

    #[test]
    fn coral_grows_towards_water_and_turns_it_into_coral() {
        let mut world = flat(64, 64);
        // A room 10 wide at rows 20-24: water in its right part, coral planted on the floor at the left.
        for y in 20..25 {
            for x in 20..30 {
                world.dig(world.idx(x, y));
            }
        }
        for y in 22..25 {
            for x in 26..30 {
                world.set_type(world.idx(x, y), WATER);
            }
        }
        world.set_type(world.idx(22, 24), CORAL);
        let water = count(&world, WATER);
        for _ in 0..200 {
            world.grow_coral();
            world.step_water();
        }
        assert!(count(&world, WATER) < water / 2, "coral should eat most of the water");
        // It grew towards the water, not everywhere: the far corner of the room stays open.
        assert_eq!(world.ty(world.idx(20, 20)), AIR);
        // Without water nearby it rests.
        for i in 0..world.w * world.h {
            if world.ty(i) == WATER {
                world.set_type(i, AIR);
            }
        }
        let corals = count(&world, CORAL);
        for _ in 0..50 {
            world.grow_coral();
        }
        assert_eq!(count(&world, CORAL), corals);
    }

    #[test]
    fn emberstone_boils_water_and_steam_wears_down_the_ceiling() {
        let mut world = flat(64, 64);
        // A closed room at rows 20-24 with a wall in its ceiling, water on its floor, emberstone beneath.
        for y in 20..25 {
            for x in 20..30 {
                world.dig(world.idx(x, y));
            }
        }
        for x in 20..30 {
            world.set_type(world.idx(x, 24), WATER);
            world.set_type(world.idx(x, 25), EMBER);
        }
        world.set_type(world.idx(25, 19), WALL);
        let mut steam_seen = 0;
        for t in 0..3000 {
            if t % 10 == 0 {
                world.boil();
            }
            world.step_water();
            steam_seen = steam_seen.max(count(&world, STEAM));
        }
        assert!(steam_seen > 0, "water next to emberstone boils");
        assert_ne!(world.ty(world.idx(25, 19)), WALL, "steam under the ceiling cracks the wall");
        let worn = (20..30).filter(|&x| world.ty(world.idx(x, 19)) != ROCK).count();
        assert!(worn > 1, "steam wears the rock of the ceiling");
    }

    #[test]
    fn a_spring_pours_until_every_side_is_plugged() {
        let mut world = flat(64, 64);
        // A shaft down to a room; damp rock in the room's floor.
        for y in 8..30 {
            world.dig(world.idx(32, y));
        }
        for x in 26..40 {
            for y in 28..30 {
                world.dig(world.idx(x, y));
            }
        }
        let spring = world.idx(32, 30);
        world.set_type(spring, SPRING_ROCK);
        world.set_type(spring, SPRING);
        let run = |world: &mut World, ticks: usize| {
            for _ in 0..ticks {
                world.step_springs(SPRING_RATE);
                world.step_caveins();
                world.step_water();
            }
        };
        run(&mut world, 60);
        let poured = count(&world, WATER);
        assert!(poured >= 15, "only {poured} water cells after 6 s");
        // Filling the room does not stop it: it pushes on up the shaft.
        run(&mut world, 300);
        let top = (8..30).find(|&y| world.ty(world.idx(32, y)) == WATER).unwrap();
        assert!(top < 28, "water should rise into the shaft, top at {top}");
        // Plug the spring: bail out the water around it and wall it in.
        for i in 0..world.w * world.h {
            if world.ty(i) == WATER {
                world.set_type(i, AIR);
            }
        }
        for (x, y) in [(31, 30), (33, 30), (32, 29)] {
            world.set_type(world.idx(x, y), WALL);
        }
        run(&mut world, 60);
        assert_eq!(count(&world, WATER), 0);
    }

    #[test]
    fn wide_ceilings_cave_in_and_narrow_ones_hold() {
        let mut world = flat(128, 64);
        let room = |world: &mut World, x0: usize, x1: usize| {
            for y in 9..12 {
                world.dig(world.idx(x0 - 1, y)); // a way in from the surface
            }
            for x in x0..x1 {
                for y in 12..15 {
                    world.dig(world.idx(x, y));
                }
            }
            world.dig(world.idx(x0 - 1, 12));
        };
        room(&mut world, 10, 36); // 26 wide: holds
        room(&mut world, 60, 110); // 50 wide: caves in
        for _ in 0..600 {
            world.step_caveins();
            world.step_water();
        }
        for x in 10..36 {
            assert_eq!(world.ty(world.idx(x, 11)), ROCK, "narrow room's ceiling fell at {x}");
        }
        let fallen = (60..110).filter(|&x| world.ty(world.idx(x, 11)) != ROCK).count();
        assert!(fallen >= 10, "only {fallen} ceiling cells of the wide room fell");
        // Near its walls the wide room's ceiling still holds: it collapses into an arch.
        assert_eq!(world.ty(world.idx(62, 11)), ROCK);
        assert_eq!(world.ty(world.idx(107, 11)), ROCK);
        assert!(count(&world, RUBBLE) > 0);
    }

    #[test]
    fn a_pillar_holds_a_wide_ceiling() {
        let mut world = flat(128, 64);
        for y in 9..12 {
            world.dig(world.idx(59, y));
        }
        // Build the pillar first, then dig the room around it, so it never stands unsupported.
        world.set_type(world.idx(85, 14), WALL);
        world.set_type(world.idx(85, 13), WALL);
        world.set_type(world.idx(85, 12), WALL);
        for x in 60..110 {
            for y in 12..15 {
                if world.ty(world.idx(x, y)) == ROCK {
                    world.dig(world.idx(x, y));
                }
            }
        }
        world.dig(world.idx(59, 12));
        for _ in 0..600 {
            world.step_caveins();
            world.step_water();
        }
        for x in 60..110 {
            assert_eq!(world.ty(world.idx(x, 11)), ROCK, "ceiling fell at {x} despite the pillar");
        }
    }

    #[test]
    fn rain_water_on_the_land_dries_up_but_lakes_and_holes_stay() {
        let mut world = flat(64, 64);
        // A puddle lying on the ground (above the surface line) and water in a dug-out hole.
        for x in 10..20 {
            world.add_water(x, 7);
        }
        for x in 40..44 {
            world.dig(world.idx(x, 8));
            world.set_type(world.idx(x, 8), WATER);
        }
        for _ in 0..3000 {
            world.step_caveins();
            world.evaporate();
            world.step_water();
        }
        assert_eq!((0..world.w).filter(|&x| world.ty(world.idx(x, 7)) == WATER).count(), 0);
        assert_eq!((40..44).filter(|&x| world.ty(world.idx(x, 8)) == WATER).count(), 4);
    }

    #[test]
    fn deeper_rock_is_harder_and_holds_smaller_rooms() {
        let hits: Vec<u8> = ROCK_LAYERS.iter().map(|&t| rock_state(t).unwrap().1).collect();
        assert_eq!(hits, [1, 3, 5, 8, 12]);
        assert_eq!(rock_state(DIRT), Some((0, 1)));
        assert_eq!(rock_state(SPONGE_ORE), None);
        // Every worn state maps back to its own kind and hits left, and the types do not overlap.
        let mut seen = HashSet::new();
        for (k, &hits) in KIND_HITS.iter().enumerate() {
            for left in 1..hits {
                assert_eq!(rock_state(worn(k, left)), Some((k, left)));
                assert!(seen.insert(worn(k, left)));
            }
        }
        assert!(seen.iter().all(|&t| t >= WORN_BASE && t < 64 && !ROCK_KINDS.contains(&t)));
        assert!(CAVE_SPANS.windows(2).all(|w| w[0] > w[1]));
        assert_eq!(CAVE_SPANS[0], CAVE_SPAN);
    }

    #[test]
    fn walls_see_and_reach_into_deep_water() {
        let mut world = flat(64, 64);
        // A flooded shaft, far deeper than players can see into water.
        for y in 8..40 {
            world.dig(world.idx(32, y));
            if y >= 12 {
                world.set_type(world.idx(32, y), WATER);
            }
        }
        world.update_sight();
        assert!(!world.is_revealed(world.idx(32, 30)), "deep water is dark");
        // A wall at the water's surface, then one under it, and one more: each next to the last.
        for y in [12, 13, 14] {
            let i = world.idx(32, y);
            assert!(world.is_revealed(i) && (world.reachable(i) || world.touches_wall(i)), "row {y}");
            world.set_type(i, WALL);
            world.update_sight();
        }
        assert!(world.is_revealed(world.idx(32, 15)), "the water under the wall is visible");
        assert!(world.touches_wall(world.idx(32, 15)));
        // The rock beside the wall shows too, but it is no place to build.
        assert!(world.is_revealed(world.idx(31, 14)));
        assert_eq!(world.ty(world.idx(31, 14)), ROCK);
    }

    #[test]
    fn falling_rubble_does_not_darken_the_shaft_below() {
        let mut world = flat(64, 64);
        for y in 8..40 {
            world.dig(world.idx(32, y));
        }
        world.set_type(world.idx(32, 9), RUBBLE_FALLING);
        world.step_water();
        world.update_sight();
        assert!(world.lit[world.idx(32, 39)], "the shaft bottom is still connected to the sky");
        for _ in 0..40 {
            world.step_water();
        }
        assert_eq!(world.ty(world.idx(32, 39)), RUBBLE);
        assert!(world.lit[world.idx(32, 38)]);
    }

    #[test]
    fn generated_world_has_water_and_spongerock_but_hides_them() {
        let world = World::generate(1024, 512, 42);
        let cells = (world.w * world.h) as f64;
        let water = count(&world, WATER) as f64 / cells;
        let ore: Vec<usize> = (0..world.w * world.h).filter(|&i| world.ty(i) == SPONGE_ORE).collect();
        println!("water {:.1}%, spongerock cells {}", water * 100.0, ore.len());
        for &i in &ore {
            let (x, y) = (i % world.w, i / world.w);
            assert!(y >= world.surface[x] as usize + SPONGE_DEPTH, "spongerock too shallow at ({x}, {y})");
        }
        assert!(water > 0.005 && water < 0.10);
        assert!(ore.len() > 100);
        let quartz: Vec<usize> = (0..world.w * world.h).filter(|&i| world.ty(i) == QUARTZ_ORE).collect();
        println!("quartz cells {}, amethyst cells {}", quartz.len(), count(&world, AMETHYST_ORE));
        assert!(count(&world, AMETHYST_ORE) > 20);
        println!("coral cells {}, emberstone cells {}", count(&world, CORAL_ORE), count(&world, EMBER));
        assert!(count(&world, CORAL_ORE) > 20 && count(&world, EMBER) > 100);
        // Layers blend: around 160 m both the first and the second layer's rock turn up; much deeper only the
        // deeper one does.
        let at = |depth: usize, t: u8| {
            (100..world.w - 100).filter(|&x| world.ty(world.idx(x, world.surface[x] as usize + depth)) == t).count()
        };
        assert!(at(LAYER_ROWS, ROCK) > 50 && at(LAYER_ROWS, ROCK_LAYERS[1]) > 50, "no blend at the boundary");
        assert_eq!(at(LAYER_ROWS + 40, ROCK), 0, "top layer rock far below its boundary");
        assert!(at(0, DIRT) + at(1, DIRT) > 1000, "a crust of dirt on top");
        println!("damp rock {}, pocket springs {}", count(&world, SPRING_ROCK), count(&world, SPRING));
        assert!(count(&world, SPRING_ROCK) > 5 && count(&world, SPRING) > 0);
        assert!(quartz.len() > 50);
        for &i in &quartz {
            let (x, y) = (i % world.w, i / world.w);
            assert!(y >= world.surface[x] as usize + QUARTZ_DEPTH, "quartz too shallow at ({x}, {y})");
        }
        // Nothing underground is visible at the start, so no gem or water value goes to clients.
        for i in 0..world.w * world.h {
            if i / world.w > 200 {
                assert_eq!(world.wire(i), 0);
            }
        }
    }

    #[test]
    fn surface_lakes_are_visible_and_still() {
        for seed in 0..20 {
            let mut world = World::generate(1024, 256, seed);
            let lake = (0..world.w * world.h).filter(|&i| world.ty(i) == WATER && world.is_revealed(i)).count();
            assert!(lake > 50, "seed {seed}: no visible lake water");
            // Water that touches visible air must be visible; only sealed pockets and deep water stay hidden.
            for i in 0..world.w * world.h {
                let (x, y) = ((i % world.w) as i64, (i / world.w) as i64);
                if world.ty(i) != WATER || world.is_revealed(i) {
                    continue;
                }
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    if let Some(n) = world.index_of(x + dx, y + dy) {
                        assert!(!world.lit_air(n), "seed {seed}: hidden water at ({x}, {y})");
                    }
                }
            }
            let before: Vec<u8> = (0..world.w * world.h).map(|i| world.ty(i)).collect();
            world.wake_all();
            for _ in 0..20 {
                world.step_water();
            }
            let moved = (0..world.w * world.h).filter(|&i| world.ty(i) != before[i]).count();
            assert_eq!(moved, 0, "seed {seed}: water moved in a fresh world");
        }
    }

    #[test]
    fn island_shape_and_endless_ocean() {
        for seed in 0..10 {
            let mut world = World::generate(1024, 512, seed);
            assert_eq!(world.sea_level, SEA_LEVEL, "seed {seed}");
            let mid = world.w / 2;
            // Sea, ramp up from the shore to the rim, down into the bowl.
            let rim = SHELF + RAMP;
            assert!((world.surface[rim] as f64) < RIM_ROW + 10.0, "seed {seed}: rim too low");
            assert!(world.surface[mid] > world.surface[rim] + 60, "seed {seed}: bowl too shallow");
            assert_eq!(world.ty(world.idx(mid, world.h - 1)), BEDROCK);
            // Scooping the sea is refilled from the ocean, and rain on it runs off.
            world.dig(world.idx(0, SEA_LEVEL));
            world.add_water(10, SEA_LEVEL - 1);
            world.tend_ocean();
            assert_eq!(world.ty(world.idx(0, SEA_LEVEL)), WATER);
            assert_eq!(world.ty(world.idx(10, SEA_LEVEL - 1)), AIR);
        }
    }

    /// A tunnel dug through the island's edge below sea level lets the ocean in, through every opening at once.
    #[test]
    fn a_breach_lets_the_ocean_in_through_every_opening() {
        let mut world = World::generate(1024, 512, 3);
        // A tunnel from the edge (under the seabed) far inland, 40 rows below sea level.
        let y = SEA_LEVEL + 40;
        for x in 0..400 {
            world.dig(world.idx(x, y));
        }
        let before = count(&world, WATER);
        for _ in 0..100 {
            world.tend_ocean();
            world.step_water();
        }
        let poured = count(&world, WATER) - before;
        assert!(poured > 20, "the ocean should pour in, only {poured} cells");
        // A second tunnel lower down: water comes in through both, not only the lowest.
        let low = y + 30;
        for x in 0..50 {
            world.dig(world.idx(x, low));
        }
        for _ in 0..30 {
            world.tend_ocean();
            world.step_water();
        }
        let wet = |world: &World, row: usize| (0..50).filter(|&x| world.ty(world.idx(x, row)) == WATER).count();
        assert!(wet(&world, low) > 5, "the lower breach pours too");
        assert!(wet(&world, y) > 5, "and the upper one still has water");
    }

    /// A big cave dug open at the island's edge fills up to sea level through its whole opening, in seconds.
    #[test]
    fn the_ocean_gushes_into_a_big_edge_cave() {
        let mut world = World::generate(1024, 512, 3);
        for y in SEA_LEVEL..SEA_LEVEL + 80 {
            for x in 0..60 {
                world.dig(world.idx(x, y));
            }
        }
        for _ in 0..300 {
            world.tend_ocean();
            world.step_water();
        }
        let water = (SEA_LEVEL + 1..SEA_LEVEL + 80)
            .flat_map(|y| (0..60).map(move |x| (x, y)))
            .filter(|&(x, y)| world.ty(world.idx(x, y)) == WATER)
            .count();
        assert!(water > 79 * 60 * 9 / 10, "only {water} of {} cells under sea level filled in 30 s", 79 * 60);
    }

    #[test]
    fn sealing_a_tunnel_makes_it_dark_and_out_of_reach() {
        let mut world = flat(64, 64);
        // A shaft down from the surface, with a side room at its bottom.
        for y in 8..20 {
            world.dig(world.idx(10, y));
        }
        for x in 11..16 {
            world.dig(world.idx(x, 19));
        }
        world.update_sight();
        let inside = world.idx(14, 20);
        assert!(world.reachable(inside));
        // Cap the shaft with a wall: everything below goes dark.
        world.set_type(world.idx(10, 8), WALL);
        world.update_sight();
        assert!(!world.reachable(inside));
        assert_eq!(world.wire(inside), 0);
        assert_eq!(world.wire(world.idx(12, 19)), 0, "the sealed room is hidden");
        // The wall itself can still be broken from outside, and that lights the tunnel again.
        let wall = world.idx(10, 8);
        assert!(world.reachable(wall));
        world.dig(wall);
        world.update_sight();
        assert!(world.reachable(inside));
    }

    #[test]
    fn a_wall_beside_an_open_tunnel_changes_nothing() {
        let mut world = flat(64, 64);
        for x in 10..20 {
            for y in 8..10 {
                world.dig(world.idx(x, y));
            }
        }
        world.set_type(world.idx(15, 9), WALL);
        world.update_sight();
        assert!(world.reachable(world.idx(18, 10)));
    }

    #[test]
    fn digging_reveals_neighbours_and_shadows_behind() {
        let mut world = flat(64, 64);
        let i = world.idx(10, 8);
        assert!(world.is_revealed(i));
        assert_eq!(world.wire(world.idx(10, 9)) & VIS_MASK, SHADOW);
        assert_eq!(world.wire(world.idx(10, 11)), 0);
        world.dig(i);
        world.update_sight();
        assert!(world.is_revealed(world.idx(10, 9)));
        assert_eq!(world.wire(world.idx(10, 11)) & VIS_MASK, SHADOW);
    }

    #[test]
    fn sight_reaches_only_a_few_cells_into_water() {
        let mut world = flat(64, 64);
        for y in 12..20 {
            for x in 20..30 {
                let i = world.idx(x, y);
                world.cells[i] = WATER;
            }
        }
        // Dig a shaft down to the top of the pocket.
        for y in 8..12 {
            world.dig(world.idx(20, y));
        }
        world.update_sight();
        // Four cells of water are visible below the shaft, the fifth is not, not even as shadow.
        assert!(world.is_revealed(world.idx(20, 15)));
        assert_eq!(world.wire(world.idx(20, 16)), 0);
        assert_eq!(world.wire(world.idx(29, 19)), 0);
        // Only the top water cell touches air, so only it can be scooped.
        assert!(world.reachable(world.idx(20, 12)));
        assert!(!world.reachable(world.idx(20, 13)));
        assert!(!world.reachable(world.idx(21, 12)));
        // Scooping the top cell lets the sight go one deeper.
        world.dig(world.idx(20, 12));
        world.update_sight();
        assert!(world.is_revealed(world.idx(20, 16)));
    }

    #[test]
    fn flooding_a_shaft_hides_its_bottom_again() {
        let mut world = flat(64, 64);
        for y in 8..30 {
            world.dig(world.idx(10, y));
        }
        world.update_sight();
        let bottom = world.idx(10, 30);
        assert!(world.is_revealed(bottom));
        assert!(world.is_revealed(world.idx(10, 25)));
        // Fill the shaft with water from row 12 down: only the top 4 water cells stay visible.
        for y in 12..30 {
            world.set_type(world.idx(10, y), WATER);
        }
        world.update_sight();
        assert!(world.is_revealed(world.idx(10, 15)));
        assert_eq!(world.wire(world.idx(10, 16)), 0);
        assert_eq!(world.wire(world.idx(10, 25)), 0);
        assert_eq!(world.wire(bottom), 0);
        // Rock beside the shaft near the top is still visible, but deep down it is gone too.
        assert!(world.is_revealed(world.idx(11, 13)));
        assert_eq!(world.wire(world.idx(11, 25)), 0);
        assert_eq!(world.take_dirty().iter().filter(|&&i| i as usize == bottom).count(), 1, "clients hear about it");
    }

    #[test]
    fn a_pocket_behind_a_thin_wall_shows_as_shadow() {
        let mut world = flat(64, 64);
        for y in 10..14 {
            for x in 22..30 {
                let i = world.idx(x, y);
                world.cells[i] = WATER;
            }
        }
        // A tunnel at rows 10-13 ending at x 20: one cell of rock (x 21) between it and the pocket.
        for y in 8..14 {
            world.dig(world.idx(15, y));
        }
        for x in 16..=20 {
            world.dig(world.idx(x, 12));
        }
        world.update_sight();
        assert!(world.is_revealed(world.idx(21, 12)));
        assert_eq!(world.wire(world.idx(22, 12)), WATER | SHADOW);
        assert_eq!(world.wire(world.idx(25, 12)), 0);
    }

    #[test]
    fn lake_bed_is_out_of_reach() {
        let mut world = flat(64, 64);
        // A lake 3 deep in a dug basin; the bed under it is visible but cannot be dug.
        for y in 8..12 {
            for x in 10..30 {
                world.dig(world.idx(x, y));
            }
        }
        for y in 9..12 {
            for x in 10..30 {
                world.set_type(world.idx(x, y), WATER);
            }
        }
        world.update_sight();
        let bed = world.idx(20, 12);
        assert!(world.is_revealed(bed));
        assert!(!world.reachable(bed));
        assert!(world.reachable(world.idx(20, 9)));
        assert!(!world.reachable(world.idx(20, 10)));
    }

    /// Two shafts joined at the bottom: water poured into one rises in the other to the same level.
    #[test]
    fn connected_shafts_level_out() {
        let mut world = flat(64, 64);
        for y in 8..40 {
            world.dig(world.idx(10, y));
            world.dig(world.idx(30, y));
        }
        for x in 10..=30 {
            world.dig(world.idx(x, 39));
        }
        for y in 14..40 {
            world.set_type(world.idx(10, y), WATER);
        }
        for x in 11..=30 {
            world.set_type(world.idx(x, 39), WATER);
        }
        let water = count(&world, WATER);
        for _ in 0..2000 {
            world.step_water();
        }
        assert_eq!(count(&world, WATER), water);
        let top = |world: &World, x: usize| (0..64).find(|&y| world.ty(world.idx(x, y)) == WATER).unwrap();
        let (left, right) = (top(&world, 10), top(&world, 30));
        assert!(left.abs_diff(right) <= 1, "left shaft level {left}, right shaft level {right}");
        assert!(world.next_active.iter().all(|&a| !a), "levelled water should sleep");
    }

    /// Air caught under a ceiling next to higher water gets filled.
    #[test]
    fn trapped_air_gets_filled() {
        let mut world = flat(64, 64);
        // A tall shaft full of water, with a side tunnel at its bottom that ends in a dead end.
        for y in 8..30 {
            world.dig(world.idx(10, y));
        }
        for x in 11..20 {
            world.dig(world.idx(x, 29));
        }
        for y in 15..30 {
            world.set_type(world.idx(10, y), WATER);
        }
        for _ in 0..500 {
            world.step_water();
        }
        for x in 11..20 {
            assert_eq!(world.ty(world.idx(x, 29)), WATER, "air left in the tunnel at x {x}");
        }
    }

    #[test]
    fn water_is_conserved_and_settles() {
        let mut world = flat(128, 128);
        // A basin 40 wide and 20 deep, with water dropped in from above.
        for y in 8..28 {
            for x in 30..70 {
                world.dig(world.idx(x, y));
            }
        }
        for x in 40..60 {
            for y in 0..10 {
                world.add_water(x, y);
            }
        }
        let water = count(&world, WATER);
        for _ in 0..1000 {
            world.step_water();
            assert_eq!(count(&world, WATER), water);
        }
        assert!(world.next_active.iter().all(|&a| !a), "settled water should let its chunks sleep");
        // 200 cells in a basin 40 wide: five rows at the bottom. The surface levels out to within one row
        // (pressure only moves water two rows down or more), so a stray cell may sit on top of a gap.
        for x in 30..70 {
            for y in 24..28 {
                assert_eq!(world.ty(world.idx(x, y)), WATER, "row {y}, x {x}");
            }
        }
        let high = (0..world.w * world.h).filter(|&i| world.ty(i) == WATER && i / world.w < 22).count();
        assert_eq!(high, 0, "water left above the top two rows");
    }

    #[test]
    fn a_diagonal_wall_holds_water() {
        let mut world = flat(64, 64);
        for y in 8..30 {
            for x in 10..40 {
                world.dig(world.idx(x, y));
            }
        }
        // A staircase wall from (20, 29) up to (29, 20); water starts on its left.
        for k in 0..10 {
            world.set_type(world.idx(20 + k, 29 - k), WALL);
        }
        for y in 22..30 {
            for x in 10..20 {
                world.add_water(x, y);
            }
        }
        for _ in 0..500 {
            world.step_water();
        }
        for y in 8..30 {
            for x in 31..40 {
                assert_ne!(world.ty(world.idx(x, y)), WATER, "water leaked to ({x}, {y})");
            }
        }
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::{Duration, Instant};

    /// cargo test --release flood_benchmark -- --ignored --nocapture
    #[test]
    #[ignore]
    fn flood_benchmark() {
        let mut world = World::generate(1024, 1024, 7);
        // Tunnels across the whole world at three depths, breaking into every pocket on the way.
        for y in [120, 121, 300, 301, 600, 601] {
            for x in 0..world.w {
                let i = world.idx(x, y);
                world.set_type(i, AIR);
            }
        }
        world.update_sight();
        world.take_dirty();
        let (mut total, mut worst, mut worst_tick, mut moving_ticks) = (Duration::ZERO, Duration::ZERO, 0, 0);
        for tick in 0..600 {
            let t = Instant::now();
            world.step_water();
            world.update_sight();
            let took = t.elapsed();
            total += took;
            if took > worst {
                worst = took;
                worst_tick = tick;
            }
            let changed = world.take_dirty().len();
            if changed > 0 {
                moving_ticks = tick;
            }
        }
        println!(
            "600 ticks: avg {:?}, worst {:?} (tick {worst_tick}), visible water still moving at tick {moving_ticks}",
            total / 600,
            worst
        );
    }
}

#[cfg(test)]
mod pocket_stats {
    use super::*;

    /// cargo test --release pocket_sizes -- --ignored --nocapture
    /// Also writes the world to target/preview-world.bin, for `node tools/map.mjs 1 server/target/preview-world.bin`.
    #[test]
    #[ignore]
    fn pocket_sizes() {
        let started = std::time::Instant::now();
        let world = World::generate(2048, 1024, 7);
        println!("generated 2048x1024 in {:?}", started.elapsed());
        write_atomic(Path::new("target/preview-world.bin"), &world.to_bytes()).unwrap();
        let (w, h) = (world.w, world.h);
        let mut seen = vec![false; w * h];
        // Per band of 256 rows: number of pockets, total cells, biggest pocket.
        let mut bands = [(0usize, 0usize, 0usize); 4];
        for start in 0..w * h {
            if seen[start] || world.ty(start) != WATER || world.is_revealed(start) {
                continue;
            }
            let mut stack = vec![start];
            seen[start] = true;
            let mut size = 0;
            while let Some(i) = stack.pop() {
                size += 1;
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    if let Some(n) = world.index_of(x + dx, y + dy) {
                        if !seen[n] && world.ty(n) == WATER {
                            seen[n] = true;
                            stack.push(n);
                        }
                    }
                }
            }
            let band = &mut bands[(start / w * 4 / h).min(3)];
            band.0 += 1;
            band.1 += size;
            band.2 = band.2.max(size);
        }
        for (k, (count, cells, biggest)) in bands.iter().enumerate() {
            println!(
                "rows {:4}-{:4}: {count:4} pockets, average {:4} cells, biggest {biggest:5}, {:.1}% water",
                k * h / 4,
                (k + 1) * h / 4,
                cells / count.max(&1),
                *cells as f64 * 100.0 / (w * h / 4) as f64
            );
        }
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn a_drained_sea_fills_back_to_sea_level() {
        let mut world = World::generate(1024, 512, 5);
        for x in 0..SHELF {
            world.dig(world.idx(x, SEA_LEVEL));
            world.dig(world.idx(x, SEA_LEVEL + 1));
        }
        for _ in 0..200 {
            world.tend_ocean();
            world.step_water();
        }
        for x in 0..SHELF {
            assert_eq!(world.ty(world.idx(x, SEA_LEVEL)), WATER, "sea not refilled at x {x}");
        }
    }

    #[test]
    fn placed_spongerock_soaks_up_water() {
        let mut world = World::generate(1024, 256, 2);
        let mid = world.w / 2;
        let y = world.surface[mid] as usize + 20;
        // A pool of 8 water cells with a sponge in the middle of its floor.
        for x in mid - 4..mid + 4 {
            world.set_type(world.idx(x, y), WATER);
        }
        world.set_type(world.idx(mid, y + 1), SPONGE);
        let water = |w: &World| (mid - 4..mid + 4).filter(|&x| w.ty(w.idx(x, y)) == WATER).count();
        world.absorb();
        assert_eq!(water(&world), 7);
        // Removing the sponge stops it.
        world.set_type(world.idx(mid, y + 1), AIR);
        world.absorb();
        assert_eq!(water(&world), 7);
    }

    #[test]
    fn a_dry_tunnel_under_the_sea_stays_dry() {
        let mut world = World::generate(1024, 512, 5);
        // A tunnel under the seabed, sealed off from the sea and from the edge by rock.
        let y = SEA_LEVEL + SEA_DEPTH as usize;
        for x in 5..SHELF + 20 {
            world.dig(world.idx(x, y));
        }
        for _ in 0..100 {
            world.tend_ocean();
            world.step_water();
        }
        assert!((5..SHELF + 20).all(|x| world.ty(world.idx(x, y)) == AIR), "ocean water got through the rock");
    }

    #[test]
    fn walls_in_the_sky_need_support() {
        let mut cells = vec![ROCK; 64 * 64];
        cells[..8 * 64].fill(AIR);
        let world = World::from_parts(64, 64, 1, vec![8; 64], cells);
        assert!(!world.supported(world.idx(10, 3)), "floating in the sky");
        assert!(world.supported(world.idx(10, 7)), "resting on the ground");
    }
}
