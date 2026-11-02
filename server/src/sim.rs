//! The simulation thread. It alone owns the world and the players; network tasks talk to it through
//! a command queue, and it sends each client its messages through that client's outbox.

use crate::noise::Rng;
use crate::protocol::{self, put_u16, put_u32, ActResult, ClientMsg};
use crate::world::{self, World, CHUNK};
use axum::body::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;

const TICK: Duration = Duration::from_millis(100);
const COOLDOWN_DIG: Duration = Duration::from_millis(1500);
const COOLDOWN_COLLECT: Duration = Duration::from_millis(2500);
const COOLDOWN_WALL: Duration = Duration::from_millis(4000);
const COOLDOWN_PLACE: Duration = Duration::from_millis(3000);
const WALL_COST: u32 = 2;
/// Repairing a cracked wall costs this much rock (nothing, so defending never runs dry) and takes the dig
/// cooldown, so one defender can hold a wall against one attacker, but not against a group.
const REPAIR_COST: u32 = 0;
/// Rubble, dropped to fill flooded shafts, is cheap so a crowd can plug a leak.
const RUBBLE_COST: u32 = 1;
/// New players cannot build walls; unlocking them costs this much rock.
const WALLS_UNLOCK_COST: u32 = 10;
/// Pickaxe upgrades: level n digs (1 + n * PICKAXE_STEP) times as fast. Going from level n to n + 1
/// costs 2^(n + 1) - 1 quartz: 1, 3, 7, 15, 31.
const PICKAXE_MAX: u8 = 5;
const PICKAXE_STEP: f64 = 0.1;
/// Heavier strikes: at level n every hit on rock does n + 1 damage (hard rock needs fewer hits). Reaching
/// level n costs STRIKE_COSTS[n - 1] amethyst.
const STRIKE_COSTS: [u32; 2] = [3, 10];
/// Player actions get faster with few players and slower with many: speed = 2 - 0.5 * log10(players),
/// so 1 player x2, 10 players x1.5, 100 players x1 (the cooldowns above), 300 players x0.76, and
/// never outside these bounds.
const SPEED_MIN: f64 = 0.5;
const SPEED_MAX: f64 = 2.0;
/// Placed spongerock soaks up a neighbouring water cell this often.
const ABSORB_EVERY_TICKS: u64 = 20;
/// Living coral gets a turn to grow, and emberstone boils water, this often.
const CORAL_EVERY_TICKS: u64 = 10;
const BOIL_EVERY_TICKS: u64 = 10;

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
/// An Upwelling comes every 30 to 60 minutes, after a warning of STIR_TICKS (the deep is stirring). Its
/// strength scales with the players online like rain (see rain_amount).
const UPWELLING_GAP_MIN_TICKS: u64 = 18_000;
const UPWELLING_GAP_RANDOM_TICKS: u64 = 18_000;
const STIR_TICKS: u64 = 300;

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
    /// `limited`: the per-address limits apply (not to the dev client, bots with the key, or this machine).
    Connect { id: u64, token: String, dev: bool, ip: IpAddr, limited: bool, outbox: Sender<Bytes> },
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
    amethyst: u32,
    strike: u8,
    /// The other kinds of rock (`rock` is stone, which pays for walls and rubble).
    dirt: u32,
    /// Granite, slate, basalt, darkstone.
    deep: [u32; 4],
    coral: u32,
    ember: u32,
    lava: u32,
    opal: u32,
    walls_unlocked: bool,
    cells_mined: u32,
    #[serde(skip)]
    ready_at: Option<Instant>,
    #[serde(skip)]
    cooldown: Duration,
}

/// Players on the leaderboard.
const LEADERS_SHOWN: usize = 10;

