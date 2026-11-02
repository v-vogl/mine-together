//! Mine Together game server: loads or generates the world, runs the simulation thread and accepts
//! WebSocket connections on /ws.
//!
//! Settings come from environment variables: HOST (default 0.0.0.0; 127.0.0.1 behind a proxy),
//! PORT (default 8080), DATA_DIR (default "data"), CONSOLE_FIFO (a named pipe to read console commands
//! from when running as a service, e.g. `echo rain > /run/mine-together/console`),
//! WORLD_W (default 2048) and WORLD_H (default 1024): multiples of 64, used only when generating a new world,
//! STATIC_DIR (optional): also serve the built client from this folder, so one program serves the whole
//! game (used by share.cmd; on the VPS, Caddy serves the client instead).
//!
//! TRUST_IP_HEADER (optional): the header that carries the player's real internet address when a proxy on
//! this machine passes the connections on: `cf-connecting-ip` behind the Cloudflare tunnel, `x-forwarded-for`
//! behind Caddy. Only set it when such a proxy is in front, or players could fake their address with it.
//!
//! Dev mode: on first start the server writes a random key to DATA_DIR/dev-key.txt. A client that connects
//! with `?dev=KEY` in its address gets dev powers (no cooldowns, no costs, a big brush; see Sim::dev_act).
//! Keep the key to yourself: anyone who has it can reshape the world. Bots that connect with `?bot=KEY` get
//! no powers but skip the per-address limits, for load tests from one machine.

mod noise;
mod protocol;
mod sim;
mod world;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use sim::Cmd;
use std::collections::HashMap;
use std::io::BufRead;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

/// Messages queued for one client before it counts as too slow and gets disconnected.
const OUTBOX_SIZE: usize = 256;

#[derive(Clone)]
struct AppState {
    cmds: mpsc::Sender<Cmd>,
    dev_key: Arc<String>,
    trust_ip_header: Option<Arc<String>>,
    next_id: Arc<AtomicU64>,
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[tokio::main]
async fn main() {
    let host: String = env_or("HOST", "0.0.0.0".to_string());
    let port: u16 = env_or("PORT", 8080);
    let data_dir = PathBuf::from(env_or("DATA_DIR", "data".to_string()));
    let world_path = data_dir.join("world.bin");

    let world = match world::World::load(&world_path) {
        Ok(w) => {
            println!("loaded world {}x{} from {}", w.w, w.h, world_path.display());
            w
        }
        Err(e) => {
            if world_path.exists() {
                eprintln!("could not load {}: {e}", world_path.display());
                std::process::exit(1);
            }
            let (w, h) = (env_or("WORLD_W", 2048usize), env_or("WORLD_H", 1024usize));
            let seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as u64;
            let started = Instant::now();
            let world = world::World::generate(w, h, seed);
            println!("generated new world {w}x{h} (seed {seed}) in {:?}", started.elapsed());
            // Save it right away, so world.bin (and the map tool) has it from the start.
            let _ = std::fs::create_dir_all(&data_dir);
            if let Err(e) = world::write_atomic(&world_path, &world.to_bytes()) {
                eprintln!("could not save {}: {e}", world_path.display());
            }
            world
        }
    };

    let dev_key = load_dev_key(&data_dir);
    println!("dev client (keep this link to yourself): http://localhost:5173/?dev={dev_key}");

    let (cmds, rx) = mpsc::channel();
    let sim = sim::Sim::new(world, data_dir);
    std::thread::Builder::new().name("sim".into()).spawn(move || sim.run(rx)).unwrap();

    // Console commands typed into the server window, or written into the console pipe on a server.
    let console = cmds.clone();
    std::thread::spawn(move || read_console(std::io::stdin().lock(), &console));
    if let Ok(fifo) = std::env::var("CONSOLE_FIFO") {
        let console = cmds.clone();
        std::thread::spawn(move || loop {
            // Opening a named pipe waits for a writer; each `echo ... >` ends with end-of-file.
            match std::fs::File::open(&fifo) {
                Ok(f) => {
                    if !read_console(std::io::BufReader::new(f), &console) {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("console pipe {fifo}: {e}");
                    break;
                }
            }
        });
    }

    let on_ctrl_c = cmds.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = on_ctrl_c.send(Cmd::Shutdown);
        }
    });

    let trust_ip_header = std::env::var("TRUST_IP_HEADER").ok().filter(|h| !h.is_empty()).map(|h| Arc::new(h.to_lowercase()));
    if let Some(h) = &trust_ip_header {
        println!("players' addresses come from the {h} header");
    }
    let state =
        AppState { cmds, dev_key: Arc::new(dev_key), trust_ip_header, next_id: Arc::new(AtomicU64::new(1)) };
    let mut app = Router::new().route("/ws", get(ws_handler)).with_state(state);
    if let Ok(dir) = std::env::var("STATIC_DIR") {
        println!("serving the client from {dir}");
        app = app.fallback_service(tower_http::services::ServeDir::new(dir));
    }
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await.unwrap();
    println!("listening on {host}:{port}; console commands: rain [light|normal|heavy], save, stats, quit");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
}

