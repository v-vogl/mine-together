//! The Upwelling: an event that adds to the world instead of taking from it. Molten veins grow up from the
//! bottom of the world, branching into a web that thins out towards the top: the deep layers get many veins,
//! the stone near the surface one or two. One main vein always reaches the surface and erupts there; the
//! magma it pours out cools into lava rock and builds a volcano. Where a vein breaks into a tunnel it spews
//! magma for a while. Afterwards the veins cool into lava rock with materials scattered along them.
//! The cataclysm (a console command, for testing and special occasions) is the same with many times the veins
//! and several main veins, each building a supervolcano.

use super::*;

/// Every vein tip grows one cell per tick: the main vein crosses a 900-row world in about 90 seconds.
/// A branch may fork with a chance of one in FORK_CHANCE per cell, while there are fewer than MAX_TIPS tips.
const FORK_CHANCE: u64 = 14;
const MAX_TIPS: usize = 48;
/// A vein that breaks into a tunnel spews magma: one cell every VENT_EVERY ticks, for VENT_TICKS ticks
/// (scaled by the strength).
const VENT_EVERY: u64 = 5;
const VENT_TICKS: f64 = 400.0;
/// The volcano pours this many magma cells per tick at full strength, for ERUPT_TICKS ticks (both scaled by
/// the strength, at least half of it). One pour in BOMB_CHANCE is thrown up into the air as a lava bomb, up
/// to BOMB_REACH cells away, and rains down on the slopes.
const ERUPT_RATE: f64 = 2.5;
const BOMB_CHANCE: u64 = 3;
const BOMB_REACH: i64 = 14;
/// A volcano buried in its own magma pushes new magma out through it, to an open cell at most this many
/// magma cells away: the pool on top overflows and the cone grows wide, not into a thin spire.
const OVERFLOW_SEARCH: usize = 400;
const ERUPT_TICKS: f64 = 1800.0;
/// After the event the veins cool, about this many ticks in all.
const COOL_TICKS: usize = 600;

struct Tip {
    x: i64,
    y: i64,
    /// Cells left to grow (a main vein grows until it reaches the surface).
    life: i64,
    main: bool,
    /// Column a main vein heads for.
    target: i64,
}

struct Vent {
    i: usize,
    left: u64,
    /// The volcano at the surface: it climbs up through its own cooled rock, so the cone grows.
    volcano: bool,
}

pub struct Upwelling {
    tips: Vec<Tip>,
    vents: Vec<Vent>,
    /// Vein cells still molten.
    molten: Vec<usize>,
    strength: f64,
    /// 1 for an Upwelling; bigger for a cataclysm: volcanoes pour this many times as fast, for longer, and
    /// throw their lava bombs further.
    size: f64,
    max_tips: usize,
    erupted: bool,
    announced: bool,
    /// Vein cells cooled per tick, fixed when the cooling starts so it takes about COOL_TICKS.
    cool_per_tick: usize,
    /// The volcano's magma carried over between ticks.
    carry: f64,
}

/// What happened in an Upwelling this tick, for the announcements.
#[derive(Debug, PartialEq)]
pub enum UpwellingNews {
    Erupted,
    Over,
}

/// The phase of an Upwelling, as sent to clients: 2 veins rising, 3 erupting, 4 cooling (0 none, 1 stirring
/// is the warning before it, kept by the simulation).
impl Upwelling {
    fn phase(&self) -> u8 {
        if self.vents.iter().any(|v| v.volcano) {
            3
        } else if !self.tips.is_empty() || !self.vents.is_empty() {
            2
        } else {
            4
        }
    }
}

impl World {
    /// Start an Upwelling. `strength` from 0.4 (alone) to 1 (a crowd) sets how many roots it has, how long
    /// vents spew and how big the volcano grows.
    pub fn start_upwelling(&mut self, strength: f64) {
        self.start_eruption(strength, 1, 1, 1.0);
    }

    /// Start a cataclysm: four times the roots, five main veins, supervolcanoes three times the size.
    pub fn start_cataclysm(&mut self) {
        self.start_eruption(1.0, 4, 5, 3.0);
    }

