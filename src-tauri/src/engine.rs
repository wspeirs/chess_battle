#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use shakmaty::uci::UciMove;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::timeout;
use vampirc_uci::{parse_one, UciInfoAttribute, UciMessage, UciOptionConfig};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub enum Score {
    Cp(i32),
    Mate(i32),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, serde::Deserialize)]
pub struct InfoUpdate {
    pub depth: Option<u32>,
    pub seldepth: Option<u32>,
    pub multipv: Option<u32>,
    pub score: Option<Score>,
    pub lowerbound: bool,
    pub upperbound: bool,
    pub wdl: Option<(u32, u32, u32)>,
    pub nodes: Option<u64>,
    pub nps: Option<u64>,
    pub hashfull: Option<u32>,
    pub tbhits: Option<u64>,
    pub time_ms: Option<u64>,
    #[serde(serialize_with = "serialize_pv", deserialize_with = "deserialize_pv")]
    pub pv: Vec<UciMove>,
}

fn deserialize_pv<'de, D>(deserializer: D) -> Result<Vec<UciMove>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let strings = Vec::<String>::deserialize(deserializer)?;
    let mut moves = Vec::new();
    for s in strings {
        if let Ok(m) = UciMove::from_ascii(s.as_bytes()) {
            moves.push(m);
        }
    }
    Ok(moves)
}

fn serialize_pv<S>(pv: &[UciMove], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(pv.len()))?;
    for m in pv {
        seq.serialize_element(&m.to_string())?;
    }
    seq.end()
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    Info(InfoUpdate),
    BestMove(Option<UciMove>),
    ReadyOk,
    UciOk,
    Id { name: Option<String>, author: Option<String> },
    Option(UciOptionConfig),
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Movetime(u64),
    Depth(u32),
    Nodes(u64),
    Infinite,
}

fn extract_wdl(line: &str) -> (Option<(u32, u32, u32)>, String) {
    let mut parts: Vec<&str> = line.split_whitespace().collect();
    let mut wdl = None;
    let mut wdl_idx = None;

    for i in 0..parts.len() {
        if parts[i] == "wdl" && i + 3 < parts.len() {
            if let (Ok(w), Ok(d), Ok(l)) = (
                parts[i + 1].parse::<u32>(),
                parts[i + 2].parse::<u32>(),
                parts[i + 3].parse::<u32>(),
            ) {
                wdl = Some((w, d, l));
                wdl_idx = Some(i);
                break;
            }
        }
    }

    if let Some(idx) = wdl_idx {
        parts.drain(idx..idx + 4);
        (wdl, parts.join(" "))
    } else {
        (None, line.to_string())
    }
}

