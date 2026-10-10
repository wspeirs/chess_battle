# Chess Battle Arena — Plan

Linux desktop app. Two UCI engines play each other on a central board. Six half-size side boards and two text panels show each engine's top-3 principal variations and search stats live. A strength bar, fed by a separate low-cost evaluator engine, sits left of the main board.

## Decisions

| Topic | Decision |
|---|---|
| Platform | Linux only |
| Shell | Rust + Tauri 2, frontend Vanilla TypeScript + Vite |
| Board UI | `@lichess-org/chessground` (GPL-3.0-or-later) → this repo becomes **GPL-3.0** |
| Rules | `shakmaty` is authoritative for everything it covers (see [shakmaty usage](#shakmaty-usage)). Only repetition history and PGN text output are hand-written, because shakmaty doesn't provide them. |
| UCI | `vampirc-uci` for both directions: `parse_one` for engine→GUI, `UciMessage` + `Display` for GUI→engine. Default features only (no `chess` feature; shakmaty is the move type). |
| Side boards | 6 total. **Left** column = engine playing the **top** color, **right** column = engine playing the **bottom** color. PV1–PV3 top-to-bottom. Swaps on flip. **Can be turned off**, which also drops engines to `MultiPV=1`. |
| PV text | **Above** main board = top-color engine, **below** = bottom-color engine. Each shows PV1–3 in SAN (PV1 only when side boards are off) plus stats. Swaps on flip. |
| Strength bar | Vertical bar left of the main board. Fed by a third UCI process ("evaluator"), Stockfish by default at low settings. |
| Search limit | Per engine: `movetime` (ms) / `depth` / `nodes`; default movetime 2000 ms |
| Turn-by-turn | Engine to move runs `go infinite` (live side boards); **Next** sends `stop` and plays `bestmove` |
| Engines | Presets: Stockfish, Lc0, GNU Chess, Berserk, Ethereal, Obsidian, Caissa, Koivisto |
| Binaries | Preset auto-fills by searching `PATH`; user can browse for binary (and Lc0 weights). No downloading. |
| Start position | Standard, or user-supplied FEN (PGN gets `SetUp`/`FEN` tags) |

## Architecture

```
┌──────────────── Tauri webview (TS) ────────────────┐
│ chessground ×7, eval bar, panels, toolbar, <dialog>│
└──────▲ events (position, engine-info, eval, …) ────┘
       │ commands (new_game, next, set_mode, …)
┌──────┴──────────── Rust backend ───────────────────┐
│ arena.rs     orchestrator / state machine          │
│ evaluator.rs strength-bar engine driver            │
│ engine.rs    UCI process (tokio + vampirc-uci)     │
│ game.rs      shakmaty game, repetition, PGN        │
│ presets.rs   engine presets + PATH lookup          │
│ config.rs    persisted settings (JSON)             │
└────────────────────────────────────────────────────┘
```

Rust owns all state; the frontend only renders events and sends commands. No rules logic in TS.

### Crates / packages

- Rust: `tauri` 2, `tauri-plugin-dialog`, `tokio` (`process`, `io-util`, `sync`, `time`, `macros`), `serde`, `serde_json`, `shakmaty`, `vampirc-uci`.
- npm: `@tauri-apps/api`, `@tauri-apps/plugin-dialog`, `@lichess-org/chessground`, `vite`, `typescript`.

## Backend

### `engine.rs`: one UCI engine process (used 3×: white, black, evaluator)

- Spawn the binary with preset args (e.g. `gnuchess --uci`), piped stdin/stdout, `kill_on_drop(true)`. Stderr goes to the app log.
- Reader task: each stdout line → `vampirc_uci::parse_one` → `UciMessage` → `mpsc` to its owner. `UciMessage::Unknown` lines are logged and dropped.
- Writer: `UciMessage::{Uci, IsReady, UciNewGame, SetOption, Position, Go, Stop, Quit}` written via `Display` + `\n`.
- Handshake: `uci` → collect `Id` and `Option(UciOptionConfig)` → `UciOk` (timeout 10 s) → `setoption` for configured options **only if the engine declared them** (warn on unknown, never send blindly) → `isready`/`readyok`.
- Per game: `ucinewgame` + `isready`. Per search: `position fen <start> moves …` then `go <limit>` or `go infinite`.
- `set_multipv(n)`: sends `setoption MultiPV` only while idle (UCI forbids it mid-search). No-op if the engine doesn't declare `MultiPV`.
- Info mapping: `UciInfoAttribute::{Depth, SelDepth, MultiPv, Score, Nodes, Nps, HashFull, TbHits, Time, Pv}` → one `InfoUpdate` struct. `wdl` arrives as `UciInfoAttribute::Any("wdl", …)` (verify, see R1). PV `UciMove`s convert to shakmaty via `shakmaty::uci::UciMove::from_ascii(m.to_string().as_bytes())`.
- Ponder always off.
- Shutdown: `stop`, `quit`, wait 1 s, then kill.

### shakmaty usage

| Need | shakmaty API |
|---|---|
| Position, legality, playing moves | `Chess`, `Position::{legal_moves, play, play_unchecked, turn, is_check}` |
| Engine move → legal move (rejects illegal `bestmove`/PV moves) | `uci::UciMove::from_ascii` + `UciMove::to_move(&pos)` |
| Move → UCI for `position … moves` | `Move::to_uci(CastlingMode::Standard)` |
| SAN with `+`/`#` for PVs, move list, PGN | `san::SanPlus::from_move_and_play_unchecked` |
| Start FEN parse/validate, FEN output, PV end FENs | `fen::Fen::from_ascii` → `into_position(CastlingMode::Standard)`; `Fen::from_position(pos, EnPassantMode::Legal)` |
| Checkmate / stalemate / insufficient material | `Position::outcome()`, `is_insufficient_material()` |
| 50-move rule | `Position::halfmoves() >= 100` |
| Threefold repetition key | `zobrist_hash::<Zobrist64>(EnPassantMode::Legal)` |
| PGN result token | `KnownOutcome` `Display` (`1-0`, `0-1`, `1/2-1/2`) |
| Last-move squares for chessground | `Move::{from, to}` |

Hand-written because shakmaty doesn't cover it:
- Repetition: `Vec<Zobrist64>` of positions since the last irreversible move; 3 matches = draw.
- PGN writer: about 40 lines of string building. `pgn-reader` only parses, and PGN import is out of scope.

### `game.rs`: game model

- Start `Chess` + start FEN + played moves (`Move`, SAN, eval/depth for PGN comments) + hash history.
- Game over: shakmaty outcome; 50-move rule and threefold repetition are **auto-adjudicated as draws** because engines can't claim them.
- Forfeits: illegal `bestmove`, `bestmove (none)` when legal moves exist, or engine crash → loss for that side, recorded in `Termination`.
- PV → SAN + end FEN: replay from the root with `to_move`; truncate at the first illegal move.
- Scores normalized to White's perspective.
- PGN: Seven Tag Roster + `White/Black` = engine `id name`, `SetUp`/`FEN` when non-standard, `Termination`, `PlyCount`; each move with a `{+0.35/22}` comment.

### `arena.rs`: orchestrator

State: `Idle | Thinking { side, search_id } | AwaitingNext { side, search_id } | Stopping | GameOver { result, reason }`.

- **play-to-end**: side to move gets `go <limit>`; on `bestmove` play it, emit, next side. Repeats until game over.
- **turn-by-turn**: side to move gets `go infinite` → `AwaitingNext`. **Next** → `stop` → `bestmove` → play → next side starts `go infinite`.
- **Restart current search** (shared path): `stop` → discard the stale `bestmove` → apply pending settings → `position` + `go`. Used for:
  - mode switch mid-game (turn-by-turn doubles as "pause");
  - side-board toggle mid-search (`MultiPV` 3↔1).
  The idle engine just gets the new `MultiPV` before its next search.
- **Move now** in play-to-end sends `stop` and plays the result (escape hatch for slow `depth`/`nodes` limits).
- Race safety: every search has an id. `info`/`bestmove` from an old search are dropped. A new `position` is never sent until the previous `bestmove` is consumed.
- Throttle: keep latest info per `(side, multipv)`; emit at most every 100 ms per engine.
- Each new position (start, every move) is also sent to `evaluator.rs`.
- Idle engine's panels/boards keep its last search output.

### `evaluator.rs`: strength bar

- Own `engine.rs` instance; default Stockfish with `Threads=1`, `Hash=16`, `MultiPV=1`, `UCI_ShowWDL=true`, limit `go depth 14` (each new position takes tens of milliseconds single-threaded [INFERENCE: typical on modern CPUs]).
- On each new position: `stop` the previous search if running → discard its result → `position` + `go`. Emits throttled `eval` on every depth increase, so the bar settles as depth grows.
- Bar value is the White expected score:
  - WDL present: `(W + D/2) / 1000`.
  - Otherwise: `cp` → lichess win% curve, `50 + 50·(2/(1+e^(−0.00368208·cp)) − 1)`.
  - Mate: 100 / 0.
- Runs alongside the playing engine. That's why playing engines default to `Threads = available_parallelism() − 2`.
- Game over: the bar is fixed to the result (1-0 → full white, draw → 50%), and the evaluator stops.
- Evaluator binary missing or broken → bar hidden plus a one-time warning. The game is unaffected.
- No Rust crate is used. The pure-Rust options are hobby engines with weak, uncalibrated evals, and Stockfish is already the expected dependency. Any preset can be chosen as evaluator in the dialog.

### Tauri commands

`list_presets`, `detect_engine(preset) -> path?`, `probe_engine(config) -> {name, author, options[]}`, `get_config`, `set_config(config)`, `set_side_boards(bool)`, `new_game(start_fen?)`, `set_mode(mode)`, `next`, `move_now`, `abort`, `save_pgn(path)`, `save_fen(path)`.

### Events

- `position` `{ ply, fen, last_move, san_history[], side_to_move, check }`
- `engine-info` `{ side, name, depth, seldepth, nodes, nps, hashfull, tbhits, time_ms, lines: [{ multipv, score, wdl?, san[], end_fen, first_move }] }`
- `eval` `{ ply, depth, score, wdl?, white_pct }` (frontend drops it if `ply` ≠ current)
- `game-over` `{ result, reason }`
- `engine-error` `{ side | "evaluator", message }`

## Presets (`presets.rs`)

Option names are checked against each engine's declared options at handshake; anything not declared is skipped with a warning. `MultiPV` is not part of any preset: the arena sets it from the side-board toggle (3 or 1), and the evaluator always uses 1.

| Engine | Binary | Args | Options |
|---|---|---|---|
| Stockfish | `stockfish` | — | `Threads`, `Hash=256`, `UCI_ShowWDL=true` |
| Lc0 | `lc0` | — | `WeightsFile=<path>` (required, browse), `Backend` (blank = auto), `Threads`, `UCI_ShowWDL=true`. Default limit movetime; `nodes` = playouts; `depth` not meaningful. |
| GNU Chess | `gnuchess` | `--uci` | `Hash`; weak, single-thread |
| Berserk | `berserk` | — | `Threads`, `Hash=256` |
| Ethereal | `Ethereal` | — | `Threads`, `Hash=256` |
| Obsidian | `obsidian` | — | `Threads`, `Hash=256` |
| Caissa | `caissa` | — | `Threads`, `Hash=256` |
| Koivisto | `Koivisto` | — | `Threads`, `Hash=256` |
| Custom | user path | user args | user key/value |

PATH lookup tries case variants (`Ethereal`/`ethereal`).

## `config.rs`

```
Config {
  white: EngineConfig, black: EngineConfig,
  evaluator: EngineConfig,            // default: Stockfish, Threads=1, Hash=16, Depth(14)
  side_boards: bool,                  // default true
}
EngineConfig { preset, path, args[], options: [(name, value)], limit: Movetime(ms) | Depth(n) | Nodes(n) }
```

Saved as JSON in Tauri's `app_config_dir`; loaded at startup.

## Frontend

### Layout (CSS grid)

Side boards on (5 columns):

```
┌────────┬──┬──────────────────────────┬────────┐
│ PV1    │  │ top engine: PV1-3 + stats│ PV1    │
│ board  │  ├──────────────────────────┤ board  │
├────────┤E │                          ├────────┤
│ PV2    │V │       MAIN BOARD         │ PV2    │
│ board  │A │                          │ board  │
├────────┤L │                          ├────────┤
│ PV3    │  ├──────────────────────────┤ PV3    │
│ board  │  │ bottom engine: PV1-3+stat│ board  │
│        │  ├──────────────────────────┤        │
│        │  │ toolbar                  │        │
└────────┴──┴──────────────────────────┴────────┘
 left = top color                  right = bottom color
```

Side boards off: side columns removed (`display: none`, chessground instances kept but not updated), eval bar + main column centered, panels show PV1 only.

- Main board sized from viewport height; side boards `min(main/2, column height/3)`.
- All 7 boards `viewOnly: true`; side boards follow main orientation.
- Side board: position at end of its PV, PV's first move drawn as an arrow, caption `PV n · +0.35 · d22`.
- **Eval bar**: same height as the main board, ~24 px wide. Bottom-color fill grows from the bottom (flips with board), animated `height` transition, score label (`+0.35`, `M5`) at the leading end, evaluator depth in tooltip.
- PV text panel: engine name, color, stats row (`depth/seldepth`, `nodes`, `nps`, `hashfull`, `tbhits`, `time`, `WDL`), then SAN lines with move numbers, score in front, wrapping.
- Move list (SAN, compact) under the bottom panel.

### Toolbar

New Game (optional FEN input), Engines…, Mode toggle (Play-to-end / Turn-by-turn), Next (enabled only in `AwaitingNext`), Move now, Flip, **Side boards** toggle, Save PGN (enabled only on game over), Save FEN (always). Result banner on game over.

### Engine config modal

Native `<dialog>` with three sections: **White**, **Black**, **Evaluator**. Each has:
- preset dropdown → auto-filled path (Browse… via `plugin-dialog`);
- args, Lc0 weights file, Threads, Hash;
- limit type + value;
- extra options as key/value rows.

**Test** runs `probe_engine` and shows `id name`, author, and declared options, which catches wrong paths and option names before the game. The same engine may be used for any or all sections (separate processes).

### Files

`src/main.ts` (wiring, events), `src/boards.ts` (chessground instances, flip, side-board toggle), `src/evalbar.ts`, `src/panels.ts` (PV/stats rendering), `src/engine-dialog.ts`, `src/style.css`, `index.html`.

## Milestones

1. **License**: replace `LICENSE` with GPL-3.0 text; `license = "GPL-3.0-or-later"` in `Cargo.toml` and `package.json`; README note.
2. **Scaffold**: `create-tauri-app` vanilla-ts template; add chessground + CSS; render a static main board.
3. **UCI layer**: `engine.rs` on `vampirc-uci`, handshake, `set_multipv`. Tests feed captured Stockfish/Lc0/GNU Chess output through `parse_one` → `InfoUpdate` (including `wdl`, `lowerbound`, `mate`). Integration test against `stockfish` on PATH (skipped if absent).
4. **Game model**: `game.rs` on shakmaty. Tests for mate, stalemate, threefold, 50-move, insufficient material, illegal engine move, custom-FEN PGN tags.
5. **Arena**: state machine, both modes, Next/Move now, restart-search path (mode switch, MultiPV toggle), search-id staleness, throttled events, forfeits.
6. **Evaluator + eval bar**: `evaluator.rs`, win% mapping (unit-tested: cp 0 → 50, mate → 100/0, WDL path), `evalbar.ts`.
7. **Main UI**: layout, 7 boards, panels, toolbar, flip, side-board toggle.
8. **Engine dialog + presets + config persistence.**
9. **Save PGN/FEN** via save dialog.
10. **Verification**:
    - Stockfish vs Stockfish play-to-end to completion;
    - Stockfish vs GNU Chess turn-by-turn;
    - Lc0 if installed;
    - toggle side boards mid-search in both modes (engine log shows `MultiPV` changing only while idle);
    - eval bar tracks a won position and flips with the board;
    - start from a custom FEN;
    - open a saved PGN in another tool (`pgn-extract` or lichess import).

## Risks

- **R1 — `vampirc-uci` coverage/maintenance.** It's a pest grammar from the pre-`wdl` era. Need to verify with captured lines that `info … wdl 50 900 50 …` parses, either as `Any` or by dropping the attribute, without failing the whole line. Fallback: strip and read the ` wdl a b c` tokens before `parse_one`. Also confirm Lc0's `info string` and GNU Chess output parse cleanly. Milestone 3 tests cover this.
- **R2 — MultiPV=3 weakens play** (engine splits effort across 3 lines). Turning off side boards restores full strength (`MultiPV=1`).
- **R3 — GNU Chess** may not declare `MultiPV`; the declared-options check handles it (only PV1 populated).
- **R4 — Lc0** requires a network file and a working backend (CUDA/OpenCL/BLAS); `probe_engine` shows failures in the dialog.
- **R5 — Info flood**: Stockfish emits many `info` lines/sec × 3 PVs; Rust-side throttling keeps IPC bounded.
- **R6 — CPU contention**: the evaluator shares cores with the playing engine. It's limited to 1 thread / shallow depth, and the playing engines default to `cores − 2`.
- **R7 — chessground license**: GPL-3.0-or-later forces GPL for the whole app (milestone 1).

## Out of scope

Time controls, pondering, opening books, tablebase config UI, PGN import, multi-game matches/tournaments, engine downloading, Windows/macOS.