/// The dev key from DATA_DIR/dev-key.txt, made up and saved on first start.
fn load_dev_key(data_dir: &std::path::Path) -> String {
    let path = data_dir.join("dev-key.txt");
    if let Ok(key) = std::fs::read_to_string(&path) {
        let key = key.trim().to_string();
        if key.len() >= 16 {
            return key;
        }
    }
    let seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as u64
        ^ (std::process::id() as u64) << 32;
    let mut rng = noise::Rng::new(seed);
    let key = format!("{:016x}{:016x}", rng.next(), rng.next());
    let _ = std::fs::create_dir_all(data_dir);
    if let Err(e) = std::fs::write(&path, &key) {
        eprintln!("could not save {}: {e}", path.display());
    }
    key
}

/// Pass console lines to the simulation. Returns false once the simulation is gone.
fn read_console(input: impl BufRead, console: &mpsc::Sender<Cmd>) -> bool {
    for line in input.lines().map_while(Result::ok) {
        let cmd = if line.trim() == "quit" { Cmd::Shutdown } else { Cmd::Console(line) };
        if console.send(cmd).is_err() {
            return false;
        }
    }
    true
}

/// The player's internet address: the trusted proxy header if one is configured and the connection comes
/// from this machine (the proxy), otherwise the address the connection comes from.
fn client_ip(peer: SocketAddr, headers: &HeaderMap, trusted: Option<&str>) -> IpAddr {
    let from_header = trusted.filter(|_| peer.ip().is_loopback()).and_then(|name| {
        let value = headers.get(name)?.to_str().ok()?;
        value.split(',').next()?.trim().parse().ok()
    });
    from_header.unwrap_or(peer.ip())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Response {
    // Guests are identified by a random token their browser keeps; anything odd gets a fresh one.
    let token = query
        .get("token")
        .filter(|t| (8..=64).contains(&t.len()) && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .cloned()
        .unwrap_or_else(|| format!("anon-{}", state.next_id.fetch_add(1, Ordering::Relaxed)));
    let dev = query.get("dev").is_some_and(|k| *k == *state.dev_key);
    let bot = query.get("bot").is_some_and(|k| *k == *state.dev_key);
    let ip = client_ip(peer, &headers, state.trust_ip_header.as_deref().map(|h| h.as_str()));
    // Players on this machine (local testing) are not limited.
    let limited = !(dev || bot || ip.is_loopback());
    ws.max_message_size(4096).on_upgrade(move |socket| handle_socket(socket, token, dev, ip, limited, state))
}

async fn handle_socket(socket: WebSocket, token: String, dev: bool, ip: IpAddr, limited: bool, state: AppState) {
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = socket.split();
    let (outbox, mut out_rx) = tokio::sync::mpsc::channel(OUTBOX_SIZE);
    if state.cmds.send(Cmd::Connect { id, token, dev, ip, limited, outbox }).is_err() {
        return;
    }

    // The writer ends when the simulation drops this client's outbox.
    let writer = tokio::spawn(async move {
        while let Some(bytes) = out_rx.recv().await {
            if sink.send(Message::Binary(bytes)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Binary(data) => {
                let Some(msg) = protocol::parse_client(&data) else { break };
                if state.cmds.send(Cmd::Client { id, msg }).is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
        if writer.is_finished() {
            break;
        }
    }
    let _ = state.cmds.send(Cmd::Disconnect { id });
    writer.abort();
}