/// A player's display name, made from their token, e.g. "Dusty Mole 42": players have no accounts yet,
/// and the same browser always gets the same name.
fn display_name(token: &str) -> String {
    const ADJECTIVES: [&str; 32] = [
        "Dusty", "Brave", "Sleepy", "Mighty", "Muddy", "Swift", "Lucky", "Gritty", "Rusty", "Clever", "Shiny",
        "Grumpy", "Jolly", "Sneaky", "Bold", "Quiet", "Wild", "Tiny", "Giant", "Soggy", "Rocky", "Hasty",
        "Cosy", "Daring", "Fuzzy", "Nimble", "Stubborn", "Cheery", "Deep", "Gentle", "Restless", "Humble",
    ];
    const ANIMALS: [&str; 32] = [
        "Mole", "Badger", "Wombat", "Rabbit", "Ferret", "Beetle", "Gopher", "Otter", "Armadillo", "Hedgehog",
        "Marmot", "Weasel", "Shrew", "Vole", "Worm", "Ant", "Fox", "Bear", "Dwarf", "Gnome", "Penguin", "Crab",
        "Lizard", "Newt", "Toad", "Raccoon", "Hamster", "Chipmunk", "Meerkat", "Aardvark", "Pangolin", "Squirrel",
    ];
    // FNV-1a: a stable hash, the same on every run.
    let h = token.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    let h = crate::noise::hash(h, 0, 0);
    format!("{} {} {}", ADJECTIVES[(h % 32) as usize], ANIMALS[(h / 32 % 32) as usize], h / 1024 % 100)
}

/// Milestones already announced in this world, saved in milestones.json.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Milestones {
    /// The world they belong to: a new world starts over.
    seed: u64,
    /// The last record depth announced.
    depth: u32,
    /// First finds already announced ("spongerock", "quartz", "spring").
    firsts: std::collections::BTreeSet<String>,
}

/// Record depths worth announcing: every 25 m up to 100 m, then every 50 m.
fn next_depth_milestone(after: u32) -> u32 {
    if after < 100 {
        (after / 25 + 1) * 25
    } else {
        (after / 50 + 1) * 50
    }
}

/// The cell a found block makes when placed: crystals go back as they were found, coral is planted alive.
fn block_cell(item: u8) -> Option<u8> {
    match item {
        protocol::ITEM_SPONGE => Some(world::SPONGE),
        protocol::ITEM_QUARTZ => Some(world::QUARTZ_ORE),
        protocol::ITEM_AMETHYST => Some(world::AMETHYST_ORE),
        protocol::ITEM_CORAL => Some(world::CORAL),
        protocol::ITEM_EMBER => Some(world::EMBER),
        protocol::ITEM_LAVA => Some(world::LAVA_ROCK),
        protocol::ITEM_OPAL => Some(world::FIRE_OPAL),
        _ => None,
    }
}

impl Player {
    /// How many found blocks of this item (one block_cell knows) the player holds.
    fn found_block(&mut self, item: u8) -> &mut u32 {
        match item {
            protocol::ITEM_SPONGE => &mut self.sponge,
            protocol::ITEM_QUARTZ => &mut self.quartz,
            protocol::ITEM_AMETHYST => &mut self.amethyst,
            protocol::ITEM_CORAL => &mut self.coral,
            protocol::ITEM_LAVA => &mut self.lava,
            protocol::ITEM_OPAL => &mut self.opal,
            _ => &mut self.ember,
        }
    }

    /// How much this player holds of a kind of rock (index into world::ROCK_KINDS).
    fn rock_kind(&mut self, kind: usize) -> &mut u32 {
        match kind {
            0 => &mut self.dirt,
            1 => &mut self.rock,
            k => &mut self.deep[k - 2],
        }
    }
}

struct Client {
    token: String,
    outbox: Sender<Bytes>,
    chunks: HashSet<u32>,
    /// Connected with the dev key: no cooldowns or costs, and actions cover brush x brush cells.
    dev: bool,
    brush: u8,
    ip: IpAddr,
    limited: bool,
}

/// Largest dev brush.
const BRUSH_MAX: u8 = 41;

