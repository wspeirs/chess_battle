use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use shakmaty::{CastlingMode, Color, Position};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use vampirc_uci::UciOptionConfig;

use crate::config::{Config, EngineConfig, LimitCfg};
use crate::engine::{Engine, EngineEvent, InfoUpdate, Limit, Score};
use crate::evaluator::Evaluator;
use crate::game::{Game, GameEnd};
use crate::presets::{detect as detect_preset, list_presets as get_presets, Preset};

pub trait ArenaEmitter: Send + Sync + 'static {
    fn emit_event(&self, event: &str, payload: serde_json::Value);
}

impl ArenaEmitter for AppHandle {
    fn emit_event(&self, event: &str, payload: serde_json::Value) {
        let _ = self.emit(event, payload);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PositionPayload {
    pub ply: u32,
    pub fen: String,
    pub last_move: Option<[String; 2]>,
    pub san_history: Vec<String>,
    pub side_to_move: String,
    pub check: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PvLinePayload {
    pub multipv: u32,
    pub score: Option<Score>,
    pub wdl: Option<(u32, u32, u32)>,
    pub san: Vec<String>,
    pub end_fen: String,
    pub first_move: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineInfoPayload {
    pub side: String,
    pub name: String,
    pub depth: Option<u32>,
    pub seldepth: Option<u32>,
    pub nodes: Option<u64>,
    pub nps: Option<u64>,
    pub hashfull: Option<u32>,
    pub tbhits: Option<u64>,
    pub time_ms: Option<u64>,
    pub lines: Vec<PvLinePayload>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameOverPayload {
    pub result: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineErrorPayload {
    pub side: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeOption {
    pub name: String,
    pub type_: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeResult {
    pub name: Option<String>,
    pub author: Option<String>,
    pub options: Vec<ProbeOption>,
}

// --- Modes and States ---

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlayMode {
    PlayToEnd,
    TurnByTurn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArenaState {
    Idle,
    Thinking { side: Color, search_id: u64 },
    AwaitingNext { side: Color, search_id: u64 },
    Stopping,
    GameOver { result: String, reason: String },
}

// --- Throttling tracker: 100ms per side, keeps last output ---

#[derive(Default)]
pub struct ThrottleTracker {
    last_emit: Option<Instant>,
    latest_lines: HashMap<u32, PvLinePayload>,
    latest_depth: Option<u32>,
    latest_seldepth: Option<u32>,
    latest_nodes: Option<u64>,
    latest_nps: Option<u64>,
    latest_hashfull: Option<u32>,
    latest_tbhits: Option<u64>,
    latest_time_ms: Option<u64>,
}

impl ThrottleTracker {
    pub fn update(&mut self, info: &InfoUpdate, game: &Game) {
        if let Some(d) = info.depth {
            self.latest_depth = Some(d);
        }
        if let Some(sd) = info.seldepth {
            self.latest_seldepth = Some(sd);
        }
        if let Some(n) = info.nodes {
            self.latest_nodes = Some(n);
        }
        if let Some(nps) = info.nps {
            self.latest_nps = Some(nps);
        }
        if let Some(h) = info.hashfull {
            self.latest_hashfull = Some(h);
        }
        if let Some(tb) = info.tbhits {
            self.latest_tbhits = Some(tb);
        }
        if let Some(t) = info.time_ms {
            self.latest_time_ms = Some(t);
        }

        let multipv = info.multipv.unwrap_or(1);
        let (san, end_fen) = game.pv_to_san(&info.pv);
        let first_move = info.pv.first().map(|m| m.to_string());

        let line = PvLinePayload {
            multipv,
            score: info.score.clone(),
            wdl: info.wdl,
            san,
            end_fen,
            first_move,
        };
        self.latest_lines.insert(multipv, line);
    }

    pub fn should_emit(&self, now: Instant, interval: Duration) -> bool {
        match self.last_emit {
            None => true,
            Some(t) => now.duration_since(t) >= interval,
        }
    }

    pub fn build_payload(&mut self, side: Color, name: &str, now: Instant) -> EngineInfoPayload {
        self.last_emit = Some(now);
        let mut lines: Vec<PvLinePayload> = self.latest_lines.values().cloned().collect();
        lines.sort_by_key(|l| l.multipv);

        let side_str = match side {
            Color::White => "white",
            Color::Black => "black",
        };

        EngineInfoPayload {
            side: side_str.to_string(),
            name: name.to_string(),
            depth: self.latest_depth,
            seldepth: self.latest_seldepth,
            nodes: self.latest_nodes,
            nps: self.latest_nps,
            hashfull: self.latest_hashfull,
            tbhits: self.latest_tbhits,
            time_ms: self.latest_time_ms,
            lines,
        }
    }
}

// Commands from ArenaCore to the Engine Driver Actor
pub enum DriverCmd {
    Go {
        fen: Option<String>,
        moves: Vec<shakmaty::uci::UciMove>,
        limit: Limit,
        search_id: u64,
    },
    StopAndSetMultiPv {
        multipv: Option<u32>,
        search_id: u64,
    },
    Shutdown,
}

// Events from Driver Actor to Event Hub
pub enum DriverEvent {
    Info {
        side: Color,
        search_id: u64,
        info: InfoUpdate,
    },
    BestMove {
        side: Color,
        search_id: u64,
        mv: Option<shakmaty::uci::UciMove>,
    },
    Stopped {
        side: Color,
        search_id: u64,
    },
    Exited {
        side: Color,
        search_id: u64,
    },
}

pub struct EngineDriver {
    cmd_tx: mpsc::Sender<DriverCmd>,
}

impl EngineDriver {
    pub fn spawn(
        mut engine: Engine,
        side: Color,
        event_tx: mpsc::Sender<DriverEvent>,
    ) -> Self {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<DriverCmd>(32);

        tokio::spawn(async move {
            let mut current_search_id: u64 = 0;
            let mut is_searching = false;

            loop {
                tokio::select! {
                    cmd_opt = cmd_rx.recv() => {
                        match cmd_opt {
                            Some(DriverCmd::Go { fen, moves, limit, search_id }) => {
                                current_search_id = search_id;
                                is_searching = true;
                                if let Err(e) = engine.position(fen.as_deref(), &moves).await {
                                    eprintln!("[arena] position error: {}", e);
                                    let _ = event_tx.send(DriverEvent::Exited { side, search_id }).await;
                                    break;
                                }
                                if let Err(e) = engine.go(limit).await {
                                    eprintln!("[arena] go error: {}", e);
                                    let _ = event_tx.send(DriverEvent::Exited { side, search_id }).await;
                                    break;
                                }
                            }
                            Some(DriverCmd::StopAndSetMultiPv { multipv, search_id }) => {
                                if is_searching {
                                    let _ = engine.stop().await;
                                    // Drain until BestMove or Exited
                                    while let Some(ev) = engine.events_mut().recv().await {
                                        match ev {
                                            EngineEvent::BestMove(mv) => {
                                                if multipv.is_none() {
                                                    let _ = event_tx.send(DriverEvent::BestMove {
                                                        side,
                                                        search_id: current_search_id,
                                                        mv,
                                                    }).await;
                                                }
                                                break;
                                            }
                                            EngineEvent::Exited => {
                                                let _ = event_tx.send(DriverEvent::Exited {
                                                    side,
                                                    search_id: current_search_id,
                                                }).await;
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                    is_searching = false;
                                }
                                if let Some(n) = multipv {
                                    let _ = engine.set_multipv(n).await;
                                    let _ = event_tx.send(DriverEvent::Stopped { side, search_id }).await;
                                }
                            }
                            Some(DriverCmd::Shutdown) | None => {
                                engine.shutdown().await;
                                break;
                            }
                        }
                    }
                    ev_opt = engine.events_mut().recv() => {
                        match ev_opt {
                            Some(EngineEvent::Info(info)) => {
                                if is_searching {
                                    let _ = event_tx.send(DriverEvent::Info {
                                        side,
                                        search_id: current_search_id,
                                        info,
                                    }).await;
                                }
                            }
                            Some(EngineEvent::BestMove(mv)) => {
                                if is_searching {
                                    is_searching = false;
                                    let _ = event_tx.send(DriverEvent::BestMove {
                                        side,
                                        search_id: current_search_id,
                                        mv,
                                    }).await;
                                }
                            }
                            Some(EngineEvent::Exited) => {
                                let _ = event_tx.send(DriverEvent::Exited {
                                    side,
                                    search_id: current_search_id,
                                }).await;
                                break;
                            }
                            None => {
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        EngineDriver { cmd_tx }
    }

    pub async fn go(&self, fen: Option<String>, moves: Vec<shakmaty::uci::UciMove>, limit: Limit, search_id: u64) {
        let _ = self.cmd_tx.send(DriverCmd::Go { fen, moves, limit, search_id }).await;
    }

    pub async fn stop_and_set_multipv(&self, multipv: Option<u32>, search_id: u64) {
        let _ = self.cmd_tx.send(DriverCmd::StopAndSetMultiPv { multipv, search_id }).await;
    }

    pub async fn stop(&self, search_id: u64) {
        let _ = self.cmd_tx.send(DriverCmd::StopAndSetMultiPv { multipv: None, search_id }).await;
    }

    pub async fn set_multipv(&self, n: u32) {
        let _ = self.cmd_tx.send(DriverCmd::StopAndSetMultiPv { multipv: Some(n), search_id: 0 }).await;
    }

    pub async fn shutdown(&self) {
        let _ = self.cmd_tx.send(DriverCmd::Shutdown).await;
    }
}

// --- Pure decision & helper functions ---

pub fn decide_forfeit_or_legal_move(
    game: &mut Game,
    turn: Color,
    best_move: Option<shakmaty::uci::UciMove>,
) -> Result<shakmaty::Move, GameEnd> {
    match best_move {
        None => {
            if game.current_pos().legal_moves().is_empty() {
                if let Some(end) = game.status() {
                    Err(end)
                } else {
                    Err(game.forfeit(turn, "Engine resigned or returned no move"))
                }
            } else {
                Err(game.forfeit(turn, "Engine returned no move when legal moves exist"))
            }
        }
        Some(m) => {
            match game.play_uci(m) {
                Ok(pm) => Ok(pm.mv),
                Err(e) => Err(game.forfeit(turn, &format!("Illegal move: {}", e.0))),
            }
        }
    }
}

pub fn is_search_id_stale(msg_id: u64, current_id: u64) -> bool {
    msg_id != current_id
}

// --- Arena Core State ---

pub struct ArenaCore {
    pub state: ArenaState,
    pub mode: PlayMode,
    pub config: Config,
    pub config_dir: PathBuf,
    pub game: Game,
    pub white_driver: Option<EngineDriver>,
    pub black_driver: Option<EngineDriver>,
    pub white_name: String,
    pub black_name: String,
    pub evaluator: Option<Evaluator>,
    pub search_counter: Arc<AtomicU64>,
    pub white_throttle: ThrottleTracker,
    pub black_throttle: ThrottleTracker,
    pub app: Arc<dyn ArenaEmitter>,
    pub driver_event_tx: mpsc::Sender<DriverEvent>,
}

impl ArenaCore {
    pub fn emit_position(&self) {
        let pos = self.game.current_pos();
        let ply = self.game.played_moves().len() as u32;
        let fen = self.game.to_fen();
        let last_move = self
            .game
            .last_move_squares()
            .map(|(from, to)| [from.to_string(), to.to_string()]);
        let san_history = self.game.san_history();
        let side_to_move = match pos.turn() {
            Color::White => "white",
            Color::Black => "black",
        };
        let check = self.game.is_check();

        let payload = PositionPayload {
            ply,
            fen: fen.clone(),
            last_move,
            san_history,
            side_to_move: side_to_move.to_string(),
            check,
        };

        self.app.emit_event("position", serde_json::to_value(&payload).unwrap_or_default());

        if let Some(eval) = &self.evaluator {
            eval.set_position(ply, &fen);
        }
    }

    pub fn emit_game_over(&mut self, end: GameEnd) {
        self.state = ArenaState::GameOver {
            result: end.result.to_string(),
            reason: end.reason.clone(),
        };

        let white_pct = match end.result {
            "1-0" => 100.0,
            "0-1" => 0.0,
            _ => 50.0,
        };

        if let Some(eval) = &self.evaluator {
            eval.finish(white_pct);
        }

        self.app.emit_event(
            "game-over",
            serde_json::to_value(&GameOverPayload {
                result: end.result.to_string(),
                reason: end.reason,
            })
            .unwrap_or_default(),
        );
    }

    pub fn emit_engine_error(&self, side: &str, message: &str) {
        self.app.emit_event(
            "engine-error",
            serde_json::to_value(&EngineErrorPayload {
                side: side.to_string(),
                message: message.to_string(),
            })
            .unwrap_or_default(),
        );
    }

    pub async fn shutdown_engines(&mut self) {
        if let Some(w) = self.white_driver.take() {
            w.shutdown().await;
        }
        if let Some(b) = self.black_driver.take() {
            b.shutdown().await;
        }
        if let Some(eval) = self.evaluator.take() {
            eval.shutdown().await;
        }
    }

    pub fn start_search_id(&self) -> u64 {
        self.search_counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current_search_id(&self) -> Option<u64> {
        match self.state {
            ArenaState::Thinking { search_id, .. } => Some(search_id),
            ArenaState::AwaitingNext { search_id, .. } => Some(search_id),
            _ => None,
        }
    }

    pub async fn start_search_for_current_turn(&mut self, existing_search_id: Option<u64>) {
        if let Some(end) = self.game.status() {
            self.emit_game_over(end);
            return;
        }

        let turn = self.game.side_to_move();
        let search_id = existing_search_id.unwrap_or_else(|| self.start_search_id());

        let driver = match turn {
            Color::White => self.white_driver.as_ref(),
            Color::Black => self.black_driver.as_ref(),
        };

        let driver = match driver {
            Some(d) => d,
            None => {
                let side_name = match turn {
                    Color::White => "White",
                    Color::Black => "Black",
                };
                let end = self.game.forfeit(turn, &format!("{} engine process is missing", side_name));
                self.emit_game_over(end);
                return;
            }
        };

        let played_ucis: Vec<shakmaty::uci::UciMove> = self
            .game
            .played_moves()
            .iter()
            .filter_map(|pm| {
                shakmaty::uci::UciMove::from_ascii(pm.mv.to_uci(CastlingMode::Standard).to_string().as_bytes()).ok()
            })
            .collect();

        let (limit, new_state) = match self.mode {
            PlayMode::PlayToEnd => {
                let cfg_limit = match turn {
                    Color::White => &self.config.white.limit,
                    Color::Black => &self.config.black.limit,
                };
                let limit = match cfg_limit {
                    LimitCfg::Movetime(ms) => Limit::Movetime(*ms),
                    LimitCfg::Depth(d) => Limit::Depth(*d),
                    LimitCfg::Nodes(n) => Limit::Nodes(*n),
                };
                (limit, ArenaState::Thinking { side: turn, search_id })
            }
            PlayMode::TurnByTurn => (
                Limit::Infinite,
                ArenaState::AwaitingNext { side: turn, search_id },
            ),
        };
        let start_fen = self.game.start_fen();
        let fen_arg = if self.game.is_custom_fen() {
            Some(start_fen)
        } else {
            None
        };

        self.state = new_state;
        driver.go(fen_arg, played_ucis, limit, search_id).await;
    }

    pub async fn restart_current_search(&mut self) {
        let side = match self.state {
            ArenaState::Thinking { side, .. } => side,
            ArenaState::AwaitingNext { side, .. } => side,
            _ => return,
        };

        let mpv = if self.config.side_boards { 3 } else { 1 };
        let new_search_id = self.start_search_id();

        self.state = match self.mode {
            PlayMode::PlayToEnd => ArenaState::Thinking { side, search_id: new_search_id },
            PlayMode::TurnByTurn => ArenaState::AwaitingNext { side, search_id: new_search_id },
        };

        let driver = match side {
            Color::White => self.white_driver.as_ref(),
            Color::Black => self.black_driver.as_ref(),
        };

        if let Some(d) = driver {
            d.stop_and_set_multipv(Some(mpv), new_search_id).await;
        }
    }

    pub async fn handle_driver_event(&mut self, event: DriverEvent) {
        match event {
            DriverEvent::Info {
                side,
                search_id,
                info,
            } => {
                if let Some(curr_id) = self.current_search_id() {
                    if is_search_id_stale(search_id, curr_id) {
                        return;
                    }
                } else {
                    return;
                }

                let now = Instant::now();
                let interval = Duration::from_millis(100);

                let (throttle, name) = match side {
                    Color::White => (&mut self.white_throttle, self.white_name.clone()),
                    Color::Black => (&mut self.black_throttle, self.black_name.clone()),
                };

                throttle.update(&info, &self.game);

                if throttle.should_emit(now, interval) {
                    let payload = throttle.build_payload(side, &name, now);
                    self.app.emit_event("engine-info", serde_json::to_value(&payload).unwrap_or_default());
                }
            }
            DriverEvent::BestMove {
                side,
                search_id,
                mv,
            } => {
                let current_id = match self.state {
                    ArenaState::Thinking { search_id: id, side: s } if s == side => id,
                    ArenaState::AwaitingNext { search_id: id, side: s } if s == side => id,
                    _ => return,
                };

                if is_search_id_stale(search_id, current_id) {
                    return;
                }

                // Flush final engine-info update before advancing move
                let now = Instant::now();
                let (throttle, name) = match side {
                    Color::White => (&mut self.white_throttle, self.white_name.clone()),
                    Color::Black => (&mut self.black_throttle, self.black_name.clone()),
                };
                let payload = throttle.build_payload(side, &name, now);
                self.app.emit_event("engine-info", serde_json::to_value(&payload).unwrap_or_default());

                match decide_forfeit_or_legal_move(&mut self.game, side, mv) {
                    Ok(_played_mv) => {
                        self.emit_position();
                        if let Some(end) = self.game.status() {
                            self.emit_game_over(end);
                        } else {
                            self.start_search_for_current_turn(None).await;
                        }
                    }
                    Err(end) => {
                        self.emit_game_over(end);
                    }
                }
            }
            DriverEvent::Stopped { side: _, search_id } => {
                if let Some(curr_id) = self.current_search_id() {
                    if is_search_id_stale(search_id, curr_id) {
                        return;
                    }
                } else {
                    return;
                }
                self.start_search_for_current_turn(Some(search_id)).await;
            }
            DriverEvent::Exited { side, search_id } => {
                let current_id = match self.state {
                    ArenaState::Thinking { search_id: id, side: s } if s == side => id,
                    ArenaState::AwaitingNext { search_id: id, side: s } if s == side => id,
                    _ => return,
                };

                if is_search_id_stale(search_id, current_id) {
                    return;
                }

                let side_str = match side {
                    Color::White => "white",
                    Color::Black => "black",
                };
                self.emit_engine_error(side_str, "Engine process exited unexpectedly");
                let end = self.game.forfeit(side, "Engine crashed");
                self.emit_game_over(end);
            }
        }
    }
}

pub struct Arena {
    pub core: Arc<Mutex<ArenaCore>>,
}

impl Arena {
    pub fn new(app: AppHandle) -> Self {
        let config_dir = app
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        Self::with_emitter_and_config_dir(Arc::new(app), config_dir)
    }

    pub fn with_emitter(emitter: Arc<dyn ArenaEmitter>) -> Self {
        Self::with_emitter_and_config_dir(emitter, PathBuf::from("."))
    }

    pub fn with_emitter_and_config_dir(emitter: Arc<dyn ArenaEmitter>, config_dir: PathBuf) -> Self {
        let config = Config::load(&config_dir);

        let game = Game::new(None).expect("default game");

        let (driver_event_tx, mut driver_event_rx) = mpsc::channel::<DriverEvent>(128);

        let search_counter = Arc::new(AtomicU64::new(0));

        let core = Arc::new(Mutex::new(ArenaCore {
            state: ArenaState::Idle,
            mode: PlayMode::PlayToEnd,
            config,
            config_dir,
            game,
            white_driver: None,
            black_driver: None,
            white_name: "White".to_string(),
            black_name: "Black".to_string(),
            evaluator: None,
            search_counter,
            white_throttle: ThrottleTracker::default(),
            black_throttle: ThrottleTracker::default(),
            app: emitter,
            driver_event_tx,
        }));

        // Event hub loop dispatching engine driver events to ArenaCore
        let core_clone = Arc::clone(&core);
        tokio::spawn(async move {
            while let Some(ev) = driver_event_rx.recv().await {
                let mut guard = core_clone.lock().await;
                guard.handle_driver_event(ev).await;
            }
        });
        Self { core }
    }

    pub async fn next(&self) -> Result<(), String> {
        let guard = self.core.lock().await;
        match guard.state {
            ArenaState::AwaitingNext { side, .. } => {
                let driver = match side {
                    Color::White => guard.white_driver.as_ref(),
                    Color::Black => guard.black_driver.as_ref(),
                };
                let search_id = guard.current_search_id().unwrap_or(0);
                if let Some(d) = driver {
                    d.stop(search_id).await;
                }
                Ok(())
            }
            _ => Err("next is only allowed in AwaitingNext state".to_string()),
        }
    }

    pub async fn move_now(&self) -> Result<(), String> {
        let guard = self.core.lock().await;
        match guard.state {
            ArenaState::Thinking { side, .. } => {
                let driver = match side {
                    Color::White => guard.white_driver.as_ref(),
                    Color::Black => guard.black_driver.as_ref(),
                };
                let search_id = guard.current_search_id().unwrap_or(0);
                if let Some(d) = driver {
                    d.stop(search_id).await;
                }
                Ok(())
            }
            _ => Err("move_now is only allowed in Thinking state".to_string()),
        }
    }
}

// --- Tauri Commands ---

#[tauri::command]
pub fn list_presets() -> Vec<Preset> {
    get_presets()
}

#[tauri::command]
pub fn detect_engine(preset: String) -> Option<String> {
    detect_preset(&preset)
}

#[tauri::command]
pub async fn probe_engine(config: EngineConfig) -> Result<ProbeResult, String> {
    let mut engine = Engine::spawn(&config.path, &config.args).map_err(|e| e.to_string())?;

    let mut id_name = None;
    let mut id_author = None;
    let mut options = Vec::new();

    engine.uci().await.map_err(|e| e.to_string())?;

    let probe_fut = async {
        while let Some(event) = engine.events_mut().recv().await {
            match event {
                EngineEvent::UciOk => break,
                EngineEvent::Id { name, author } => {
                    if name.is_some() {
                        id_name = name;
                    }
                    if author.is_some() {
                        id_author = author;
                    }
                }
                EngineEvent::Option(opt) => {
                    let (name, type_) = match opt {
                        UciOptionConfig::Check { name, .. } => (name, "check"),
                        UciOptionConfig::Spin { name, .. } => (name, "spin"),
                        UciOptionConfig::Combo { name, .. } => (name, "combo"),
                        UciOptionConfig::Button { name } => (name, "button"),
                        UciOptionConfig::String { name, .. } => (name, "string"),
                    };
                    options.push(ProbeOption {
                        name,
                        type_: type_.to_string(),
                    });
                }
                EngineEvent::Exited => return Err("engine exited during probe".to_string()),
                _ => {}
            }
        }
        Ok::<(), String>(())
    };

    let _ = timeout(Duration::from_secs(5), probe_fut)
        .await
        .map_err(|_| "probe timed out waiting for uciok".to_string())?;

    engine.shutdown().await;

    Ok(ProbeResult {
        name: id_name,
        author: id_author,
        options,
    })
}

#[tauri::command]
pub async fn get_config(arena: State<'_, Arena>) -> Result<Config, String> {
    let guard = arena.core.lock().await;
    Ok(guard.config.clone())
}

#[tauri::command]
pub async fn set_config(config: Config, arena: State<'_, Arena>) -> Result<(), String> {
    let mut guard = arena.core.lock().await;
    guard.config = config;
    let config_dir = guard.config_dir.clone();
    guard.config.save(&config_dir)?;
    Ok(())
}

#[tauri::command]
pub async fn set_side_boards(side_boards: bool, arena: State<'_, Arena>) -> Result<(), String> {
    let mut guard = arena.core.lock().await;
    if guard.config.side_boards == side_boards {
        return Ok(());
    }
    guard.config.side_boards = side_boards;

    let config_dir = guard.config_dir.clone();
    let _ = guard.config.save(&config_dir);
    // If searching, restart current search to toggle MultiPV 3 <-> 1
    guard.restart_current_search().await;

    // The idle engine also gets the updated MultiPV
    let mpv = if side_boards { 3 } else { 1 };
    match guard.state {
        ArenaState::Thinking { side, .. } | ArenaState::AwaitingNext { side, .. } => {
            let other = match side {
                Color::White => Color::Black,
                Color::Black => Color::White,
            };
            if let Some(d) = match other {
                Color::White => guard.white_driver.as_ref(),
                Color::Black => guard.black_driver.as_ref(),
            } {
                d.set_multipv(mpv).await;
            }
        }
        _ => {
            if let Some(w) = guard.white_driver.as_ref() {
                w.set_multipv(mpv).await;
            }
            if let Some(b) = guard.black_driver.as_ref() {
                b.set_multipv(mpv).await;
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn new_game(start_fen: Option<String>, arena: State<'_, Arena>) -> Result<(), String> {
    let mut guard = arena.core.lock().await;

    // Abort existing game & shutdown old engines
    guard.shutdown_engines().await;

    // Reset throttle states
    guard.white_throttle = ThrottleTracker::default();
    guard.black_throttle = ThrottleTracker::default();

    // Create new Game model
    let new_game = Game::new(start_fen.as_deref())?;
    guard.game = new_game;
    guard.state = ArenaState::Idle;

    let mpv = if guard.config.side_boards { 3 } else { 1 };

    // Spawn and handshake white engine
    let mut white_engine = match Engine::spawn(&guard.config.white.path, &guard.config.white.args) {
        Ok(e) => e,
        Err(e) => {
            guard.emit_engine_error("white", &format!("Failed to spawn white engine: {}", e));
            return Err(e.to_string());
        }
    };
    let white_id = white_engine.handshake(&guard.config.white.options).await.map_err(|e| {
        guard.emit_engine_error("white", &format!("Handshake failed: {}", e));
        e
    })?;
    let _ = white_engine.ucinewgame().await;
    let _ = white_engine.isready().await;
    let _ = white_engine.set_multipv(mpv).await;

    guard.white_name = white_id.0.unwrap_or_else(|| "White".to_string());

    // Spawn and handshake black engine
    let mut black_engine = match Engine::spawn(&guard.config.black.path, &guard.config.black.args) {
        Ok(e) => e,
        Err(e) => {
            guard.emit_engine_error("black", &format!("Failed to spawn black engine: {}", e));
            return Err(e.to_string());
        }
    };
    let black_id = black_engine.handshake(&guard.config.black.options).await.map_err(|e| {
        guard.emit_engine_error("black", &format!("Handshake failed: {}", e));
        e
    })?;
    let _ = black_engine.ucinewgame().await;
    let _ = black_engine.isready().await;
    let _ = black_engine.set_multipv(mpv).await;

    guard.black_name = black_id.0.unwrap_or_else(|| "Black".to_string());

    // Spawn evaluator
    match Evaluator::start(&guard.config.evaluator, guard.app.clone()).await {
        Ok(eval) => {
            guard.evaluator = Some(eval);
        }
        Err(err) => {
            guard.emit_engine_error("evaluator", &format!("Evaluator start failed: {}", err));
            guard.evaluator = None;
        }
    }

    // Wrap engines in drivers
    let white_driver = EngineDriver::spawn(
        white_engine,
        Color::White,
        guard.driver_event_tx.clone(),
    );
    let black_driver = EngineDriver::spawn(
        black_engine,
        Color::Black,
        guard.driver_event_tx.clone(),
    );

    guard.white_driver = Some(white_driver);
    guard.black_driver = Some(black_driver);

    // Emit initial position
    guard.emit_position();

    // Start thinking for side to move
    guard.start_search_for_current_turn(None).await;

    Ok(())
}

#[tauri::command]
pub async fn set_mode(mode: String, arena: State<'_, Arena>) -> Result<(), String> {
    let mut guard = arena.core.lock().await;
    let new_mode = match mode.as_str() {
        "play-to-end" => PlayMode::PlayToEnd,
        "turn-by-turn" => PlayMode::TurnByTurn,
        other => return Err(format!("unknown mode: {}", other)),
    };

    if guard.mode == new_mode {
        return Ok(());
    }

    guard.mode = new_mode;
    // Restart current search to switch between go <limit> and go infinite
    guard.restart_current_search().await;

    Ok(())
}

#[tauri::command]
pub async fn next(arena: State<'_, Arena>) -> Result<(), String> {
    arena.next().await
}

#[tauri::command]
pub async fn move_now(arena: State<'_, Arena>) -> Result<(), String> {
    arena.move_now().await
}

#[tauri::command]
pub async fn abort(arena: State<'_, Arena>) -> Result<(), String> {
    let mut guard = arena.core.lock().await;
    guard.shutdown_engines().await;
    guard.state = ArenaState::Idle;
    Ok(())
}

#[tauri::command]
pub async fn save_pgn(path: String, arena: State<'_, Arena>) -> Result<(), String> {
    let guard = arena.core.lock().await;
    let pgn = guard.game.to_pgn(&guard.white_name, &guard.black_name);
    std::fs::write(&path, pgn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_fen(path: String, arena: State<'_, Arena>) -> Result<(), String> {
    let guard = arena.core.lock().await;
    let fen = guard.game.to_fen();
    std::fs::write(&path, fen).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::uci::UciMove as ShakmatyUciMove;
    use shakmaty::Square;

    #[test]
    fn test_search_id_staleness() {
        assert!(is_search_id_stale(1, 2));
        assert!(!is_search_id_stale(2, 2));
        assert!(is_search_id_stale(3, 2));
    }

    #[test]
    fn test_throttle_tracker_logic() {
        let mut tracker = ThrottleTracker::default();
        let game = Game::new(None).unwrap();
        let now = Instant::now();

        assert!(tracker.should_emit(now, Duration::from_millis(100)));

        let info = InfoUpdate {
            depth: Some(12),
            nodes: Some(4000),
            ..Default::default()
        };
        tracker.update(&info, &game);
        let _payload = tracker.build_payload(Color::White, "Stockfish", now);

        // Immediately after emit, should_emit is false for < 100ms
        assert!(!tracker.should_emit(now + Duration::from_millis(50), Duration::from_millis(100)));
        // After 100ms, should_emit is true
        assert!(tracker.should_emit(now + Duration::from_millis(101), Duration::from_millis(100)));
    }

    #[test]
    fn test_forfeit_on_illegal_bestmove() {
        let mut game = Game::new(None).unwrap();
        // Illegal move for starting position: e1e8
        let illegal_uci = ShakmatyUciMove::Normal {
            from: Square::E1,
            to: Square::E8,
            promotion: None,
        };

        let result = decide_forfeit_or_legal_move(&mut game, Color::White, Some(illegal_uci));
        assert!(result.is_err());
        let end = result.unwrap_err();
        assert_eq!(end.result, "0-1");
        assert!(end.reason.contains("Illegal move"));
    }

    #[test]
    fn test_legal_bestmove_played() {
        let mut game = Game::new(None).unwrap();
        // Legal move: e2e4
        let legal_uci = ShakmatyUciMove::Normal {
            from: Square::E2,
            to: Square::E4,
            promotion: None,
        };

        let result = decide_forfeit_or_legal_move(&mut game, Color::White, Some(legal_uci));
        assert!(result.is_ok());
        assert_eq!(game.played_moves().len(), 1);
        assert_eq!(game.side_to_move(), Color::Black);
    }

    #[test]
    fn test_forfeit_on_none_bestmove_when_legal_moves_exist() {
        let mut game = Game::new(None).unwrap();
        let result = decide_forfeit_or_legal_move(&mut game, Color::White, None);
        assert!(result.is_err());
        let end = result.unwrap_err();
        assert_eq!(end.result, "0-1");
        assert!(end.reason.contains("Engine returned no move"));
    }
}
