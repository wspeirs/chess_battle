use app_lib::arena::*;
use app_lib::config::*;
use app_lib::engine::*;
use app_lib::evaluator::*;
use app_lib::game::*;
use parking_lot::Mutex;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, Instant};

fn find_engine(name: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let fallback = PathBuf::from("/tmp/sf/usr/games").join(name);
    if fallback.is_file() {
        return Some(fallback);
    }
    None
}

struct TestEmitter {
    events: Arc<Mutex<Vec<(String, Value)>>>,
}

impl ArenaEmitter for TestEmitter {
    fn emit_event(&self, event: &str, payload: Value) {
        self.events.lock().push((event.to_string(), payload));
    }
}

// Verification (1): Stockfish vs Stockfish play-to-end to game-over + save_pgn validation
#[tokio::test]
async fn verify_1_stockfish_vs_stockfish_play_to_end() {
    let sf = match find_engine("stockfish") {
        Some(p) => p.to_string_lossy().to_string(),
        None => return,
    };

    let events = Arc::new(Mutex::new(Vec::new()));
    let emitter = Arc::new(TestEmitter { events: events.clone() });
    let arena = Arena::with_emitter(emitter);

    let mut cfg = arena.core.lock().await.config.clone();
    cfg.white = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Movetime(50),
    };
    cfg.black = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Movetime(50),
    };
    cfg.evaluator = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Depth(8),
    };
    cfg.side_boards = true;

    arena.core.lock().await.config = cfg.clone();

    let fen = "7k/Q7/6K1/8/8/8/8/8 w - - 0 1".to_string();

    {
        let mut guard = arena.core.lock().await;
        guard.game = Game::new(Some(&fen)).unwrap();
        guard.state = ArenaState::Idle;
        let mut w_eng = Engine::spawn(&cfg.white.path, &cfg.white.args).unwrap();
        w_eng.handshake(&cfg.white.options).await.unwrap();
        w_eng.ucinewgame().await.unwrap();
        w_eng.isready().await.unwrap();
        w_eng.set_multipv(3).await.unwrap();
        guard.white_name = "Stockfish White".to_string();

        let mut b_eng = Engine::spawn(&cfg.black.path, &cfg.black.args).unwrap();
        b_eng.handshake(&cfg.black.options).await.unwrap();
        b_eng.ucinewgame().await.unwrap();
        b_eng.isready().await.unwrap();
        b_eng.set_multipv(3).await.unwrap();
        guard.black_name = "Stockfish Black".to_string();

        let w_driver = EngineDriver::spawn(w_eng, shakmaty::Color::White, guard.driver_event_tx.clone());
        let b_driver = EngineDriver::spawn(b_eng, shakmaty::Color::Black, guard.driver_event_tx.clone());
        guard.white_driver = Some(w_driver);
        guard.black_driver = Some(b_driver);
        guard.emit_position();
        guard.start_search_for_current_turn(None).await;
    }

    let start = Instant::now();
    let mut got_game_over = false;
    while start.elapsed() < Duration::from_secs(5) {
        {
            let evs = events.lock();
            for (ev_name, _) in evs.iter() {
                if ev_name == "game-over" {
                    got_game_over = true;
                    break;
                }
            }
        }
        if got_game_over {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    assert!(got_game_over, "Game over event was not received within 5s!");

    let guard = arena.core.lock().await;
    let pgn = guard.game.to_pgn(&guard.white_name, &guard.black_name);

    assert!(pgn.contains("[SetUp \"1\"]"), "PGN must contain SetUp tag for custom FEN");
    assert!(pgn.contains("[FEN \"7k/Q7/6K1/8/8/8/8/8 w - - 0 1\"]"), "PGN must contain custom FEN");
    assert!(pgn.contains("[Termination \"Checkmate\"]"), "PGN must contain Termination tag");
    assert!(pgn.contains("[Result \"1-0\"]"), "PGN must contain Result tag");
    assert!(pgn.contains("1-0"), "PGN movetext must conclude with result");
}

// Verification (2): Stockfish vs GNU Chess turn-by-turn + Next advances + multiPV check
#[tokio::test]
async fn verify_2_stockfish_vs_gnuchess_turn_by_turn() {
    let sf = match find_engine("stockfish") {
        Some(p) => p.to_string_lossy().to_string(),
        None => return,
    };
    let gc = match find_engine("gnuchess") {
        Some(p) => p.to_string_lossy().to_string(),
        None => return,
    };

    let events = Arc::new(Mutex::new(Vec::new()));
    let emitter = Arc::new(TestEmitter { events: events.clone() });
    let arena = Arena::with_emitter(emitter);

    let mut cfg = arena.core.lock().await.config.clone();
    cfg.white = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Movetime(100),
    };
    cfg.black = EngineConfig {
        preset: "gnuchess".to_string(),
        path: gc.clone(),
        args: vec!["--uci".to_string()],
        options: vec![],
        limit: LimitCfg::Movetime(100),
    };
    cfg.side_boards = true;

    {
        let mut guard = arena.core.lock().await;
        guard.config = cfg.clone();
        guard.mode = PlayMode::TurnByTurn;
        guard.game = Game::new(None).unwrap();
        guard.state = ArenaState::Idle;

        let mut w_eng = Engine::spawn(&cfg.white.path, &cfg.white.args).unwrap();
        w_eng.handshake(&cfg.white.options).await.unwrap();
        w_eng.ucinewgame().await.unwrap();
        w_eng.isready().await.unwrap();
        w_eng.set_multipv(3).await.unwrap();
        guard.white_name = "Stockfish White".to_string();

        let mut b_eng = Engine::spawn(&cfg.black.path, &cfg.black.args).unwrap();
        b_eng.handshake(&cfg.black.options).await.unwrap();
        b_eng.ucinewgame().await.unwrap();
        b_eng.isready().await.unwrap();
        b_eng.set_multipv(3).await.unwrap();
        guard.black_name = "GNU Chess Black".to_string();

        let w_driver = EngineDriver::spawn(w_eng, shakmaty::Color::White, guard.driver_event_tx.clone());
        let b_driver = EngineDriver::spawn(b_eng, shakmaty::Color::Black, guard.driver_event_tx.clone());
        guard.white_driver = Some(w_driver);
        guard.black_driver = Some(b_driver);
        guard.emit_position();
        guard.start_search_for_current_turn(None).await;
    }

    // Wait for Stockfish to emit engine-info with 3 multipv lines
    let start = Instant::now();
    let mut sf_multipv_count = 0;
    while start.elapsed() < Duration::from_secs(3) {
        {
            let evs = events.lock();
            for (ev_name, payload) in evs.iter() {
                if ev_name == "engine-info" && payload["side"] == "white" {
                    if let Some(lines) = payload["lines"].as_array() {
                        if lines.len() > sf_multipv_count {
                            sf_multipv_count = lines.len();
                        }
                    }
                }
            }
        }
        if sf_multipv_count >= 3 {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(sf_multipv_count, 3, "Stockfish should emit 3 MultiPV lines with side_boards=true");

    // Advance White's turn via real next
    arena.next().await.expect("next should succeed for White in AwaitingNext");

    // Wait until position ply becomes 1 (Black to move)
    let start = Instant::now();
    let mut ply_advanced = false;
    while start.elapsed() < Duration::from_secs(3) {
        {
            let guard = arena.core.lock().await;
            if guard.game.played_moves().len() == 1 && guard.game.side_to_move() == shakmaty::Color::Black {
                ply_advanced = true;
                break;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(ply_advanced, "Next should advance White move to ply 1");

    // Wait a brief moment for Black (GNU Chess) to search
    sleep(Duration::from_millis(500)).await;

    // Advance Black's turn via real next
    arena.next().await.expect("next should succeed for Black in AwaitingNext");

    // Wait until position ply becomes 2 (White to move)
    let start = Instant::now();
    let mut ply_advanced_2 = false;
    while start.elapsed() < Duration::from_secs(3) {
        {
            let guard = arena.core.lock().await;
            if guard.game.played_moves().len() == 2 && guard.game.side_to_move() == shakmaty::Color::White {
                ply_advanced_2 = true;
                break;
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(ply_advanced_2, "Next should advance Black move to ply 2");
}

// Verification (3): toggling set_side_boards mid-search in both modes
#[tokio::test]
async fn verify_3_toggle_side_boards_mid_search() {
    let sf = match find_engine("stockfish") {
        Some(p) => p.to_string_lossy().to_string(),
        None => return,
    };

    let events = Arc::new(Mutex::new(Vec::new()));
    let emitter = Arc::new(TestEmitter { events: events.clone() });
    let arena = Arena::with_emitter(emitter);

    let mut cfg = arena.core.lock().await.config.clone();
    cfg.white = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Movetime(5000), // long search
    };
    cfg.black = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Movetime(5000),
    };
    cfg.side_boards = true;

    // Test in PlayToEnd mode
    {
        let mut guard = arena.core.lock().await;
        guard.config = cfg.clone();
        guard.mode = PlayMode::PlayToEnd;
        guard.game = Game::new(None).unwrap();
        guard.state = ArenaState::Idle;

        let mut w_eng = Engine::spawn(&cfg.white.path, &cfg.white.args).unwrap();
        w_eng.handshake(&cfg.white.options).await.unwrap();
        w_eng.ucinewgame().await.unwrap();
        w_eng.isready().await.unwrap();
        w_eng.set_multipv(3).await.unwrap();

        let w_driver = EngineDriver::spawn(w_eng, shakmaty::Color::White, guard.driver_event_tx.clone());
        guard.white_driver = Some(w_driver);
        guard.start_search_for_current_turn(None).await;
    }

    sleep(Duration::from_millis(200)).await;

    // Mid-search toggle side_boards: false
    {
        let mut guard = arena.core.lock().await;
        guard.config.side_boards = false;
        guard.restart_current_search().await;
    }

    // Wait for search to restart and emit engine-info with lines=1
    let start = Instant::now();
    let mut got_mpv_1 = false;
    while start.elapsed() < Duration::from_secs(3) {
        {
            let evs = events.lock();
            for (ev_name, payload) in evs.iter().rev() {
                if ev_name == "engine-info" && payload["side"] == "white" {
                    if let Some(lines) = payload["lines"].as_array() {
                        if lines.len() == 1 {
                            got_mpv_1 = true;
                            break;
                        }
                    }
                }
            }
        }
        if got_mpv_1 {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(got_mpv_1, "Restart search with side_boards=false should produce 1 PV line");

    // Test in TurnByTurn mode
    {
        let mut guard = arena.core.lock().await;
        guard.mode = PlayMode::TurnByTurn;
        guard.config.side_boards = true;
        guard.restart_current_search().await;
    }

    let start = Instant::now();
    let mut got_mpv_3 = false;
    while start.elapsed() < Duration::from_secs(3) {
        {
            let evs = events.lock();
            for (ev_name, payload) in evs.iter().rev() {
                if ev_name == "engine-info" && payload["side"] == "white" {
                    if let Some(lines) = payload["lines"].as_array() {
                        if lines.len() == 3 {
                            got_mpv_3 = true;
                            break;
                        }
                    }
                }
            }
        }
        if got_mpv_3 {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(got_mpv_3, "Turn-by-turn restart search with side_boards=true should produce 3 PV lines");
}

// Verification (4): evaluator eval events arrive with sensible white_pct for won position (>80) and game-over fixes the bar
#[tokio::test]
async fn verify_4_evaluator_won_position_and_game_over() {
    let sf = match find_engine("stockfish") {
        Some(p) => p.to_string_lossy().to_string(),
        None => return,
    };

    let events = Arc::new(Mutex::new(Vec::new()));
    let emitter = Arc::new(TestEmitter { events: events.clone() });

    let eval_cfg = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Depth(12),
    };

    let eval = Evaluator::start(&eval_cfg, emitter.clone()).await.unwrap();

    // White up a queen: 7k/Q7/6K1/8/8/8/8/8 w - - 0 1
    eval.set_position(1, "7k/Q7/6K1/8/8/8/8/8 w - - 0 1");

    let start = Instant::now();
    let mut max_white_pct = 0.0;
    while start.elapsed() < Duration::from_secs(4) {
        {
            let evs = events.lock();
            for (ev_name, payload) in evs.iter() {
                if ev_name == "eval" {
                    if let Some(pct) = payload["white_pct"].as_f64() {
                        if pct > max_white_pct {
                            max_white_pct = pct;
                        }
                    }
                }
            }
        }
        if max_white_pct > 80.0 {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    assert!(max_white_pct > 80.0, "Evaluator win% for white up a queen should be > 80%, got {}", max_white_pct);

    // Call finish(100.0) as on game-over
    eval.finish(100.0);
    sleep(Duration::from_millis(100)).await;

    let evs = events.lock();
    let last_eval = evs.iter().rev().find(|(name, _)| name == "eval").unwrap();
    assert_eq!(last_eval.1["white_pct"].as_f64().unwrap(), 100.0, "Game over should fix the bar to 100.0");
}

// Verification (5): mode switch mid-game works
#[tokio::test]
async fn verify_5_mode_switch_mid_game() {
    let sf = match find_engine("stockfish") {
        Some(p) => p.to_string_lossy().to_string(),
        None => return,
    };

    let events = Arc::new(Mutex::new(Vec::new()));
    let emitter = Arc::new(TestEmitter { events: events.clone() });
    let arena = Arena::with_emitter(emitter);

    let mut cfg = arena.core.lock().await.config.clone();
    cfg.white = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Movetime(5000),
    };
    cfg.black = EngineConfig {
        preset: "stockfish".to_string(),
        path: sf.clone(),
        args: vec![],
        options: vec![],
        limit: LimitCfg::Movetime(5000),
    };
    cfg.side_boards = true;

    {
        let mut guard = arena.core.lock().await;
        guard.config = cfg.clone();
        guard.mode = PlayMode::TurnByTurn;
        guard.game = Game::new(None).unwrap();
        guard.state = ArenaState::Idle;

        let mut w_eng = Engine::spawn(&cfg.white.path, &cfg.white.args).unwrap();
        w_eng.handshake(&cfg.white.options).await.unwrap();
        w_eng.ucinewgame().await.unwrap();
        w_eng.isready().await.unwrap();

        let w_driver = EngineDriver::spawn(w_eng, shakmaty::Color::White, guard.driver_event_tx.clone());
        guard.white_driver = Some(w_driver);
        guard.start_search_for_current_turn(None).await;
    }

    sleep(Duration::from_millis(200)).await;

    // Switch mode from TurnByTurn to PlayToEnd mid-game
    {
        let mut guard = arena.core.lock().await;
        guard.mode = PlayMode::PlayToEnd;
        guard.restart_current_search().await;
    }

    sleep(Duration::from_millis(200)).await;

    let guard = arena.core.lock().await;
    assert_eq!(guard.mode, PlayMode::PlayToEnd);
    assert!(matches!(guard.state, ArenaState::Thinking { .. }));
}