/// One internet address (a home, a school, or one person with a script opening many connections) acts like
/// at most this many players: its actions share this many cooldowns, and it counts at most this many times
/// towards the players online (which sets the action speed and the rain).
const IP_SLOTS: usize = 3;
/// Connections allowed per internet address (several tabs of one player count too); more are refused.
const MAX_CONNECTIONS_PER_IP: usize = 10;

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
    /// When the next Upwelling starts stirring, and, while it stirs, the tick it breaks loose.
    next_upwelling: u64,
    stirring_until: Option<u64>,
    /// The stirring leads to a cataclysm, not an Upwelling.
    cataclysm: bool,
    /// The Upwelling phase last logged (see upwelling_phase).
    logged_phase: u8,
    record_depth: u32,
    /// Distinct players connected (several tabs of one guest count once).
    players_online: usize,
    milestones: Milestones,
    /// The leaderboard (blocks mined, token), best first, and when it was last worked out.
    leaders: Vec<(u32, String)>,
    leaders_at: Option<Instant>,
    /// Announcements from this step, sent to everyone after it.
    announcements: Vec<String>,
    /// For each limited internet address, when each of its IP_SLOTS shared cooldowns is over.
    ip_slots: HashMap<IpAddr, [Instant; IP_SLOTS]>,
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
        let milestones = std::fs::read(data_dir.join("milestones.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Milestones>(&b).ok())
            .filter(|m| m.seed == world.seed)
            .unwrap_or_else(|| Milestones { seed: world.seed, depth: world.deepest_dig(), ..Default::default() });
        let mut rng = Rng::new(world.seed ^ 0x7A1);
        let next_rain = RAIN_GAP_MIN_TICKS + rng.below(RAIN_GAP_RANDOM_TICKS);
        let next_upwelling = UPWELLING_GAP_MIN_TICKS + rng.below(UPWELLING_GAP_RANDOM_TICKS);
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
            next_upwelling,
            stirring_until: None,
            cataclysm: false,
            logged_phase: 0,
            players_online: 0,
            ip_slots: HashMap::new(),
            milestones,
            leaders: Vec::new(),
            leaders_at: None,
            announcements: Vec::new(),
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
            Cmd::Connect { id, token, dev, ip, limited, outbox } => {
                if dev {
                    println!("dev client connected");
                }
                // Refused: dropping the outbox closes the connection.
                if limited && self.clients.values().filter(|c| c.ip == ip).count() >= MAX_CONNECTIONS_PER_IP {
                    return;
                }
                self.players.entry(token.clone()).or_default();
                let client = Client { token, outbox, chunks: HashSet::new(), dev, brush: 1, ip, limited };
                self.clients.insert(id, client);
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
                self.send_announcements();
            }
            Cmd::Client { id, msg: ClientMsg::Leaders } => self.send_leaders(id),
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
            ("upwell", _) => self.stir(false),
            ("cataclysm", _) => self.stir(true),
            ("save", _) => self.save(false),
            ("stats", _) => println!(
                "{} clients, {} players known, record depth {}",
                self.players_online,
                self.players.len(),
                self.record_depth
            ),
            ("", _) => {}
            _ => println!("commands: rain [light|normal|heavy], upwell, cataclysm, save, stats, quit"),
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
        if self.ticks % CORAL_EVERY_TICKS == 0 {
            self.world.grow_coral();
        }
        if self.ticks % BOIL_EVERY_TICKS == 5 {
            self.world.boil();
        }
        self.rain();
        self.upwelling();
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

    /// The deep starts stirring: an Upwelling (or with `cataclysm`, a cataclysm) breaks loose after STIR_TICKS.
    fn stir(&mut self, cataclysm: bool) {
        if self.stirring_until.is_some() || self.world.upwelling_phase() != 0 {
            println!("an Upwelling is already going on");
            return;
        }
        self.stirring_until = Some(self.ticks + STIR_TICKS);
        self.cataclysm = cataclysm;
        self.announcements.push(if cataclysm {
            "The whole island is shaking… a cataclysm is coming.".to_string()
        } else {
            "The deep is stirring… an Upwelling is coming.".to_string()
        });
    }

    fn upwelling(&mut self) {
        if self.ticks >= self.next_upwelling {
            self.next_upwelling = self.ticks + UPWELLING_GAP_MIN_TICKS + self.rng.below(UPWELLING_GAP_RANDOM_TICKS);
            self.stir(false);
        }
        if self.stirring_until.is_some_and(|t| self.ticks >= t) {
            self.stirring_until = None;
            if self.cataclysm {
                self.world.start_cataclysm();
            } else {
                self.world.start_upwelling(rain_amount(self.players_online));
            }
        }
        match self.world.step_upwelling() {
            Some(world::UpwellingNews::Erupted) => self
                .announcements
                .push("The Upwelling broke through: a volcano is erupting!".to_string()),
            Some(world::UpwellingNews::Over) => {
                self.announcements.push("The Upwelling is over.".to_string());
            }
            None => {}
        }
        let phase = self.upwelling_phase();
        if phase != self.logged_phase {
            self.logged_phase = phase;
            let (tips, vents, molten) = self.world.upwelling_counts();
            let name = ["over", "stirring", "veins rising", "erupting", "veins cooling"][phase as usize];
            println!("upwelling: {name} ({tips} vein tips, {vents} vents, {molten} molten cells)");
        }
    }

    /// What clients see of the Upwelling: 0 none, 1 stirring, 2 veins rising, 3 erupting, 4 veins cooling.
    fn upwelling_phase(&self) -> u8 {
        if self.stirring_until.is_some() { 1 } else { self.world.upwelling_phase() }
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

    /// Distinct players online, each limited internet address counting at most IP_SLOTS times.
    fn count_players(&mut self) {
        let mut per_ip: HashMap<Option<IpAddr>, HashSet<&String>> = HashMap::new();
        for c in self.clients.values() {
            per_ip.entry(c.limited.then_some(c.ip)).or_default().insert(&c.token);
        }
        self.players_online =
            per_ip.iter().map(|(ip, tokens)| if ip.is_some() { tokens.len().min(IP_SLOTS) } else { tokens.len() }).sum();
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
        let limited_ip = client.limited.then_some(client.ip);
        let player = self.players.get_mut(&client.token).expect("player exists for every client");
        let now = Instant::now();
        if player.ready_at.is_some_and(|t| t > now) {
            return ActResult::Cooldown;
        }
        // The address's shared cooldowns: the one that is over soonest. If none is over yet, this player
        // waits for it too.
        let slot = limited_ip.map(|ip| {
            let slots = self.ip_slots.entry(ip).or_insert([now; IP_SLOTS]);
            (0..IP_SLOTS).min_by_key(|&k| slots[k]).map(|k| (ip, k, slots[k])).unwrap()
        });
        if let Some((_, _, free_at)) = slot.filter(|&(_, _, free_at)| free_at > now) {
            player.ready_at = Some(free_at);
            player.cooldown = free_at - now;
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
        let t = self.world.ty(i);
        // Walls can also be built on from an existing wall: into open cells next to one (down into a
        // flooded shaft, say), and repaired there; never into rock or other solid ground.
        let wall_reach = kind == protocol::ACT_WALL && self.world.touches_wall(i) && (world::is_open(t) || world::is_wall(t));
        if !self.world.reachable(i) && !wall_reach {
            return ActResult::OutOfReach;
        }

        let cooldown = match kind {
            protocol::ACT_DIG => match t {
                world::WATER => {
                    player.water += 1;
                    self.world.dig(i);
                    COOLDOWN_COLLECT
                }
                // Deeper rock takes several hits (world::hits_left); each hit but the last wears it down. Heavier
                // strikes do more damage per hit.
                t if world::rock_state(t).is_some() => {
                    let (kind, left) = world::rock_state(t).unwrap();
                    let left = left.saturating_sub(1 + player.strike);
                    if left > 0 {
                        self.world.set_type(i, world::worn(kind, left));
                    } else {
                        *player.rock_kind(kind) += 1;
                        player.cells_mined += 1;
                        self.world.dig(i);
                    }
                    COOLDOWN_DIG
                }
                // Walls take three hits: intact, cracked, crumbling, gone.
                world::WALL => {
                    self.world.set_type(i, world::WALL_CRACKED);
                    COOLDOWN_DIG
                }
                world::WALL_CRACKED => {
                    self.world.set_type(i, world::WALL_CRUMBLING);
                    COOLDOWN_DIG
                }
                // Breaking a wall gives back all the rock it cost.
                world::WALL_CRUMBLING => {
                    player.rock += WALL_COST;
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
                world::AMETHYST_ORE => {
                    player.amethyst += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                world::QUARTZ_ORE => {
                    player.quartz += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                world::CORAL_ORE | world::CORAL => {
                    player.coral += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                // Lava rock takes two hits (one with heavier strikes).
                world::LAVA_ROCK if player.strike == 0 => {
                    self.world.set_type(i, world::LAVA_ROCK_CHIPPED);
                    COOLDOWN_DIG
                }
                world::LAVA_ROCK | world::LAVA_ROCK_CHIPPED => {
                    player.lava += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                world::FIRE_OPAL => {
                    player.opal += 1;
                    player.cells_mined += 1;
                    self.world.dig(i);
                    COOLDOWN_DIG
                }
                world::EMBER => {
                    player.ember += 1;
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
                } else if let Some(kind) = protocol::item_rock_kind(arg) {
                    let held = player.rock_kind(kind);
                    if *held == 0 {
                        return ActResult::NoResources;
                    }
                    *held -= 1;
                    self.world.set_type(i, world::ROCK_KINDS[kind]);
                    COOLDOWN_DIG
                } else if let Some(cell) = block_cell(arg) {
                    let held = player.found_block(arg);
                    if *held == 0 {
                        return ActResult::NoResources;
                    }
                    *held -= 1;
                    self.world.set_type(i, cell);
                    COOLDOWN_PLACE
                } else {
                    return ActResult::WrongTarget;
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
        if let Some((ip, k, _)) = slot {
            if let Some(slots) = self.ip_slots.get_mut(&ip) {
                slots[k] = now + cooldown;
            }
        }

        if self.world.ty(i) == world::AIR && y > self.world.surface[x] as usize {
            self.record_depth = self.record_depth.max((y - self.world.surface[x] as usize) as u32);
            let next = next_depth_milestone(self.milestones.depth);
            if self.record_depth >= next {
                self.milestones.depth = next;
                self.announcements.push(format!("New record depth: {next} m!"));
            }
        }
        if kind == protocol::ACT_DIG {
            let first = match t {
                world::SPONGE_ORE => Some(("spongerock", "Someone found the first spongerock!")),
                world::QUARTZ_ORE => Some(("quartz", "Someone found the first quartz!")),
                world::AMETHYST_ORE => Some(("amethyst", "Someone found the first amethyst!")),
                world::SPRING_ROCK => Some(("spring", "A spring burst open!")),
                world::CORAL_ORE => Some(("coral", "Someone found the first swellcoral!")),
                world::EMBER => Some(("ember", "Someone found the first emberstone!")),
                world::FIRE_OPAL => Some(("opal", "Someone found the first fire opal!")),
                _ => None,
            };
            if let Some((name, text)) = first {
                if self.milestones.firsts.insert(name.to_string()) {
                    self.announcements.push(text.to_string());
                }
            }
        }
        ActResult::Ok
    }

    /// A dev client's action: no cooldown, no cost, no reach or sight rules, applied to every cell of a
    /// brush x brush square around (x, y). Digging takes everything but bedrock and springs (damp rock
    /// opens its spring); walls, rubble and spongerock fill the open cells.
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
                    protocol::ACT_PLACE if open && block_cell(arg).is_some() => block_cell(arg).unwrap(),
                    protocol::ACT_PLACE if open && protocol::item_rock_kind(arg).is_some() => {
                        world::ROCK_KINDS[protocol::item_rock_kind(arg).unwrap()]
                    }
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
                let cost = (1u32 << (player.pickaxe + 1)) - 1;
                if player.quartz < cost {
                    return ActResult::NoResources;
                }
                player.quartz -= cost;
                player.pickaxe += 1;
            }
            protocol::UPGRADE_STRIKE if (player.strike as usize) < STRIKE_COSTS.len() => {
                let cost = STRIKE_COSTS[player.strike as usize];
                if player.amethyst < cost {
                    return ActResult::NoResources;
                }
                player.amethyst -= cost;
                player.strike += 1;
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
        put_u32(&mut m, p.amethyst);
        m.push(p.strike);
        put_u32(&mut m, p.dirt);
        for &n in &p.deep {
            put_u32(&mut m, n);
        }
        put_u32(&mut m, p.coral);
        put_u32(&mut m, p.ember);
        put_u32(&mut m, p.lava);
        put_u32(&mut m, p.opal);
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
        m.push(self.upwelling_phase());
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
            if !self.clients.values().any(|o| o.ip == c.ip) {
                self.ip_slots.remove(&c.ip);
            }
            self.count_players();
            for chunk in c.chunks {
                self.subscribers[chunk as usize].retain(|&s| s != id);
            }
        }
    }

    /// Save the world and players. In the background, the file writing runs on its own thread.
    /// Send one client the leaderboard and their own place on it. Worked out at most once a second, however
    /// many players ask.
    fn send_leaders(&mut self, id: u64) {
        let Some(client) = self.clients.get(&id) else { return };
        let token = client.token.clone();
        let now = Instant::now();
        if self.leaders_at.is_none_or(|t| now - t >= Duration::from_secs(1)) {
            let mut all: Vec<(u32, String)> =
                self.players.iter().filter(|(_, p)| p.cells_mined > 0).map(|(t, p)| (p.cells_mined, t.clone())).collect();
            all.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            self.leaders = all;
            self.leaders_at = Some(now);
        }
        let put_name = |m: &mut Vec<u8>, token: &str| {
            let name = display_name(token);
            m.push(name.len() as u8);
            m.extend_from_slice(name.as_bytes());
        };
        let mut m = vec![protocol::S_LEADERS];
        let top = &self.leaders[..self.leaders.len().min(LEADERS_SHOWN)];
        m.push(top.len() as u8);
        for (mined, t) in top {
            put_name(&mut m, t);
            put_u32(&mut m, *mined);
        }
        let mined = self.players.get(&token).map_or(0, |p| p.cells_mined);
        let rank = self.leaders.iter().filter(|(n, _)| *n > mined).count() + 1;
        put_u32(&mut m, rank as u32);
        put_u32(&mut m, mined);
        put_name(&mut m, &token);
        self.send(id, m);
    }

    /// Send this step's announcements to everyone.
    fn send_announcements(&mut self) {
        for text in std::mem::take(&mut self.announcements) {
            println!("announced: {text}");
            let mut m = vec![protocol::S_ANNOUNCE];
            put_u16(&mut m, text.len() as u16);
            m.extend_from_slice(text.as_bytes());
            let ids: Vec<u64> = self.clients.keys().copied().collect();
            for id in ids {
                self.send(id, m.clone());
            }
        }
    }

    fn save(&self, background: bool) {
        let world = self.world.to_bytes();
        let players = serde_json::to_vec(&self.players).expect("players serialise");
        let milestones = serde_json::to_vec(&self.milestones).expect("milestones serialise");
        let dir = self.data_dir.clone();
        let write = move || {
            let result = std::fs::create_dir_all(&dir)
                .and_then(|_| world::write_atomic(&dir.join("world.bin"), &world))
                .and_then(|_| world::write_atomic(&dir.join("players.json"), &players))
                .and_then(|_| world::write_atomic(&dir.join("milestones.json"), &milestones));
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
    fn many_players_on_one_address_share_three_cooldowns() {
        let world = World::generate(1024, 512, 3);
        let mut sim = Sim::new(world, std::env::temp_dir().join("mine-together-test-none"));
        let ip = IpAddr::from([203, 0, 113, 7]);
        let mut keep = Vec::new();
        for id in 1..=5u64 {
            let (outbox, rx) = tokio::sync::mpsc::channel(1024);
            keep.push(rx);
            sim.handle(Cmd::Connect { id, token: format!("griefer-{id}"), dev: false, ip, limited: true, outbox });
        }
        assert_eq!(sim.players_online, IP_SLOTS, "five players on one address count as three");
        // Five players dig at once: three get through, the other two wait for a shared cooldown.
        let results: Vec<ActResult> = (1..=5u64)
            .map(|id| {
                let x = sim.world.w / 2 + 20 + id as usize * 3;
                sim.act(id, protocol::ACT_DIG, x as u32, sim.world.surface[x] as u32, 0)
            })
            .collect();
        assert_eq!(results.iter().filter(|&&r| r == ActResult::Ok).count(), 3, "{results:?}");
        assert_eq!(results.iter().filter(|&&r| r == ActResult::Cooldown).count(), 2, "{results:?}");
        // Connections beyond the limit are refused.
        for id in 6..=20u64 {
            let (outbox, rx) = tokio::sync::mpsc::channel(1024);
            keep.push(rx);
            sim.handle(Cmd::Connect { id, token: format!("griefer-{id}"), dev: false, ip, limited: true, outbox });
        }
        assert_eq!(sim.clients.len(), MAX_CONNECTIONS_PER_IP);
    }

    #[test]
    fn display_names_are_stable_and_varied() {
        assert_eq!(display_name("abc"), display_name("abc"));
        let names: HashSet<String> = (0..200).map(|k| display_name(&format!("token-{k}"))).collect();
        assert!(names.len() > 190, "only {} different names for 200 players", names.len());
        assert!(names.iter().all(|n| n.split(' ').count() == 3));
    }

    #[test]
    fn depth_milestones() {
        assert_eq!(next_depth_milestone(0), 25);
        assert_eq!(next_depth_milestone(25), 50);
        assert_eq!(next_depth_milestone(99), 100);
        assert_eq!(next_depth_milestone(100), 150);
        assert_eq!(next_depth_milestone(260), 300);
    }

    #[test]
    fn springs_are_gentler_for_small_groups() {
        assert!((spring_amount(1) - 0.2).abs() < 0.01);
        assert!((spring_amount(10) - 0.6).abs() < 0.01);
        assert_eq!(spring_amount(100), 1.0);
    }

    #[test]
    fn the_upwelling_shows_only_while_it_is_on() {
        let mut sim = Sim::new(World::generate(1024, 512, 3), std::env::temp_dir().join("mine-together-test-none"));
        assert_eq!(*sim.stats().last().unwrap(), 0, "no Upwelling on a fresh server");
        sim.stir(false);
        assert_eq!(*sim.stats().last().unwrap(), 1);
        for _ in 0..=STIR_TICKS {
            sim.ticks += 1;
            sim.upwelling();
        }
        assert_eq!(*sim.stats().last().unwrap(), 2);
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
        let ip = IpAddr::from([127, 0, 0, 1]);
        sim.handle(Cmd::Connect { id: 1, token: "tester".into(), dev: false, ip, limited: false, outbox });
        let (x, y) = (sim.world.w / 2, sim.world.surface[sim.world.w / 2] as usize - 1);
        sim.players.get_mut("tester").unwrap().rock = 12;
        assert_eq!(sim.act(1, protocol::ACT_WALL, x as u32, y as u32, 0), ActResult::Locked);
        assert_eq!(sim.buy(1, protocol::UPGRADE_WALLS), ActResult::Ok);
        assert_eq!(sim.players["tester"].rock, 2);
        assert_eq!(sim.buy(1, protocol::UPGRADE_WALLS), ActResult::WrongTarget, "already unlocked");

        let p = sim.players.get_mut("tester").unwrap();
        p.quartz = 6;
        assert_eq!(sim.buy(1, protocol::UPGRADE_PICKAXE), ActResult::Ok); // level 1 costs 1
        assert_eq!(sim.buy(1, protocol::UPGRADE_PICKAXE), ActResult::Ok); // level 2 costs 3
        assert_eq!(sim.buy(1, protocol::UPGRADE_PICKAXE), ActResult::NoResources);
        assert_eq!(sim.players["tester"].pickaxe, 2); // level 3 costs 7, 2 left
        assert_eq!(sim.players["tester"].quartz, 2);

        sim.players.get_mut("tester").unwrap().amethyst = 12;
        assert_eq!(sim.buy(1, protocol::UPGRADE_STRIKE), ActResult::Ok); // 3
        assert_eq!(sim.buy(1, protocol::UPGRADE_STRIKE), ActResult::NoResources); // 10 more needed, 9 left
        assert_eq!(sim.players["tester"].strike, 1);
    }
}