    /// `roots` times the usual roots and fork limit, `volcanoes` main veins spread over the island, `size`
    /// see Upwelling::size.
    fn start_eruption(&mut self, strength: f64, roots: usize, volcanoes: usize, size: f64) {
        let margin = ((SHELF + RAMP) as i64).min(self.w as i64 / 4);
        let span = (self.w as i64 - 2 * margin).max(1);
        let h = self.h as i64;
        let mut tips = Vec::new();
        // Main veins: their columns spread evenly over the island, each starting below its own column.
        for k in 0..volcanoes as i64 {
            let slot = span / volcanoes as i64;
            let target = margin + slot * k + self.rng.below(slot.max(1) as u64) as i64;
            let x = (target + self.rng.below(81) as i64 - 40).clamp(1, self.w as i64 - 2);
            tips.push(Tip { x, y: h - 3, life: i64::MAX, main: true, target });
        }
        for _ in 0..roots * (2.0 + 5.0 * strength).round() as usize {
            let x = margin + self.rng.below(span as u64) as i64;
            let life = h * (3 + self.rng.below(5) as i64) / 10;
            tips.push(Tip { x, y: h - 3, life, main: false, target: x });
        }
        self.upwelling = Some(Upwelling {
            tips,
            vents: Vec::new(),
            molten: Vec::new(),
            strength,
            size,
            max_tips: MAX_TIPS * roots,
            erupted: false,
            announced: false,
            cool_per_tick: 0,
            carry: 0.0,
        });
    }

    /// 0 when no Upwelling is going on, else its phase (see Upwelling::phase).
    pub fn upwelling_phase(&self) -> u8 {
        self.upwelling.as_ref().map_or(0, |u| u.phase())
    }

    /// Vein tips growing, vents pouring (the volcano among them) and vein cells still molten, for the log.
    pub fn upwelling_counts(&self) -> (usize, usize, usize) {
        self.upwelling.as_ref().map_or((0, 0, 0), |u| (u.tips.len(), u.vents.len(), u.molten.len()))
    }

    /// One tick of the Upwelling, if one is going on.
    pub fn step_upwelling(&mut self) -> Option<UpwellingNews> {
        let mut up = self.upwelling.take()?;
        let mut news = None;
        self.grow_veins(&mut up);
        if up.erupted && !up.announced {
            up.announced = true;
            news = Some(UpwellingNews::Erupted);
        }
        self.pour_vents(&mut up);
        if up.tips.is_empty() && up.vents.is_empty() {
            // Cool the veins, a few cells each tick.
            if up.cool_per_tick == 0 {
                up.cool_per_tick = up.molten.len() / COOL_TICKS + 1;
            }
            for _ in 0..up.cool_per_tick {
                let Some(i) = up.molten.pop() else { break };
                if self.ty(i) == MOLTEN {
                    let cooled = self.cooled_vein(i);
                    self.set_type(i, cooled);
                }
            }
            if up.molten.is_empty() {
                return Some(UpwellingNews::Over);
            }
        }
        self.upwelling = Some(up);
        news
    }

    fn erupt_ticks(&self, up: &Upwelling) -> u64 {
        (ERUPT_TICKS * (0.5 + 0.5 * up.strength) * (1.0 + up.size) / 2.0) as u64
    }

    fn grow_veins(&mut self, up: &mut Upwelling) {
        let mut k = 0;
        while k < up.tips.len() {
            if !self.grow_tip(up, k) {
                up.tips.swap_remove(k);
            } else {
                k += 1;
            }
        }
    }

