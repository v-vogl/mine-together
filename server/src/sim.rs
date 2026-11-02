//! The simulation thread. It alone owns the world and the players; network tasks talk to it through
//! a command queue, and it sends each client its messages through that client's outbox.

use crate::noise::Rng;
use crate::protocol::{self, put_u16, put_u32, ActResult, ClientMsg};
use crate::world::{self, World, CHUNK};
use axum::body::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;

const TICK: Duration = Duration::from_millis(100);
const COOLDOWN_DIG: Duration = Duration::from_millis(1500);
const COOLDOWN_COLLECT: Duration = Duration::from_millis(2500);
const COOLDOWN_WALL: Duration = Duration::from_millis(4000);
const COOLDOWN_PLACE: Duration = Duration::from_millis(3000);
const WALL_COST: u32 = 3;
/// Repairing a cracked wall costs this much rock and takes the dig cooldown, so one defender can hold
/// a wall against one attacker, but not against a group.
const REPAIR_COST: u32 = 1;
/// Rubble, dropped to fill flooded shafts, is cheap so a crowd can plug a leak.
const RUBBLE_COST: u32 = 1;
/// New players cannot build walls; unlocking them costs this much rock.
const WALLS_UNLOCK_COST: u32 = 10;
/// Pickaxe upgrades: level n digs (1 + n * PICKAXE_STEP) times as fast. Going from level n to n + 1
/// costs n + 1 quartz.
const PICKAXE_MAX: u8 = 5;
const PICKAXE_STEP: f64 = 0.1;
/// Player actions get faster with few players and slower with many: speed = 2 - 0.5 * log10(players),
/// so 1 player x2, 10 players x1.5, 100 players x1 (the cooldowns above), 300 players x0.76, and
/// never outside these bounds.
const SPEED_MIN: f64 = 0.5;
const SPEED_MAX: f64 = 2.0;
/// Placed spongestone soaks up a neighbouring water cell this often.
const ABSORB_EVERY_TICKS: u64 = 20;

const STATS_EVERY_TICKS: u64 = 10;
/// While players are connected, print tick time and bandwidth this often.
const REPORT_EVERY_TICKS: u64 = 100;
const SAVE_EVERY_TICKS: u64 = 600;
/// A rain shower lasts this many ticks, and the next one comes after 10 to 20 minutes.
const RAIN_TICKS: u32 = 200;
/// Rain strengths (light, normal, heavy) as drops per tick per 128 cells of world width (for 100 or more
/// players, see rain_amount), how likely each one is in percent, and how many players must be online for
/// it to come at all: a heavy shower would drown a handful of players, while a crowd can bail it out.
const RAIN_DROPS: [f64; 3] = [0.7, 1.0, 2.0];
const RAIN_CHANCE: [u64; 3] = [30, 50, 20];
const RAIN_MIN_PLAYERS: [usize; 3] = [0, 10, 50];
const RAIN_NAMES: [&str; 3] = ["light", "normal", "heavy"];
const RAIN_GAP_MIN_TICKS: u64 = 6000;
const RAIN_GAP_RANDOM_TICKS: u64 = 6000;

/// How much of a shower's full strength falls with this many players online: 40% alone, 70% at 10 players,
/// all of it from 100 players.
fn rain_amount(players: usize) -> f64 {
    (0.4 + 0.3 * (players.max(1) as f64).log10()).min(1.0)
}

/// How strongly springs flow with this many players online: 20% alone (about one cell every 1.5 s, slower
/// than one player scoops), 60% at 10 players, full strength from 100 players.
fn spring_amount(players: usize) -> f64 {
    (0.2 + 0.4 * (players.max(1) as f64).log10()).min(1.0)
}

/// How fast player actions are with this many players online (cooldowns are divided by it).
fn action_speed(players: usize) -> f64 {
    (2.0 - 0.5 * (players.max(1) as f64).log10()).clamp(SPEED_MIN, SPEED_MAX)
}

