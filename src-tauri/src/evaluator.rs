#![allow(dead_code)]

use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use serde::{Deserialize, Serialize};
use shakmaty::Color;
use tauri::Emitter;
use tokio::sync::mpsc;

use crate::config::{EngineConfig, LimitCfg};
use crate::engine::{Engine, EngineEvent, Limit, Score};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalScorePayload {
    #[serde(rename = "type")]
    pub score_type: String,
    pub value: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalPayload {
    pub ply: u32,
    pub depth: u32,
    pub score: EvalScorePayload,
    pub wdl: Option<Vec<u32>>,
    pub white_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineErrorPayload {
    pub side: String,
    pub message: String,
}

enum Command {
    SetPosition { ply: u32, fen: String },
    Finish { white_pct: f64 },
    Shutdown,
}

pub struct Evaluator {
    cmd_tx: std_mpsc::Sender<Command>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Pure win_pct function per plan:
/// - WDL present: (W + D / 2) / 1000 -> percent 0..100
/// - Otherwise: cp -> lichess win% curve: 50 + 50 * (2 / (1 + e^(-0.00368208 * cp)) - 1)
/// - Mate: 100 / 0 (adjusted for white perspective: mate > 0 -> 100, mate < 0 -> 0)
///
/// If white_to_move_adjusted is false, the score is from the perspective of the side to move (engine default).
/// We normalize score to White perspective first.
pub fn win_pct(score: &Score, wdl: Option<(u32, u32, u32)>, white_to_move: bool) -> f64 {
    if let Some((w, d, _l)) = wdl {
        // vampirc-uci / engine info wdl: typically from side-to-move perspective
        // w, d, l sum to 1000
        let w_pct = (w as f64 + (d as f64) / 2.0) / 10.0; // ((w + d/2) / 1000) * 100
        if white_to_move {
            w_pct
        } else {
            100.0 - w_pct
        }
    } else {
        match score {
            Score::Mate(m) => {
                let m_white = if white_to_move { *m } else { -*m };
                if m_white > 0 {
                    100.0
                } else if m_white < 0 {
                    0.0
                } else {
                    50.0
                }
            }
            Score::Cp(cp) => {
                let cp_white = if white_to_move { *cp } else { -*cp };
                let x = -0.00368208 * (cp_white as f64);
                let exp = x.exp();
                50.0 + 50.0 * (2.0 / (1.0 + exp) - 1.0)
            }
        }
    }
}

pub fn normalize_score(score: &Score, white_to_move: bool) -> EvalScorePayload {
    match score {
        Score::Cp(cp) => {
            let val = if white_to_move { *cp } else { -*cp };
            EvalScorePayload {
                score_type: "cp".to_string(),
                value: val,
            }
        }
        Score::Mate(m) => {
            let val = if white_to_move { *m } else { -*m };
            EvalScorePayload {
                score_type: "mate".to_string(),
                value: val,
            }
        }
    }
}

impl Evaluator {
    pub async fn start(cfg: &EngineConfig, app: Arc<dyn crate::arena::ArenaEmitter>) -> Result<Evaluator, String> {
        let mut engine = Engine::spawn(&cfg.path, &cfg.args).map_err(|e| e.to_string())?;

        engine
            .handshake(&cfg.options)
            .await
            .map_err(|e| e.to_string())?;

        let _ = engine.set_multipv(1).await;

        let limit = match cfg.limit {
            LimitCfg::Movetime(ms) => Limit::Movetime(ms),
            LimitCfg::Depth(d) => Limit::Depth(d),
            LimitCfg::Nodes(n) => Limit::Nodes(n),
        };

        let (cmd_tx, cmd_rx) = std_mpsc::channel::<Command>();

        let join_handle = tokio::spawn(async move {
            run_evaluator_loop(engine, limit, cmd_rx, app).await;
        });

        Ok(Evaluator {
            cmd_tx,
            join_handle: Some(join_handle),
        })
    }

    pub fn set_position(&self, ply: u32, fen: &str) {
        let _ = self.cmd_tx.send(Command::SetPosition {
            ply,
            fen: fen.to_string(),
        });
    }

    pub fn finish(&self, white_pct: f64) {
        let _ = self.cmd_tx.send(Command::Finish { white_pct });
    }

    pub async fn shutdown(mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
    }
}
async fn run_evaluator_loop(
    mut engine: Engine,
    limit: Limit,
    cmd_rx: std_mpsc::Receiver<Command>,
    app: Arc<dyn crate::arena::ArenaEmitter>,
) {
    let (async_tx, mut async_rx) = mpsc::channel::<Command>(32);

    // Forward blocking std_mpsc commands to async_rx in a dedicated blocking thread
    std::thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv() {
            let is_shutdown = matches!(cmd, Command::Shutdown);
            if async_tx.blocking_send(cmd).is_err() {
                break;
            }
            if is_shutdown {
                break;
            }
        }
    });

    let mut current_ply: u32 = 0;
    let mut white_to_move: bool = true;
    let mut last_depth: u32 = 0;
    let mut searching = false;
    let mut finished = false;

    loop {
        tokio::select! {
            cmd_opt = async_rx.recv() => {
                let cmd = match cmd_opt {
                    Some(c) => c,
                    None => break,
                };
                match cmd {
                    Command::SetPosition { ply, fen } => {
                        finished = false;
                        if searching {
                            let _ = engine.stop().await;
                            // Drain remaining events until BestMove or timeout
                            drain_bestmove(engine.events_mut()).await;
                        }

                        current_ply = ply;
                        last_depth = 0;

                        // Parse FEN to determine side to move
                        white_to_move = match fen.parse::<shakmaty::fen::Fen>() {
                            Ok(f) => {
                                let setup: shakmaty::Setup = f.into();
                                setup.turn == Color::White
                            }
                            Err(_) => true,
                        };

                        if let Err(e) = engine.position(Some(&fen), &[]).await {
                            let _ = app.emit_event("engine-error", serde_json::to_value(&EngineErrorPayload {
                                side: "evaluator".to_string(),
                                message: e.to_string(),
                            }).unwrap_or_default());
                            break;
                        }

                        if let Err(e) = engine.go(limit).await {
                            let _ = app.emit_event("engine-error", serde_json::to_value(&EngineErrorPayload {
                                side: "evaluator".to_string(),
                                message: e.to_string(),
                            }).unwrap_or_default());
                            break;
                        }
                        searching = true;
                    }
                    Command::Finish { white_pct } => {
                        if searching {
                            let _ = engine.stop().await;
                            drain_bestmove(engine.events_mut()).await;
                            searching = false;
                        }
                        finished = true;
                        let payload = EvalPayload {
                            ply: 0,
                            depth: 0,
                            score: EvalScorePayload {
                                score_type: "cp".to_string(),
                                value: 0,
                            },
                            wdl: None,
                            white_pct,
                        };
                        let _ = app.emit_event("eval", serde_json::to_value(&payload).unwrap_or_default());
                    }
                    Command::Shutdown => {
                        break;
                    }
                }
            }
            event_opt = engine.events_mut().recv() => {
                match event_opt {
                    Some(EngineEvent::Info(info)) => {
                        if !finished {
                            if let (Some(depth), Some(score)) = (info.depth, &info.score) {
                                if depth > last_depth {
                                    last_depth = depth;
                                    let pct = win_pct(score, info.wdl, white_to_move);
                                    let norm_score = normalize_score(score, white_to_move);
                                    let wdl_vec = info.wdl.map(|(w, d, l)| {
                                        if white_to_move {
                                            vec![w, d, l]
                                        } else {
                                            vec![l, d, w]
                                        }
                                    });
                                    let payload = EvalPayload {
                                        ply: current_ply,
                                        depth,
                                        score: norm_score,
                                        wdl: wdl_vec,
                                        white_pct: pct,
                                    };
                                    let _ = app.emit_event("eval", serde_json::to_value(&payload).unwrap_or_default());
                                }
                            }
                        }
                    }
                    Some(EngineEvent::BestMove(_)) => {
                        searching = false;
                    }
                    Some(EngineEvent::Exited) => {
                        let _ = app.emit_event("engine-error", serde_json::to_value(&EngineErrorPayload {
                            side: "evaluator".to_string(),
                            message: "evaluator engine process exited unexpectedly".to_string(),
                        }).unwrap_or_default());
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

    engine.shutdown().await;
}

async fn drain_bestmove(rx: &mut mpsc::Receiver<EngineEvent>) {
    use tokio::time::{timeout, Duration};
    let _ = timeout(Duration::from_millis(500), async {
        while let Some(event) = rx.recv().await {
            if matches!(event, EngineEvent::BestMove(_)) {
                break;
            }
        }
    }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_win_pct_cp_zero() {
        let pct = win_pct(&Score::Cp(0), None, true);
        assert!((pct - 50.0).abs() < 1e-6);

        let pct_black = win_pct(&Score::Cp(0), None, false);
        assert!((pct_black - 50.0).abs() < 1e-6);
    }

    #[test]
    fn test_win_pct_mate() {
        // Mate for white
        let pct_w = win_pct(&Score::Mate(3), None, true);
        assert_eq!(pct_w, 100.0);

        let pct_w_black_turn = win_pct(&Score::Mate(-3), None, false);
        assert_eq!(pct_w_black_turn, 100.0);

        // Mate against white
        let pct_b = win_pct(&Score::Mate(-2), None, true);
        assert_eq!(pct_b, 0.0);

        let pct_b_black_turn = win_pct(&Score::Mate(2), None, false);
        assert_eq!(pct_b_black_turn, 0.0);
    }

    #[test]
    fn test_win_pct_wdl() {
        // w=600, d=300, l=100 from white's perspective: (600 + 150) / 10 = 75.0%
        let pct_w = win_pct(&Score::Cp(150), Some((600, 300, 100)), true);
        assert!((pct_w - 75.0).abs() < 1e-6);

        // From black's perspective, w=600, d=300, l=100 means Black expected score is 75%, so White is 25%
        let pct_b = win_pct(&Score::Cp(150), Some((600, 300, 100)), false);
        assert!((pct_b - 25.0).abs() < 1e-6);
    }

    #[test]
    fn test_win_pct_symmetric_cp() {
        let pct_pos = win_pct(&Score::Cp(100), None, true);
        let pct_neg = win_pct(&Score::Cp(-100), None, true);
        assert!((pct_pos + pct_neg - 100.0).abs() < 1e-6);

        // Turn symmetry
        let pct_black_pos = win_pct(&Score::Cp(100), None, false);
        assert!((pct_black_pos - pct_neg).abs() < 1e-6);
    }
}