    /// Grow tip `k` by one cell. False when the tip is done.
    fn grow_tip(&mut self, up: &mut Upwelling, k: usize) -> bool {
        let Tip { x, y, main, target, .. } = up.tips[k];
        let here = y as usize * self.w + x as usize;
        // Mostly upwards, wandering to the sides; a main vein steers towards its column.
        let drift = if main && self.rng.below(3) == 0 {
            (target - x).signum()
        } else {
            [-1, 0, 0, 1][self.rng.below(4) as usize]
        };
        let sideways = self.rng.below(4) == 0 && drift != 0;
        let nx = (x + drift).clamp(1, self.w as i64 - 2);
        let ny = if sideways { y } else { y - 1 };
        if ny < 0 {
            return false;
        }
        let i = ny as usize * self.w + nx as usize;
        let t = self.ty(i);
        if t == BEDROCK {
            return true;
        }
        // Through another vein: nothing changes.
        if t == MOLTEN {
            up.tips[k].x = nx;
            up.tips[k].y = ny;
            return true;
        }
        if is_wall(t) {
            self.scorch(i);
            return true;
        }
        if is_open(t) {
            if !main {
                // A branch breaks into a tunnel: it spews magma there for a while, then it is done.
                let left = (VENT_TICKS * up.strength) as u64;
                up.vents.push(Vent { i: here, left, volcano: false });
                return false;
            }
            if (ny as usize) < self.surface[nx as usize] as usize {
                // The main vein reaches the sky: the eruption begins.
                up.erupted = true;
                let left = self.erupt_ticks(up);
                up.vents.push(Vent { i: here, left, volcano: true });
                return false;
            }
            // The main vein crossing a tunnel spews into it, and goes on from the other side.
            if !up.vents.iter().any(|v| v.i == here) {
                up.vents.push(Vent { i: here, left: (VENT_TICKS * up.strength) as u64 / 2, volcano: false });
            }
            let mut cy = ny;
            while cy > 0 && is_open(self.ty(cy as usize * self.w + nx as usize)) {
                if (cy as usize) < self.surface[nx as usize] as usize {
                    up.erupted = true;
                    let left = self.erupt_ticks(up);
                    up.vents.push(Vent { i: here, left, volcano: true });
                    return false;
                }
                cy -= 1;
            }
            up.tips[k].x = nx;
            up.tips[k].y = cy + 1;
            return true;
        }
        self.set_type(i, MOLTEN);
        up.molten.push(i);
        let tip = &mut up.tips[k];
        tip.x = nx;
        tip.y = ny;
        if !tip.main {
            tip.life -= 1;
            if tip.life <= 0 {
                return false;
            }
        }
        // Fork: a side branch with part of what is left of this one (a branch of the main vein gets a short life).
        if up.tips.len() < up.max_tips && self.rng.below(FORK_CHANCE) == 0 {
            let parent = &up.tips[k];
            let base = if parent.main { self.h as i64 / 5 } else { parent.life };
            let life = base * (3 + self.rng.below(5) as i64) / 10;
            if life > 0 {
                up.tips.push(Tip { x: nx, y: ny, life, main: false, target: nx });
            }
        }
        true
    }

    fn pour_vents(&mut self, up: &mut Upwelling) {
        let mut k = 0;
        while k < up.vents.len() {
            let volcano = up.vents[k].volcano;
            let pours = if volcano {
                up.carry += ERUPT_RATE * (0.5 + 0.5 * up.strength) * up.size;
                let n = up.carry as u32;
                up.carry -= n as f64;
                n
            } else {
                ((self.age + k as u64) % VENT_EVERY == 0) as u32
            };
            for _ in 0..pours {
                self.pour(&mut up.vents[k], &mut up.molten, up.size);
            }
            let vent = &mut up.vents[k];
            vent.left = vent.left.saturating_sub(1);
            if vent.left == 0 {
                up.vents.swap_remove(k);
            } else {
                k += 1;
            }
        }
    }

    /// One cell of magma out of a vent, into the open space next to it (upwards first). Water next to it
    /// boils instead. A volcano also throws lava bombs, pushes magma out through the magma pooled on it, and
    /// once buried under its own cooled rock, climbs up through it.
    fn pour(&mut self, vent: &mut Vent, molten: &mut Vec<usize>, size: f64) {
        let (x, y) = ((vent.i % self.w) as i64, (vent.i / self.w) as i64);
        if vent.volcano && self.rng.below(BOMB_CHANCE) == 0 {
            let reach = (BOMB_REACH as f64 * size.sqrt()) as i64;
            let dx = self.rng.below(2 * reach as u64 + 1) as i64 - reach;
            let dy = -2 - self.rng.below(reach as u64) as i64;
            if let Some(i) = self.index_of(x + dx, y + dy) {
                if self.ty(i) == AIR {
                    return self.set_type(i, MAGMA);
                }
            }
        }
        let d = if self.rng.next() & 1 == 0 { -1 } else { 1 };
        for (dx, dy) in [(0, -1), (d, 0), (-d, 0), (0, 1)] {
            let Some(n) = self.index_of(x + dx, y + dy) else { continue };
            match self.ty(n) {
                AIR | STEAM => return self.set_type(n, MAGMA),
                WATER => return self.set_type(n, STEAM),
                _ => {}
            }
        }
        if !vent.volcano {
            return;
        }
        if let Some(n) = self.air_through_magma(vent.i, (OVERFLOW_SEARCH as f64 * size) as usize) {
            return self.set_type(n, MAGMA);
        }
        let Some(above) = self.index_of(x, y - 1) else { return };
        if matches!(self.ty(above), MAGMA | LAVA_ROCK | LAVA_ROCK_CHIPPED) {
            self.set_type(above, MOLTEN);
            molten.push(above);
            vent.i = above;
        }
    }