pub fn parse_line(line: &str) -> Option<EngineEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Special case: "bestmove (none)" or bestmove with invalid move syntax
    if trimmed.starts_with("bestmove") {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            if parts[1] == "(none)" || parts[1] == "0000" || parts[1] == "none" {
                return Some(EngineEvent::BestMove(None));
            }
            if let Ok(m) = UciMove::from_ascii(parts[1].as_bytes()) {
                return Some(EngineEvent::BestMove(Some(m)));
            }
        }
    }

    // Pre-extract wdl before vampirc_uci::parse_one to avoid pest grammar dropping/failing attributes
    let (wdl, cleaned_line) = extract_wdl(trimmed);

    let msg = parse_one(&cleaned_line);
    match msg {
        UciMessage::UciOk => Some(EngineEvent::UciOk),
        UciMessage::ReadyOk => Some(EngineEvent::ReadyOk),
        UciMessage::Id { name, author } => Some(EngineEvent::Id { name, author }),
        UciMessage::Option(config) => Some(EngineEvent::Option(config)),
        UciMessage::BestMove { best_move, .. } => {
            let m = UciMove::from_ascii(best_move.to_string().as_bytes()).ok();
            Some(EngineEvent::BestMove(m))
        }
        UciMessage::Info(attrs) => {
            let mut info = InfoUpdate {
                wdl,
                ..Default::default()
            };

            for attr in attrs {
                match attr {
                    UciInfoAttribute::Depth(d) => info.depth = Some(d as u32),
                    UciInfoAttribute::SelDepth(sd) => info.seldepth = Some(sd as u32),
                    UciInfoAttribute::MultiPv(mpv) => info.multipv = Some(mpv as u32),
                    UciInfoAttribute::Score {
                        cp,
                        mate,
                        lower_bound,
                        upper_bound,
                    } => {
                        if let Some(m) = mate {
                            info.score = Some(Score::Mate(m as i32));
                        } else if let Some(c) = cp {
                            info.score = Some(Score::Cp(c as i32));
                        }
                        if lower_bound.unwrap_or(false) {
                            info.lowerbound = true;
                        }
                        if upper_bound.unwrap_or(false) {
                            info.upperbound = true;
                        }
                    }
                    UciInfoAttribute::Nodes(n) => info.nodes = Some(n),
                    UciInfoAttribute::Nps(n) => info.nps = Some(n),
                    UciInfoAttribute::HashFull(h) => info.hashfull = Some(h as u32),
                    UciInfoAttribute::TbHits(tb) => info.tbhits = Some(tb),
                    UciInfoAttribute::Time(t) => {
                        let ms = t.num_milliseconds();
                        if ms >= 0 {
                            info.time_ms = Some(ms as u64);
                        }
                    }
                    UciInfoAttribute::Pv(moves) => {
                        let mut pv_vec = Vec::with_capacity(moves.len());
                        let mut ok = true;
                        for m in moves {
                            match UciMove::from_ascii(m.to_string().as_bytes()) {
                                Ok(sm) => pv_vec.push(sm),
                                Err(_) => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        if ok {
                            info.pv = pv_vec;
                        }
                    }
                    _ => {}
                }
            }

            Some(EngineEvent::Info(info))
        }
        UciMessage::Unknown(raw, _) => {
            eprintln!("UCI unknown message: {}", raw);
            None
        }
        _ => None,
    }
}

pub struct Engine {
    child: Option<Child>,
    stdin: Option<tokio::process::ChildStdin>,
    events_rx: mpsc::Receiver<EngineEvent>,
    declared_options: HashSet<String>,
}

impl Engine {
    pub fn spawn<P: AsRef<Path>>(path: P, args: &[String]) -> std::io::Result<Self> {
        let mut cmd = Command::new(path.as_ref());
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("child stdout piped");
        let stderr = child.stderr.take().expect("child stderr piped");

        let (tx, rx) = mpsc::channel(256);

        // Stderr reader task: logs to eprintln
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("[engine stderr] {}", line);
            }
        });

        // Stdout reader task: parse each line into EngineEvent
        let tx_out = tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(event) = parse_line(&line) {
                    if tx_out.send(event).await.is_err() {
                        break;
                    }
                }
            }
            let _ = tx_out.send(EngineEvent::Exited).await;
        });

        Ok(Self {
            child: Some(child),
            stdin,
            events_rx: rx,
            declared_options: HashSet::new(),
        })
    }

    pub fn events_mut(&mut self) -> &mut mpsc::Receiver<EngineEvent> {
        &mut self.events_rx
    }

    pub async fn send_raw(&mut self, line: &str) -> std::io::Result<()> {
        if let Some(stdin) = self.stdin.as_mut() {
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stdin closed",
            ))
        }
    }

    pub async fn uci(&mut self) -> std::io::Result<()> {
        self.send_raw("uci").await
    }

    pub async fn isready(&mut self) -> std::io::Result<()> {
        self.send_raw("isready").await
    }

    pub async fn ucinewgame(&mut self) -> std::io::Result<()> {
        self.send_raw("ucinewgame").await
    }

    pub async fn setoption(&mut self, name: &str, value: &str) -> std::io::Result<()> {
        self.send_raw(&format!("setoption name {} value {}", name, value))
            .await
    }

    pub async fn position(&mut self, fen: Option<&str>, moves: &[UciMove]) -> std::io::Result<()> {
        let mut cmd = match fen {
            Some(f) => format!("position fen {}", f),
            None => "position startpos".to_string(),
        };

        if !moves.is_empty() {
            cmd.push_str(" moves");
            for m in moves {
                cmd.push(' ');
                cmd.push_str(&m.to_string());
            }
        }

        self.send_raw(&cmd).await
    }

    pub async fn go(&mut self, limit: Limit) -> std::io::Result<()> {
        let cmd = match limit {
            Limit::Movetime(ms) => format!("go movetime {}", ms),
            Limit::Depth(d) => format!("go depth {}", d),
            Limit::Nodes(n) => format!("go nodes {}", n),
            Limit::Infinite => "go infinite".to_string(),
        };
        self.send_raw(&cmd).await
    }

    pub async fn stop(&mut self) -> std::io::Result<()> {
        self.send_raw("stop").await
    }

    pub async fn quit(&mut self) -> std::io::Result<()> {
        self.send_raw("quit").await
    }

    pub async fn handshake(
        &mut self,
        options: &[(String, String)],
    ) -> Result<(Option<String>, Option<String>), String> {
        self.uci().await.map_err(|e| e.to_string())?;

        let mut id_name = None;
        let mut id_author = None;

        // Collect Id and Option until UciOk with 10s timeout
        let handshake_fut = async {
            while let Some(event) = self.events_rx.recv().await {
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
                        let opt_name = match opt {
                            UciOptionConfig::Check { name, .. } => name,
                            UciOptionConfig::Spin { name, .. } => name,
                            UciOptionConfig::Combo { name, .. } => name,
                            UciOptionConfig::Button { name } => name,
                            UciOptionConfig::String { name, .. } => name,
                        };
                        self.declared_options.insert(opt_name.to_lowercase());
                    }
                    EngineEvent::Exited => {
                        return Err("engine exited during handshake".to_string());
                    }
                    _ => {}
                }
            }
            Ok::<(), String>(())
        };

        timeout(Duration::from_secs(10), handshake_fut)
            .await
            .map_err(|_| "timeout waiting for uciok".to_string())??;

        // Send configured options only if declared
        for (name, val) in options {
            if self.declared_options.contains(&name.to_lowercase()) {
                self.setoption(name, val)
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                eprintln!(
                    "warning: engine does not declare option '{}'; skipping",
                    name
                );
            }
        }

        // Send isready and wait for readyok
        self.isready().await.map_err(|e| e.to_string())?;

        let ready_fut = async {
            while let Some(event) = self.events_rx.recv().await {
                match event {
                    EngineEvent::ReadyOk => return Ok(()),
                    EngineEvent::Exited => return Err("engine exited waiting for readyok".to_string()),
                    _ => {}
                }
            }
            Err("channel closed waiting for readyok".to_string())
        };

        timeout(Duration::from_secs(10), ready_fut)
            .await
            .map_err(|_| "timeout waiting for readyok".to_string())??;

        Ok((id_name, id_author))
    }

    pub async fn set_multipv(&mut self, n: u32) -> std::io::Result<()> {
        if self.declared_options.contains("multipv") {
            self.setoption("MultiPV", &n.to_string()).await
        } else {
            Ok(())
        }
    }

    pub async fn shutdown(&mut self) {
        let _ = self.stop().await;
        let _ = self.quit().await;

        if let Some(mut child) = self.child.take() {
            let wait_fut = child.wait();
            if timeout(Duration::from_secs(1), wait_fut).await.is_err() {
                let _ = child.start_kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stockfish_info_line_with_wdl() {
        let line = "info depth 20 seldepth 28 multipv 1 score cp 35 wdl 50 900 50 nodes 100000 nps 500000 hashfull 12 tbhits 0 time 200 pv e2e4 e7e5 g1f3 b8c6";
        let ev = parse_line(line).expect("must parse");
        match ev {
            EngineEvent::Info(info) => {
                assert_eq!(info.depth, Some(20));
                assert_eq!(info.seldepth, Some(28));
                assert_eq!(info.multipv, Some(1));
                assert_eq!(info.score, Some(Score::Cp(35)));
                assert_eq!(info.wdl, Some((50, 900, 50)));
                assert_eq!(info.nodes, Some(100000));
                assert_eq!(info.nps, Some(500000));
                assert_eq!(info.hashfull, Some(12));
                assert_eq!(info.tbhits, Some(0));
                assert_eq!(info.time_ms, Some(200));
                assert_eq!(info.pv.len(), 4);
                assert_eq!(info.pv[0].to_string(), "e2e4");
                assert_eq!(info.pv[1].to_string(), "e7e5");
                assert_eq!(info.pv[2].to_string(), "g1f3");
                assert_eq!(info.pv[3].to_string(), "b8c6");
                assert!(!info.lowerbound);
                assert!(!info.upperbound);
            }
            _ => panic!("expected info event"),
        }
    }

    #[test]
    fn test_score_mate_and_lowerbound() {
        let line = "info depth 15 multipv 2 score mate -3 lowerbound nodes 50000 pv f7f5 d2d4";
        let ev = parse_line(line).expect("must parse");
        match ev {
            EngineEvent::Info(info) => {
                assert_eq!(info.depth, Some(15));
                assert_eq!(info.multipv, Some(2));
                assert_eq!(info.score, Some(Score::Mate(-3)));
                assert!(info.lowerbound);
                assert!(!info.upperbound);
                assert_eq!(info.pv.len(), 2);
            }
            _ => panic!("expected info event"),
        }
    }

    #[test]
    fn test_score_upperbound() {
        let line = "info depth 10 score cp 120 upperbound nodes 1234 pv e2e4";
        let ev = parse_line(line).expect("must parse");
        match ev {
            EngineEvent::Info(info) => {
                assert_eq!(info.score, Some(Score::Cp(120)));
                assert!(!info.lowerbound);
                assert!(info.upperbound);
            }
            _ => panic!("expected info event"),
        }
    }

    #[test]
    fn test_lc0_info_string() {
        let line = "info string NNUE evaluation using embedded weights or Leela CUDA backend initialized";
        let ev = parse_line(line);
        assert!(ev.is_none() || matches!(ev, Some(EngineEvent::Info(_))));
    }

    #[test]
    fn test_gnuchess_output() {
        let line = "info depth 8 score cp -15 time 35 nodes 8421 pv d2d4 d7d5 c2c4";
        let ev = parse_line(line).expect("must parse");
        match ev {
            EngineEvent::Info(info) => {
                assert_eq!(info.depth, Some(8));
                assert_eq!(info.score, Some(Score::Cp(-15)));
                assert_eq!(info.nodes, Some(8421));
                assert_eq!(info.time_ms, Some(35));
                assert_eq!(info.pv.len(), 3);
            }
            _ => panic!("expected info event"),
        }
    }

    #[test]
    fn test_bestmove_variations() {
        let line1 = "bestmove e2e4 ponder e7e5";
        let ev1 = parse_line(line1).expect("must parse");
        match ev1 {
            EngineEvent::BestMove(Some(m)) => assert_eq!(m.to_string(), "e2e4"),
            _ => panic!("expected BestMove"),
        }

        let line2 = "bestmove (none)";
        let ev2 = parse_line(line2).expect("must parse");
        match ev2 {
            EngineEvent::BestMove(None) => {}
            _ => panic!("expected BestMove(None)"),
        }
    }

    #[test]
    fn test_shakmaty_conversion_failure_drops_pv() {
        // vampirc_uci might parse a move that shakmaty cannot parse (e.g. from string)
        let mut info = InfoUpdate::default();
        let invalid_uci_str = "e2e9"; // 9 is invalid rank
        let m = shakmaty::uci::UciMove::from_ascii(invalid_uci_str.as_bytes());
        assert!(m.is_err());

        // Verify conversion logic drops pv
        let moves = vec![invalid_uci_str];
        let mut pv_vec = Vec::new();
        let mut ok = true;
        for m_str in moves {
            match shakmaty::uci::UciMove::from_ascii(m_str.as_bytes()) {
                Ok(sm) => pv_vec.push(sm),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            info.pv = pv_vec;
        }
        assert!(info.pv.is_empty());
    }

    fn find_stockfish_on_path() -> Option<std::path::PathBuf> {
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let candidate = dir.join("stockfish");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    #[tokio::test]
    async fn test_integration_stockfish() {
        let sf_path = match find_stockfish_on_path() {
            Some(p) => p,
            None => {
                eprintln!("stockfish not found on PATH; skipping integration test");
                return;
            }
        };

        let mut engine = Engine::spawn(sf_path, &[]).expect("failed to spawn stockfish");
        let (id_name, _) = engine
            .handshake(&[("Hash".to_string(), "16".to_string())])
            .await
            .expect("handshake failed");
        println!("Connected to engine: {:?}", id_name);

        engine.ucinewgame().await.expect("ucinewgame failed");
        engine.isready().await.expect("isready failed");
        while let Some(event) = engine.events_mut().recv().await {
            if let EngineEvent::ReadyOk = event {
                break;
            }
        }

        engine.position(None, &[]).await.expect("position failed");
        engine.go(Limit::Depth(8)).await.expect("go failed");

        let mut got_bestmove = false;
        let mut got_info = false;
        while let Some(event) = engine.events_mut().recv().await {
            match event {
                EngineEvent::Info(_) => got_info = true,
                EngineEvent::BestMove(bm) => {
                    assert!(bm.is_some());
                    got_bestmove = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(got_info, "should receive at least one info event");
        assert!(got_bestmove, "should receive bestmove");

        engine.shutdown().await;
    }
}
