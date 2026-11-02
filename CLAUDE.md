# Mine Together

A multiplayer browser game inspired by r/place: thousands of players share one huge side-view pixel world and dig down
together, one pixel at a time, on a cooldown. Working title "Mine Together".

The full concept, technical plan and open questions are in [docs/DESIGN.md](docs/DESIGN.md). Read it before starting
work. Gameplay ideas marked *proposal* there are not decided yet; ask before building them.

## Core rules (decided)

- Side view, like an ant farm: the surface is at the top, players dig downward. The goal is to dig as deep as possible.
- Only exposed cells (next to air) are visible; the next 1-2 cells behind them are shown darkened; everything else is
  black. Hidden cells never leave the server, so materials cannot be found with the browser dev tools.
- Players act only on visible cells that touch open air, with a cooldown per action: only the top layer of a lake can be
  scooped, and a lake bed cannot be dug while water covers it.
- Sight goes through air without limit but only 4 cells deep into water, and always follows the current world: when a
  hole floods, what is deep under the water goes dark again.
- Cell types: air, rock (mined for a resource, can be placed back), water (falls and spreads every server tick; scooped
  water can be poured out again), wall (built by players
  with mined rock; three hits to break, repairable; unlocked for 10 rock), materials found by depth (spongestone,
  which soaks up water once placed; quartz, the lowest tier of upgrade crystals, spent on pickaxe upgrades in the
  Upgrades panel), rubble (dropped for 1 rock; falls, sinks through water and heaps up like gravel, to fill flooded
  shafts and plug leaks), damp rock (opens a spring when dug: it pours water until its sides are plugged), cracked
  ceiling (rooms wider than 30 cells cave in as rubble unless propped up), bedrock (unbreakable; the world's bottom).
  Gems as paint were removed.
- The world is an island in an endless ocean (Made in Abyss style): ramps rise from the shore to a rim, a hilly bowl lies
  between the rims, players start digging at its bottom. The ocean at the edges is endless and can be breached by
  digging (it pours in at a limited rate, so a crowd can stop it); the world's edge is never visible.
- Only open space connected to the sky or the ocean is lit: a tunnel sealed with a wall goes dark and out of reach.
- Water comes from pockets found while digging and from rain events at the surface. Players can collect water cell by
  cell, like mining.
- Cooldowns: about 1-3 seconds for mining and collecting water, longer for building. They scale with the players online: twice as fast
  alone, normal at 100 players, half as fast at 1000.
- Intended player moments: panic when someone (by accident or as sabotage) floods the dig and everyone rushes to fix the
  leak; frenzy and chaos when a new material is uncovered. Trolling is part of the game, but it must stay fixable.

- First launch target: a streamer with about 500 viewers sharing the link; see "Launch scenario" in docs/DESIGN.md.
  Getting in must take one click: open the link, click a pixel.

## Technical decisions

- Client: TypeScript + plain WebGL2, no game engine. The visible world is a texture with one byte per cell; colours,
  per-cell shade variation, fog, water shimmer and material textures are done in the shader. UI in plain HTML/CSS.
- Server: Rust, WebSocket, authoritative. One simulation thread owns the world; the world is split into chunks (about
  64x64), and only chunks with moving water are simulated (falling-sand cellular automaton, about 10 ticks per second).
- Network: clients subscribe to the chunks in their viewport, receive a compressed snapshot once, then per-tick binary
  diffs of changed cells. Only visible information is sent.
- Persistence: periodic world snapshots to disk; a small database for accounts and inventories.
- Hosting (later): one VPS for the game server, the static client on Cloudflare Pages.
- Before going online, load-test with simulated bot clients (target: 1000+ concurrent players).

## Layout and running

- [docs/COMMANDS.md](docs/COMMANDS.md) is the user's cheat sheet of every command (local play, dev client, sharing,
  server console, resets, map, bots, deploy). Keep it up to date whenever a command, script or setting changes.

- `server/` Rust game server. `cargo run --release` (in `server/`) listens on port 8080 and saves to `server/data/`.
  Console commands in its window: `rain`, `save`, `stats`, `quit`. `cargo test` runs the world tests.
- `client/` TypeScript + WebGL2 client. `npm run dev` (in `client/`) serves it on http://localhost:5173 and passes
  `/ws` on to the server. `npm run check` type-checks.
- Dev mode for testing alone: the server prints a link `http://localhost:5173/?dev=KEY` (key in
  `server/data/dev-key.txt`, made on first start). With it: no cooldowns or costs, no reach rules, all tools, and a
  brush up to 41x41 (slider, or `[` `]`). The server checks the key; never share it.
- `tools/map.mjs` draws the whole saved world, hidden parts included, to `server/data/map.png`: `node tools/map.mjs`.
- `tools/bots.mjs` bots for load tests and playtests: `node tools/bots.mjs 500 60` runs 500 bots for 60 seconds against
  the local server. Roles: digger, explorer, builder (defends), capper and flooder (griefers); pick a mix with
  `--roles=digger:6,flooder:1`.
- `share.cmd` plays with friends from this PC before the VPS is ready: builds the client, starts the server (with
  `STATIC_DIR` so it serves the page too, `HOST=127.0.0.1`) and a Cloudflare quick tunnel that prints a public
  `https://....trycloudflare.com` link. Needs `cloudflared` (`winget install --id Cloudflare.cloudflared`).
- `deploy/` puts the game on a Linux server (netcup VPS, Ubuntu 24.04): Caddy serves the page and HTTPS and passes `/ws`
  to the game server, which runs as the systemd service `mine-together`. First time:
  `.\deploy\deploy.ps1 -Server IP -Setup DOMAIN`; updates: `.\deploy\deploy.ps1 -Server IP`. Console on the server:
  `echo rain | sudo tee /run/mine-together/console`. World data and daily backups: `/var/lib/mine-together`.
- Rust is installed via rustup in `%USERPROFILE%\.cargo\bin`; Git Bash may need `export PATH="$HOME/.cargo/bin:$PATH"`.

## Working with the user

- Solo developer on Windows 11. The user describes ideas and tests the game; explain technical terms in plain words.
- The user tests by playing and sending screenshots. Ask before launching or automating a browser or the game.
- Commit only when asked. Ask before large downloads, paid services, deploying anything public, or creating accounts.
- Separate project from `C:\Users\voglw\VCMI-RPG` (a VCMI fork); do not touch that folder from here.