    /// The nearest air cell reached through the magma around `start`, looking at most `limit` cells.
    fn air_through_magma(&mut self, start: usize, limit: usize) -> Option<usize> {
        let round = self.next_visit_round();
        self.visit[start] = round;
        let mut queue = std::collections::VecDeque::from([start]);
        let mut seen = 0;
        while let Some(c) = queue.pop_front() {
            seen += 1;
            if seen > limit {
                return None;
            }
            let next: Vec<usize> = self.neighbours(c).filter(|&n| self.visit[n] != round).collect();
            for n in next {
                self.visit[n] = round;
                match self.ty(n) {
                    AIR | STEAM => return Some(n),
                    MAGMA => queue.push_back(n),
                    _ => {}
                }
            }
        }
        None
    }

    /// What a vein cell cools into: mostly lava rock, now and then a material of its depth, and very rarely,
    /// deep down, a fire opal.
    fn cooled_vein(&mut self, i: usize) -> u8 {
        let (x, y) = (i % self.w, i / self.w);
        let depth = y.saturating_sub(self.surface[x] as usize);
        match self.rng.below(1000) {
            r if r < 2 && depth >= 300 => FIRE_OPAL,
            r if r < 25 && depth >= AMETHYST_DEPTH => AMETHYST_ORE,
            r if r < 55 && depth >= QUARTZ_DEPTH => QUARTZ_ORE,
            r if r < 85 && depth >= SPONGE_DEPTH => SPONGE_ORE,
            r if r < 130 && depth >= EMBER_DEPTH => EMBER,
            _ => LAVA_ROCK,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(world: &World, t: u8) -> usize {
        (0..world.w * world.h).filter(|&i| world.ty(i) == t).count()
    }

    #[test]
    fn an_upwelling_reaches_the_surface_builds_a_volcano_and_cools() {
        // Air above row 20, rock below, with a tunnel across at row 120.
        let (w, h) = (128, 256);
        let mut cells = vec![ROCK; w * h];
        cells[..20 * w].fill(AIR);
        cells[(h - 2) * w..].fill(BEDROCK);
        let mut world = World::from_parts(w, h, 7, vec![20; w], cells);
        for x in 2..w - 2 {
            world.dig(world.idx(x, 120));
        }
        world.relight_all();
        world.start_upwelling(1.0);
        let mut erupted = false;
        let mut over = false;
        for _ in 0..20_000 {
            match world.step_upwelling() {
                Some(UpwellingNews::Erupted) => erupted = true,
                Some(UpwellingNews::Over) => {
                    over = true;
                    break;
                }
                None => {}
            }
            world.step_water();
        }
        assert!(erupted, "the main vein reaches the surface");
        assert!(over, "the Upwelling ends");
        assert_eq!(count(&world, MOLTEN), 0, "every vein has cooled");
        // The volcano: lava rock above the old surface.
        let cone = (0..20 * w).filter(|&i| matches!(world.ty(i), LAVA_ROCK | MAGMA)).count();
        assert!(cone > 50, "only {cone} cells of volcano");
        // The veins left lava rock deep down.
        let deep = (150 * w..(h - 2) * w).filter(|&i| world.ty(i) == LAVA_ROCK).count();
        assert!(deep > 100, "only {deep} cells of cooled vein deep down");
    }

    /// cargo test --release upwelling_timing -- --ignored --nocapture
    #[test]
    #[ignore]
    fn upwelling_timing() {
        let mut world = World::generate(2048, 1024, 5);
        if std::env::var("CATACLYSM").is_ok() {
            world.start_cataclysm();
        } else {
            world.start_upwelling(0.4);
        }
        let mut last_phase = 0;
        let start = std::time::Instant::now();
        let (mut worst, mut ticks) = (std::time::Duration::ZERO, 0);
        loop {
            let t = std::time::Instant::now();
            let news = world.step_upwelling();
            world.step_water();
            world.step_caveins();
            world.update_sight();
            worst = worst.max(t.elapsed());
            if world.upwelling_phase() != last_phase {
                last_phase = world.upwelling_phase();
                let u = world.upwelling.as_ref();
                println!("tick {ticks}: phase {last_phase}, tips {}, vents {}", u.map_or(0, |u| u.tips.len()), u.map_or(0, |u| u.vents.len()));
            }
            ticks += 1;
            if let Some(n) = news {
                println!("tick {ticks}: {n:?}");
                if n == UpwellingNews::Over {
                    break;
                }
            }
        }
        let cone = (0..world.w * world.h)
            .filter(|&i| i / world.w < world.surface[i % world.w] as usize && matches!(world.ty(i), LAVA_ROCK | MAGMA))
            .count();
        let height = (0..world.w)
            .filter_map(|x| (0..world.surface[x] as usize).find(|&y| world.ty(y * world.w + x) != AIR).map(|y| world.surface[x] as usize - y))
            .max()
            .unwrap_or(0);
        println!("volcano height {height}");
        let profile: Vec<usize> = (0..world.w).step_by(16).map(|x| (0..world.surface[x] as usize).find(|&y| world.ty(y * world.w + x) != AIR).map_or(0, |y| world.surface[x] as usize - y)).collect();
        println!("heights above the old surface every 16 columns: {profile:?}");
        let peak = (0..world.w).max_by_key(|&x| (0..world.surface[x] as usize).find(|&y| world.ty(y * world.w + x) != AIR).map_or(0, |y| world.surface[x] as usize - y)).unwrap();
        let top = (0..world.h).find(|&y| world.ty(y * world.w + peak) != AIR).unwrap();
        for y in (top..top + height + 6).step_by(3) {
            let row: String = (peak.saturating_sub(90)..(peak + 90).min(world.w)).step_by(2).map(|x| match world.ty(y * world.w + x) {
                AIR => ' ', LAVA_ROCK | LAVA_ROCK_CHIPPED => '#', MAGMA => 'm', MOLTEN => 'M', WATER => '~', _ => '.',
            }).collect();
            println!("{row}");
        }
        println!("{ticks} ticks, {:?} per tick, worst {worst:?}; volcano {cone} cells, lava rock {}, fire opal {}",
            start.elapsed() / ticks, count(&world, LAVA_ROCK), count(&world, FIRE_OPAL));
    }

    #[test]
    fn water_stops_magma_and_heat_breaks_walls() {
        let (w, h) = (64, 64);
        let mut cells = vec![ROCK; w * h];
        cells[..8 * w].fill(AIR);
        let mut world = World::from_parts(w, h, 3, vec![8; w], cells);
        world.relight_all();
        // A room at rows 20-24; magma in it, a wall in the middle, and water poured on the left part.
        for y in 20..25 {
            for x in 10..30 {
                world.dig(world.idx(x, y));
            }
        }
        for y in 20..25 {
            world.set_type(world.idx(20, y), WALL);
            world.set_type(world.idx(21, y), MAGMA);
        }
        world.set_type(world.idx(12, 24), MAGMA);
        world.set_type(world.idx(12, 23), WATER);
        for _ in 0..5 {
            world.step_water();
        }
        assert_eq!(world.ty(world.idx(12, 24)), LAVA_ROCK, "water hardens magma at once");
        assert!(count(&world, STEAM) > 0 || count(&world, WATER) < 1, "and boils away");
        for _ in 0..3000 {
            world.step_water();
        }
        let walls = (20..25).filter(|&y| world.ty(world.idx(20, y)) == WALL).count();
        assert!(walls < 5, "the heat breaks the wall next to the magma");
    }
}
