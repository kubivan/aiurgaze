# AI Copilot Instructions for aiurgaze

## Project Overview

**aiurgaze** is a Rust-based StarCraft II game viewer and launcher built with the **Bevy** game engine. It runs a headless SC2 instance in Docker and visualizes games for bots (vs AI or bot vs bot).

### Key Architecture Pattern: Event-Driven Data Flow

The project uses **Bevy's Entity Component System (ECS)** with async task spawning via `bevy_tokio_tasks`. Critical data flows through:

1. **Docker Container** → SC2 Game Server (WebSocket on port 5555)
2. **ProxyWS** → Intercepts and bridges SC2 API traffic
3. **Events** → Emitted by proxy/observer, processed by Bevy systems
4. **Visualization** → UI systems render game state and handle user input

## Essential Build & Runtime Commands

```bash
# Development build & run (with local resources flag)
AIURGAZE_LOCAL_RESOURCES=1 cargo run --release

# Build SC2 Docker image (required prerequisite)
cd docker && docker build -t minimal-sc2 .

# Production install
cargo install --path .
```

**Critical Environment Variables:**
- `AIURGAZE_LOCAL_RESOURCES=1`: Uses repo `data/` and `assets/` dirs instead of XDG paths (essential for dev)
- `RUST_LOG`: Set log level (e.g., `debug`, `info`)

## Core Module Architecture

### Configuration Layer (`app_settings.rs`)
- **XDG compliance**: Uses `~/.config/aiurgaze/config.toml` (user) and `~/.local/share/aiurgaze/` (data)
- **Fallback logic**: Environment variable override (`AIURGAZE_LOCAL_RESOURCES=1`) → XDG → HOME-based defaults
- **Key struct**: `AppSettings` (loaded at startup, includes `StarcraftConfig`, `MapConfig`, window settings)
- **Common pattern**: Pass `Res<AppSettings>` to systems that need config

### Event & Data Sources (`proxy_ws.rs`, `controller.rs`)
- **ProxyWS**: Accepts one client connection, bridges it to upstream SC2 server
  - Filters responses, triggers `ProxyResponseEvent` on main thread
  - Protocol: SC2 API over WebSocket (protobuf messages)
- **ObserverClient**: Parallel connection for direct observation (independent of client proxy)
- **Event types**: `ProxyResponseEvent` includes source (BotProxy/DirectObserver) + Response

### Game State Management (`entity_system.rs`, `units.rs`)
- **EntitySystem**: Central resource holding pre-loaded unit/ability data, icons, map config
  - Data from `data/data.json` and `data/entities.toml`
  - Initialized once; accessed by all rendering/gameplay systems
- **UnitRegistry**: Maps unit tags to live entity state (position, health, orders)
- **Handle observations**: `handle_observation()` updates live unit state from SC2 API responses

### Rendering & Interaction (`ui.rs`, `map.rs`)
- **Bevy EGUI integration** for UI panels (game config, unit details, status)
- **Tilemap rendering**: `spawn_tilemap()` loads Tiled/LDtk maps, renders terrain + overlays (visibility, creep)
- **Camera controls**: Middle-mouse drag to pan, scroll to zoom
- **UI state machine**: `AppState::StartScreen` ↔ `AppState::GameScreen`

### Bot Process Management (`bot_runner.rs`)
- Spawns bot processes via CLI commands (stored in `GameConfigPanel`)
- Tracks process status; emits `StartBotProcessesEvent`

## Project-Specific Patterns & Conventions

### 1. **UI-Driven Startup**
   - User configures game in UI, then creates game
   - Pending requests are produced by UI interactions and stored in `PendingCreateGameRequest`

### 2. **Async Tasks in Bevy**
   - Spawned via `TokioTasksRuntime::spawn_background_task()`
   - Use closure `|ctx|` to queue events back to main thread: `ctx.run_on_main_thread(|world| ...)`
   - Never block main thread; always use `tokio::task::spawn_blocking()` for sync work

