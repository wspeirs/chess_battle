use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", content = "value")]
pub enum LimitCfg {
    Movetime(u64),
    Depth(u32),
    Nodes(u64),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EngineConfig {
    pub preset: String,
    pub path: String,
    pub args: Vec<String>,
    pub options: Vec<(String, String)>,
    pub limit: LimitCfg,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub white: EngineConfig,
    pub black: EngineConfig,
    pub evaluator: EngineConfig,
    pub side_boards: bool,
}

impl Default for Config {
    fn default() -> Self {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).saturating_sub(2).max(1);
        let play = || EngineConfig {
            preset: "Stockfish".into(),
            path: crate::presets::detect("Stockfish").unwrap_or_else(|| "stockfish".into()),
            args: vec![],
            options: vec![
                ("Threads".into(), threads.to_string()),
                ("Hash".into(), "256".into()),
                ("UCI_ShowWDL".into(), "true".into()),
            ],
            limit: LimitCfg::Movetime(2000),
        };
        Config {
            white: play(),
            black: play(),
            evaluator: EngineConfig {
                preset: "Stockfish".into(),
                path: crate::presets::detect("Stockfish").unwrap_or_else(|| "stockfish".into()),
                args: vec![],
                options: vec![
                    ("Threads".into(), "1".into()),
                    ("Hash".into(), "16".into()),
                    ("UCI_ShowWDL".into(), "true".into()),
                ],
                limit: LimitCfg::Depth(14),
            },
            side_boards: true,
        }
    }
}

impl Config {
    /// Load `<dir>/config.json`; missing or invalid file yields defaults.
    pub fn load(dir: &Path) -> Config {
        std::fs::read(dir.join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("config.json"), json).map_err(|e| e.to_string())
    }
}