pub enum Cmd {
    Connect { id: u64, token: String, dev: bool, outbox: Sender<Bytes> },
    Disconnect { id: u64 },
    Client { id: u64, msg: ClientMsg },
    Console(String),
    Shutdown,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Player {
    rock: u32,
    water: u32,
    sponge: u32,
    quartz: u32,
    pickaxe: u8,
    walls_unlocked: bool,
    cells_mined: u32,
    #[serde(skip)]
    ready_at: Option<Instant>,
    #[serde(skip)]
    cooldown: Duration,
}

struct Client {
    token: String,
    outbox: Sender<Bytes>,
    chunks: HashSet<u32>,
    /// Connected with the dev key: no cooldowns or costs, and actions cover brush x brush cells.
    dev: bool,
    brush: u8,
}

/// Largest dev brush.
const BRUSH_MAX: u8 = 41;

pub struct Sim {
    world: World,
    players: HashMap<String, Player>,
    clients: HashMap<u64, Client>,
    /// For each chunk, the clients subscribed to it.
    subscribers: Vec<Vec<u64>>,
    /// Clients whose outbox was full; they are disconnected after the current step.
    dropped: Vec<u64>,
    data_dir: PathBuf,
    ticks: u64,
    rain_left: u32,
    /// 1 light, 2 normal, 3 heavy; 0 while it is dry.
    rain_strength: u8,
    /// Fraction of a drop left over from the last tick.
    rain_carry: f64,
    next_rain: u64,
    record_depth: u32,
    /// Distinct players connected (several tabs of one guest count once).
    players_online: usize,
    rng: Rng,
    /// Load figures since the last report.
    tick_time: Duration,
    tick_time_max: Duration,
    bytes_sent: usize,
}

impl Sim {
    pub fn new(world: World, data_dir: PathBuf) -> Sim {
        let players = std::fs::read(data_dir.join("players.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let chunk_count = world.chunks_wide() * world.chunks_high();
        let mut rng = Rng::new(world.seed ^ 0x7A1);
        let next_rain = RAIN_GAP_MIN_TICKS + rng.below(RAIN_GAP_RANDOM_TICKS);
        Sim {
            record_depth: world.deepest_dig(),
            world,
            players,
            clients: HashMap::new(),
            subscribers: vec![Vec::new(); chunk_count],
            dropped: Vec::new(),
            data_dir,
            ticks: 0,
            rain_left: 0,
            rain_strength: 0,
            rain_carry: 0.0,
            next_rain,
            players_online: 0,
            rng,
            tick_time: Duration::ZERO,
            tick_time_max: Duration::ZERO,
            bytes_sent: 0,
        }
    }

    pub fn run(mut self, rx: Receiver<Cmd>) {
        let mut next_tick = Instant::now() + TICK;
        loop {
            let now = Instant::now();
            if now >= next_tick {
                let started = Instant::now();
                self.tick();
                let took = started.elapsed();
                self.tick_time += took;
                self.tick_time_max = self.tick_time_max.max(took);
                if self.ticks % REPORT_EVERY_TICKS == 0 {
                    self.report();
                }
                next_tick += TICK;
                if next_tick < now {
                    next_tick = now + TICK;
                }
                continue;
            }
            match rx.recv_timeout(next_tick - now) {
                Ok(Cmd::Shutdown) => {
                    self.save(false);
                    println!("saved, bye");
                    std::process::exit(0);
                }
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.drop_slow_clients();
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Connect { id, token, dev, outbox } => {
                if dev {
                    println!("dev client connected");
                }
                self.players.entry(token.clone()).or_default();
                self.clients.insert(id, Client { token, outbox, chunks: HashSet::new(), dev, brush: 1 });
                self.count_players();
                let welcome = self.welcome();
                self.send(id, welcome);
                self.send_player(id, ActResult::None);
                let stats = self.stats();
                self.send(id, stats);
            }
            Cmd::Disconnect { id } => self.remove_client(id),
            Cmd::Client { id, msg: ClientMsg::Subscribe(chunks) } => self.subscribe(id, chunks),
            Cmd::Client { id, msg: ClientMsg::Action { kind, x, y, arg } } => {
                let result = self.act(id, kind, x, y, arg);
                self.send_player(id, result);
            }
            Cmd::Client { id, msg: ClientMsg::Brush(size) } => {
                if let Some(c) = self.clients.get_mut(&id).filter(|c| c.dev) {
                    c.brush = size.clamp(1, BRUSH_MAX);
                }
            }
            Cmd::Client { id, msg: ClientMsg::Buy(upgrade) } => {
                let result = self.buy(id, upgrade);
                self.send_player(id, result);
            }
            Cmd::Console(line) => self.console(line.trim()),
            Cmd::Shutdown => unreachable!(),
        }
    }

    fn console(&mut self, line: &str) {
        let mut words = line.split_whitespace();
        match (words.next().unwrap_or(""), words.next()) {
            ("rain", strength) => {
                let pick = strength.and_then(|n| RAIN_NAMES.iter().position(|&r| r == n));
                if strength.is_some() && pick.is_none() {
                    println!("rain strengths: light, normal, heavy");
                } else {
                    self.start_rain(pick);
                }
            }
            ("save", _) => self.save(false),
            ("stats", _) => println!(
                "{} clients, {} players known, record depth {}",
                self.players_online,
                self.players.len(),
                self.record_depth
            ),
            ("", _) => {}
            _ => println!("commands: rain [light|normal|heavy], save, stats, quit"),
        }
    }

    fn tick(&mut self) {
        self.ticks += 1;
        self.world.tend_ocean();
        self.world.step_water();
        self.world.step_springs(world::SPRING_RATE * spring_amount(self.players_online));
        self.world.step_caveins();
        // Puddles on the land dry up between showers.
        if self.rain_left == 0 {
            self.world.evaporate();
        }
        if self.ticks % ABSORB_EVERY_TICKS == 0 {
            self.world.absorb();
        }
        self.rain();
        self.world.update_sight();
        self.send_diffs();
        if self.ticks % STATS_EVERY_TICKS == 0 {
            let stats = self.stats();
            let ids: Vec<u64> = self.clients.keys().copied().collect();
            for id in ids {
                self.send(id, stats.clone());
            }
        }
        if self.ticks % SAVE_EVERY_TICKS == 0 {
            self.save(true);
        }
        self.drop_slow_clients();
    }

    fn report(&mut self) {
        if !self.clients.is_empty() {
            let secs = (TICK * REPORT_EVERY_TICKS as u32).as_secs_f64();
            println!(
                "{} clients | tick avg {:.2} ms, max {:.2} ms | sent {:.0} KB/s ({:.1} KB/s per client)",
                self.clients.len(),
                self.tick_time.as_secs_f64() * 1000.0 / REPORT_EVERY_TICKS as f64,
                self.tick_time_max.as_secs_f64() * 1000.0,
                self.bytes_sent as f64 / 1024.0 / secs,
                self.bytes_sent as f64 / 1024.0 / secs / self.clients.len() as f64,
            );
        }
        self.tick_time = Duration::ZERO;
        self.tick_time_max = Duration::ZERO;
        self.bytes_sent = 0;
    }

    /// Start a shower of the given strength (index into RAIN_NAMES), or a random one of the strengths
    /// allowed with the players online.
    fn start_rain(&mut self, strength: Option<usize>) {
        let players = self.players_online;
        let strength = strength.unwrap_or_else(|| {
            let allowed = |k: usize| players >= RAIN_MIN_PLAYERS[k];
            let total: u64 = (0..3).filter(|&k| allowed(k)).map(|k| RAIN_CHANCE[k]).sum();
            let mut roll = self.rng.below(total);
            (0..3).filter(|&k| allowed(k)).find(|&k| roll < RAIN_CHANCE[k] || { roll -= RAIN_CHANCE[k]; false }).unwrap_or(0)
        });
        self.rain_strength = strength as u8 + 1;
        self.rain_left = RAIN_TICKS;
        println!("{} rain started", RAIN_NAMES[strength]);
    }

    fn rain(&mut self) {
        if self.rain_left == 0 && self.ticks >= self.next_rain {
            self.start_rain(None);
        }
        if self.rain_left == 0 {
            return;
        }
        self.rain_left -= 1;
        if self.rain_left == 0 {
            self.rain_strength = 0;
            self.next_rain = self.ticks + RAIN_GAP_MIN_TICKS + self.rng.below(RAIN_GAP_RANDOM_TICKS);
            return;
        }
        let per_128 = RAIN_DROPS[self.rain_strength as usize - 1] * rain_amount(self.players_online);
        self.rain_carry += (self.world.w as f64 / 128.0 + 1.0) * per_128;
        while self.rain_carry >= 1.0 {
            self.rain_carry -= 1.0;
            let x = self.rng.below(self.world.w as u64) as usize;
            self.world.add_water(x, 0);
        }
    }

    fn count_players(&mut self) {
        self.players_online = self.clients.values().map(|c| &c.token).collect::<HashSet<_>>().len();
    }

    fn speed(&self) -> f64 {
        action_speed(self.players_online)
    }

    fn act(&mut self, id: u64, kind: u8, x: u32, y: u32, arg: u8) -> ActResult {
        let speed = self.speed();
        let Some(client) = self.clients.get(&id) else { return ActResult::None };
        if client.dev {
            let brush = client.brush;
            return self.dev_act(kind, x as usize, y as usize, arg, brush);
        }
        let player = self.players.get_mut(&client.token).expect("player exists for every client");
        let now = Instant::now();
        if player.ready_at.is_some_and(|t| t > now) {
            return ActResult::Cooldown;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.world.w || y >= self.world.h {
            return ActResult::WrongTarget;
        }
        let i = self.world.idx(x, y);
        if !self.world.is_revealed(i) {
            return ActResult::NotVisible;
        }
        if !self.world.reachable(i) {
            return ActResult::OutOfReach;
        }
        let t = self.world.ty(i);

        let cooldown = match kind {
            protocol::ACT_DIG => match t {
                world::WATER => {
                    player.water += 1;
                    self.world.dig(i);
                    COOLDOWN_COLLECT
                }
                world::ROCK => {
                    player.rock += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                // Walls take three hits: intact, cracked, crumbling, gone (1 rock back).
                world::WALL => {
                    self.world.set_type(i, world::WALL_CRACKED);
                    COOLDOWN_DIG
                }
                world::WALL_CRACKED => {
                    self.world.set_type(i, world::WALL_CRUMBLING);
                    COOLDOWN_DIG
                }
                world::WALL_CRUMBLING => {
                    player.rock += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                world::RUBBLE | world::RUBBLE_FALLING => {
                    player.rock += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                world::ROCK_CRACKED => {
                    player.rock += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                // Damp rock gives rock, and opens the spring behind it.
                world::SPRING_ROCK => {
                    player.rock += 1;
                    player.cells_mined += 1;
                    self.world.set_type(i, world::SPRING);
                    COOLDOWN_DIG
                }
                world::QUARTZ_ORE => {
                    player.quartz += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                world::SPONGE_ORE | world::SPONGE => {
                    player.sponge += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                _ => return ActResult::WrongTarget,
            },
            protocol::ACT_WALL if !player.walls_unlocked => return ActResult::Locked,
            protocol::ACT_WALL if t == world::WALL_CRACKED || t == world::WALL_CRUMBLING => {
                if player.rock < REPAIR_COST {
                    return ActResult::NoResources;
                }
                player.rock -= REPAIR_COST;
                self.world.set_type(i, world::WALL);
                COOLDOWN_DIG
            }
            // Rubble can be dropped anywhere open, in the sky too: it falls until it lands.
            protocol::ACT_PLACE if arg == protocol::ITEM_RUBBLE => {
                if t != world::AIR && t != world::WATER {
                    return ActResult::WrongTarget;
                }
                if player.rock < RUBBLE_COST {
                    return ActResult::NoResources;
                }
                player.rock -= RUBBLE_COST;
                self.world.set_type(i, world::RUBBLE_FALLING);
                COOLDOWN_DIG
            }
            protocol::ACT_PLACE if arg == protocol::ITEM_WATER => {
                if t != world::AIR {
                    return ActResult::WrongTarget;
                }
                if player.water == 0 {
                    return ActResult::NoResources;
                }
                player.water -= 1;
                self.world.set_type(i, world::WATER);
                COOLDOWN_COLLECT
            }
            protocol::ACT_WALL | protocol::ACT_PLACE => {
                if t != world::AIR && t != world::WATER {
                    return ActResult::WrongTarget;
                }
                if !self.world.supported(i) {
                    return ActResult::Unsupported;
                }
                if kind == protocol::ACT_WALL {
                    if player.rock < WALL_COST {
                        return ActResult::NoResources;
                    }
                    player.rock -= WALL_COST;
                    self.world.set_type(i, world::WALL);
                    COOLDOWN_WALL
                } else if arg == protocol::ITEM_ROCK {
                    if player.rock == 0 {
                        return ActResult::NoResources;
                    }
                    player.rock -= 1;
                    self.world.set_type(i, world::ROCK);
                    COOLDOWN_DIG
                } else {
                    if arg != protocol::ITEM_SPONGE {
                        return ActResult::WrongTarget;
                    }
                    if player.sponge == 0 {
                        return ActResult::NoResources;
                    }
                    player.sponge -= 1;
                    self.world.set_type(i, world::SPONGE);
                    COOLDOWN_PLACE
                }
            }
            _ => return ActResult::WrongTarget,
        };
        // A better pickaxe digs faster; it does not help with scooping water or building.
        let pickaxe = if kind == protocol::ACT_DIG && t != world::WATER {
            1.0 + player.pickaxe as f64 * PICKAXE_STEP
        } else {
            1.0
        };
        let cooldown = cooldown.div_f64(speed * pickaxe);
        player.ready_at = Some(now + cooldown);
        player.cooldown = cooldown;

        if self.world.ty(i) == world::AIR && y > self.world.surface[x] as usize {
            self.record_depth = self.record_depth.max((y - self.world.surface[x] as usize) as u32);
        }
        ActResult::Ok
    }

    /// A dev client's action: no cooldown, no cost, no reach or sight rules, applied to every cell of a
    /// brush x brush square around (x, y). Digging takes everything but bedrock and springs (damp rock
    /// opens its spring); walls, rubble and spongestone fill the open cells.
    fn dev_act(&mut self, kind: u8, x: usize, y: usize, arg: u8, brush: u8) -> ActResult {
        let r = (brush / 2) as i64;
        for dy in -r..=r {
            for dx in -r..=r {
                let (cx, cy) = (x as i64 + dx, y as i64 + dy);
                if cx < 0 || cy < 0 || cx >= self.world.w as i64 || cy >= self.world.h as i64 {
                    continue;
                }
                let i = self.world.idx(cx as usize, cy as usize);
                let t = self.world.ty(i);
                let open = t == world::AIR || t == world::WATER;
                let to = match kind {
                    protocol::ACT_DIG => match t {
                        world::AIR | world::BEDROCK | world::SPRING => continue,
                        world::SPRING_ROCK => world::SPRING,
                        _ => world::AIR,
                    },
                    protocol::ACT_WALL if open => world::WALL,
                    protocol::ACT_PLACE if open && arg == protocol::ITEM_RUBBLE => world::RUBBLE_FALLING,
                    protocol::ACT_PLACE if open && arg == protocol::ITEM_SPONGE => world::SPONGE,
                    protocol::ACT_PLACE if open && arg == protocol::ITEM_ROCK => world::ROCK,
                    protocol::ACT_PLACE if t == world::AIR && arg == protocol::ITEM_WATER => world::WATER,
                    _ => continue,
                };
                self.world.set_type(i, to);
            }
        }
        ActResult::Ok
    }

    fn buy(&mut self, id: u64, upgrade: u8) -> ActResult {
        let Some(client) = self.clients.get(&id) else { return ActResult::None };
        let player = self.players.get_mut(&client.token).expect("player exists for every client");
        match upgrade {
            protocol::UPGRADE_WALLS if !player.walls_unlocked => {
                if player.rock < WALLS_UNLOCK_COST {
                    return ActResult::NoResources;
                }
                player.rock -= WALLS_UNLOCK_COST;
                player.walls_unlocked = true;
            }
            protocol::UPGRADE_PICKAXE if player.pickaxe < PICKAXE_MAX => {
                let cost = player.pickaxe as u32 + 1;
                if player.quartz < cost {
                    return ActResult::NoResources;
                }
                player.quartz -= cost;
                player.pickaxe += 1;
            }
            _ => return ActResult::WrongTarget,
        }
        ActResult::Ok
    }

    fn subscribe(&mut self, id: u64, wanted: Vec<(u16, u16)>) {
        let (cw, ch) = (self.world.chunks_wide(), self.world.chunks_high());
        let wanted: HashSet<u32> = wanted
            .into_iter()
            .filter(|&(cx, cy)| (cx as usize) < cw && (cy as usize) < ch)
            .map(|(cx, cy)| cy as u32 * cw as u32 + cx as u32)
            .collect();
        let Some(client) = self.clients.get_mut(&id) else { return };
        let old = std::mem::replace(&mut client.chunks, wanted.clone());

        for &c in old.difference(&wanted) {
            self.subscribers[c as usize].retain(|&s| s != id);
        }
        let mut msg = vec![protocol::S_CHUNKS, 0, 0];
        let mut n: u16 = 0;
        for &c in wanted.difference(&old) {
            self.subscribers[c as usize].push(id);
            let (cx, cy) = (c as usize % cw, c as usize / cw);
            put_u16(&mut msg, cx as u16);
            put_u16(&mut msg, cy as u16);
            let len_at = msg.len();
            put_u16(&mut msg, 0);
            self.world.encode_chunk(cx, cy, &mut msg);
            let len = (msg.len() - len_at - 2) as u16;
            msg[len_at..len_at + 2].copy_from_slice(&len.to_le_bytes());
            n += 1;
        }
        if n > 0 {
            msg[1..3].copy_from_slice(&n.to_le_bytes());
            self.send(id, msg);
        }
    }

    /// Send every client the changed cells in the chunks it is subscribed to.
    fn send_diffs(&mut self) {
        let dirty = self.world.take_dirty();
        if dirty.is_empty() {
            return;
        }
        let (w, cw) = (self.world.w, self.world.chunks_wide());
        let mut by_chunk: HashMap<u32, Vec<u8>> = HashMap::new();
        for i in dirty {
            let i = i as usize;
            let (x, y) = (i % w, i / w);
            let chunk = ((y / CHUNK) * cw + x / CHUNK) as u32;
            let entries = by_chunk.entry(chunk).or_default();
            put_u16(entries, ((y % CHUNK) * CHUNK + x % CHUNK) as u16);
            entries.push(self.world.wire(i));
        }

        let mut out: HashMap<u64, (u16, Vec<u8>)> = HashMap::new();
        for (chunk, entries) in &by_chunk {
            for &id in &self.subscribers[*chunk as usize] {
                let (n, msg) = out.entry(id).or_insert_with(|| (0, vec![protocol::S_DIFF, 0, 0]));
                *n += 1;
                put_u16(msg, (*chunk as usize % cw) as u16);
                put_u16(msg, (*chunk as usize / cw) as u16);
                put_u16(msg, (entries.len() / 3) as u16);
                msg.extend_from_slice(entries);
            }
        }
        for (id, (n, mut msg)) in out {
            msg[1..3].copy_from_slice(&n.to_le_bytes());
            self.send(id, msg);
        }
    }

    fn welcome(&self) -> Vec<u8> {
        let w = &self.world;
        let mut m = vec![protocol::S_WELCOME];
        put_u32(&mut m, w.w as u32);
        put_u32(&mut m, w.h as u32);
        put_u16(&mut m, CHUNK as u16);
        let spawn_x = w.w / 2;
        put_u32(&mut m, spawn_x as u32);
        put_u32(&mut m, w.surface[spawn_x] as u32);
        put_u16(&mut m, TICK.as_millis() as u16);
        for c in [COOLDOWN_DIG, COOLDOWN_COLLECT, COOLDOWN_WALL, COOLDOWN_PLACE] {
            put_u16(&mut m, c.as_millis() as u16);
        }
        put_u16(&mut m, WALL_COST as u16);
        put_u16(&mut m, REPAIR_COST as u16);
        put_u16(&mut m, w.sea_level as u16);
        for &s in &w.surface {
            put_u16(&mut m, s);
        }
        m
    }

    fn send_player(&mut self, id: u64, result: ActResult) {
        let Some(client) = self.clients.get(&id) else { return };
        let p = &self.players[&client.token];
        let now = Instant::now();
        let left = p.ready_at.map_or(Duration::ZERO, |t| t.saturating_duration_since(now));
        let mut m = vec![protocol::S_PLAYER, result as u8];
        put_u32(&mut m, left.as_millis() as u32);
        put_u32(&mut m, p.cooldown.as_millis() as u32);
        put_u32(&mut m, p.rock);
        put_u32(&mut m, p.water);
        put_u32(&mut m, p.sponge);
        put_u32(&mut m, p.quartz);
        m.push(p.pickaxe);
        m.push(p.walls_unlocked as u8);
        // Several tabs may share one player: keep them all in sync.
        let token = client.token.clone();
        let ids: Vec<u64> =
            self.clients.iter().filter(|(_, c)| c.token == token).map(|(&cid, _)| cid).collect();
        for cid in ids {
            if cid != id {
                m[1] = ActResult::None as u8;
            }
            self.send(cid, m.clone());
        }
    }

    fn stats(&self) -> Vec<u8> {
        let mut m = vec![protocol::S_STATS];
        put_u16(&mut m, self.players_online.min(u16::MAX as usize) as u16);
        put_u32(&mut m, self.record_depth);
        m.push(self.rain_strength);
        put_u16(&mut m, (self.speed() * 100.0).round() as u16);
        m
    }

    fn send(&mut self, id: u64, msg: Vec<u8>) {
        if let Some(c) = self.clients.get(&id) {
            self.bytes_sent += msg.len();
            if c.outbox.try_send(Bytes::from(msg)).is_err() {
                self.dropped.push(id);
            }
        }
    }

    /// A client that cannot keep up with its messages is disconnected rather than buffered forever.
    fn drop_slow_clients(&mut self) {
        while let Some(id) = self.dropped.pop() {
            if self.clients.contains_key(&id) {
                println!("client {id} too slow, disconnecting");
                self.remove_client(id);
            }
        }
    }

    fn remove_client(&mut self, id: u64) {
        if let Some(c) = self.clients.remove(&id) {
            self.count_players();
            for chunk in c.chunks {
                self.subscribers[chunk as usize].retain(|&s| s != id);
            }
        }
    }

    /// Save the world and players. In the background, the file writing runs on its own thread.
    fn save(&self, background: bool) {
        let world = self.world.to_bytes();
        let players = serde_json::to_vec(&self.players).expect("players serialise");
        let dir = self.data_dir.clone();
        let write = move || {
            let result = std::fs::create_dir_all(&dir)
                .and_then(|_| world::write_atomic(&dir.join("world.bin"), &world))
                .and_then(|_| world::write_atomic(&dir.join("players.json"), &players));
            if let Err(e) = result {
                eprintln!("saving failed: {e}");
            }
        };
        if background {
            std::thread::spawn(write);
        } else {
            write();
            println!("world saved");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_scales_with_players() {
        assert!((action_speed(1) - 2.0).abs() < 0.01);
        assert!((action_speed(10) - 1.5).abs() < 0.01);
        assert!((action_speed(100) - 1.0).abs() < 0.01);
        assert!((action_speed(300) - 0.76).abs() < 0.01);
        assert_eq!(action_speed(5000), SPEED_MIN);
    }

    #[test]
    fn springs_are_gentler_for_small_groups() {
        assert!((spring_amount(1) - 0.2).abs() < 0.01);
        assert!((spring_amount(10) - 0.6).abs() < 0.01);
        assert_eq!(spring_amount(100), 1.0);
    }

    #[test]
    fn rain_is_gentler_for_small_groups() {
        assert!((rain_amount(1) - 0.4).abs() < 0.01);
        assert!((rain_amount(10) - 0.7).abs() < 0.01);
        assert_eq!(rain_amount(100), 1.0);
        assert_eq!(rain_amount(1000), 1.0);
        let mut sim = Sim::new(World::generate(1024, 512, 3), std::env::temp_dir().join("mine-together-test-none"));
        for (players, strongest) in [(1, 1), (10, 2), (50, 3)] {
            sim.players_online = players;
            let worst = (0..200).map(|_| (sim.start_rain(None), sim.rain_strength).1).max().unwrap();
            assert_eq!(worst, strongest, "with {players} players");
        }
    }

    #[test]
    fn walls_must_be_unlocked_and_pickaxe_upgrades_cost_quartz() {
        let world = World::generate(1024, 512, 3);
        let mut sim = Sim::new(world, std::env::temp_dir().join("mine-together-test-none"));
        let (outbox, _rx) = tokio::sync::mpsc::channel(1024);
        sim.handle(Cmd::Connect { id: 1, token: "tester".into(), dev: false, outbox });
        let (x, y) = (sim.world.w / 2, sim.world.surface[sim.world.w / 2] as usize - 1);
        sim.players.get_mut("tester").unwrap().rock = 12;
        assert_eq!(sim.act(1, protocol::ACT_WALL, x as u32, y as u32, 0), ActResult::Locked);
        assert_eq!(sim.buy(1, protocol::UPGRADE_WALLS), ActResult::Ok);
        assert_eq!(sim.players["tester"].rock, 2);
        assert_eq!(sim.buy(1, protocol::UPGRADE_WALLS), ActResult::WrongTarget, "already unlocked");

        let p = sim.players.get_mut("tester").unwrap();
        p.quartz = 3;
        assert_eq!(sim.buy(1, protocol::UPGRADE_PICKAXE), ActResult::Ok); // level 1 costs 1
        assert_eq!(sim.buy(1, protocol::UPGRADE_PICKAXE), ActResult::Ok); // level 2 costs 2
        assert_eq!(sim.buy(1, protocol::UPGRADE_PICKAXE), ActResult::NoResources);
        assert_eq!(sim.players["tester"].pickaxe, 2);
        assert_eq!(sim.players["tester"].quartz, 0);
    }
}