### 3. **Configuration & Data File Locations**
   - **Runtime config**: `config.toml` (parsed into `AppSettings`)
   - **Unit/ability data**: `data/data.json` (loaded into `EntitySystem`)
   - **Display info**: `data/entities.toml` (icon paths, tile sizes per unit)
   - **Map assets**: `maps/*.sc2map` mounted in Docker container
   - **Unit icons**: `assets/units/*.webp` loaded as Bevy handles

### 4. **Error Handling**
   - Use `thiserror` crate for domain errors
   - Docker failures: Check `DockerStatus` resource (Starting, Running, NotFound, Error)
   - Network failures: Proxy retries 5 times with 2-sec delays before failing

### 5. **Multi-Source Observation**
   - `ObservationSource` enum: BotProxy (client's messages) vs DirectObserver (independent feed)
   - `ActiveObservationSource` tracks current source to avoid conflicts
   - Responses tagged by source for debugging

## Preferred Coding Techniques

### Code Organization
- **Small, standalone functions**: Break logic into focused functions with single responsibilities
  - Prefer functions that fit on one screen (~20-40 lines)
  - Extract complex logic into helper functions with descriptive names
   - Example: `start_server_container()` and `docker_startup_system()` in [src/main.rs](src/main.rs)

### Control Flow
- **Early returns/termination**: Use guard clauses to avoid deep nesting
  ```rust
  // ✅ Preferred
  pub fn process_unit(unit: &Unit) -> Result<()> {
      if unit.health == 0 { return Ok(()); }
      if !unit.is_visible { return Ok(()); }
      // main logic here
  }
  
  // ❌ Avoid
  pub fn process_unit(unit: &Unit) -> Result<()> {
      if unit.health > 0 {
          if unit.is_visible {
              // main logic deeply nested
          }
      }
  }
  ```

### Error Handling
- Use `?` operator for propagating errors in functions that return `Result`
- Pattern match with early returns: `let Ok(value) = result else { return; }`
- Log errors before returning: `eprintln!("[context] Error: {e}")`

### Bevy System Patterns
- Query destructuring: `Query<(&ComponentA, &mut ComponentB)>` at function signature
- Resource extraction: `Res<T>` for read-only, `ResMut<T>` for mutable
- Early exits from systems: `let Ok(value) = query.get_single() else { return; }`

## Key File References

| File | Purpose |
|------|---------|
| [src/main.rs](src/main.rs) | Docker startup, proxy wiring, Bevy app setup |
| [src/app_settings.rs](src/app_settings.rs) | XDG config loading, path resolution |
| [src/proxy_ws.rs](src/proxy_ws.rs) | WebSocket proxy for SC2 API traffic |
| [src/controller.rs](src/controller.rs) | Response event handling, terrain updates |
| [src/entity_system.rs](src/entity_system.rs) | Unit/ability data registry, icons |
| [src/units.rs](src/units.rs) | Live unit state, health bars, selections |
| [src/ui.rs](src/ui.rs) | EGUI panels, game creation flow, camera |
| [src/map.rs](src/map.rs) | Tilemap rendering, terrain layers |
| [config.toml](config.toml) | Runtime defaults (Docker port, window size, game config) |
| [data/data.json](data/data.json) | SC2 unit/ability metadata |

## Common Development Tasks

**Adding a new UI panel:**
1. Create struct in `src/ui/` with `Resource` derive
2. Add to `ui_system` parameters
3. Render with `egui::Window::new()` in system

**Updating unit rendering:**
1. Modify `handle_observation()` in `units.rs` to extract new fields
2. Add components to spawned unit entities
3. Create drawing system or update existing draw systems

**Debugging Docker/Network:**
- Check `DockerStatus` in logs: `[docker_startup_system]`
- Proxy startup: `[setup_proxy]` and `[setup_observer]` debug prints
- Network failures logged with source (ProxyWS retries, observer timeouts)

**Testing game creation flow:**
- Use `AIURGAZE_LOCAL_RESOURCES=1 cargo run --release` locally
- Verify `game_config_panel.rs` (maps, AI difficulty, fog settings)
