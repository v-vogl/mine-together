# Commands

Everything you type to run, test and share the game. All commands are for the Windows **Command Prompt** (cmd).
Each block that starts a program needs its own Command Prompt window, and the program keeps running in it until you
press **Ctrl+C**.

## Play locally (alone or for testing)

Two windows.

**Window 1, the game server:**
```
cd C:\Users\voglw\mine-together\server
cargo run --release
```
The first start after a code change takes a minute (it rebuilds). It is ready when it says `listening on 0.0.0.0:8080`.

**Window 2, the client (the web page):**
```
cd C:\Users\voglw\mine-together\client
npm run dev
```
Then open http://localhost:5173 in the browser. Client changes show up by themselves; server changes need a server
restart (Ctrl+C, then `cargo run --release` again).

## Dev client (testing powers)

With both windows above running, open the dev link the server printed at start:
```
http://localhost:5173/?dev=KEY
```
The key is in `server\data\dev-key.txt`. Dev powers: no cooldowns or costs, no reach rules, all tools, brush from 1x1
to 41x41 (slider, or the `[` and `]` keys). Never share the key: anyone with it can reshape the world.
To make a new key: delete `server\data\dev-key.txt` and restart the server.

## Play with friends (Cloudflare tunnel)

Stop the local server first (both want port 8080). Then one window:
```
cd C:\Users\voglw\mine-together
share.cmd
```
It builds the page, starts the server in a second window, and shows a link like
`https://random-words.trycloudflare.com`. Send that link to your friends. Dev client: the same link with
`?dev=KEY` at the end. The link changes every time and works only while both windows and your PC are running.

One-time install of the tunnel program (then open a new Command Prompt):
```
winget install --id Cloudflare.cloudflared
```

## Commands in the server window

Type these into the server's window and press Enter:

| Command | What it does |
|---|---|
| `rain` | start a random rain shower (`rain light`, `rain normal`, `rain heavy` for a strength) |
| `save` | save the world now (it also saves every minute and when it stops) |
| `stats` | show players online, record depth and load |
| `quit` | save and stop (same as Ctrl+C) |

## New world / reset players

Stop the server first (Ctrl+C or `quit`), otherwise it saves the old world again. Then:
```
cd C:\Users\voglw\mine-together\server
ren data\world.bin world-old1.bin
ren data\players.json players-old1.json
```
Use a number that is not taken yet (`dir data` lists the files). The first line gives a new map, the second resets
everyone's rock, quartz and upgrades; use just the one you need. Start the server again and it makes a new world.
"The system cannot find the file specified" means there is no `world.bin` right now, so the next start makes a new
world anyway.

Reset only yourself (in the browser): F12, Console tab, type `localStorage.removeItem("mt-token")`, Enter, reload.

## Map picture

```
cd C:\Users\voglw\mine-together
node tools/map.mjs
```
Draws the last saved world, hidden parts included, to `server\data\map.png`. Type `save` in the server window first
for an up-to-date picture. Zoomed in on one area (4x, from x 880, y 150, 240 wide, 130 high):
```
node tools/map.mjs 4 server/data/world.bin server/data/map-zoom.png 880,150,240,130
```

## Bots

With the server running, in another window:
```
cd C:\Users\voglw\mine-together
node tools/bots.mjs 100 60
```
100 bots for 60 seconds. Pick the roles (numbers are proportions, not counts):
```
node tools/bots.mjs 500 300 --roles=digger:6,explorer:2,builder:1,capper:0.5,flooder:0.5
```
Roles: `digger`, `explorer`, `builder` (defends), `capper` (griefer: walls over shafts), `flooder` (griefer: breaks
through to the ocean). Order is always: number of bots, seconds, then options.

## Checks after code changes

```
cd C:\Users\voglw\mine-together\server
cargo test

cd C:\Users\voglw\mine-together\client
npm run check
```

## Online server (netcup VPS)

Not set up yet; filled in once the server is ready. Planned:

```
cd C:\Users\voglw\mine-together
.\deploy\deploy.ps1 -Server IP -Setup DOMAIN
```
first time (in PowerShell, not cmd), and for every update after that:
```
.\deploy\deploy.ps1 -Server IP
```
Server console from your PC: `ssh root@IP`, then `echo rain | sudo tee /run/mine-together/console`.
