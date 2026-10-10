use crate::config::{EngineConfig, LimitCfg};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub binary: String,
    pub args: Vec<String>,
    pub options: Vec<(String, String)>,
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .saturating_sub(2)
        .max(1)
}

pub fn list_presets() -> Vec<Preset> {
    let threads = default_threads().to_string();
    vec![
        Preset {
            name: "Stockfish".into(),
            binary: "stockfish".into(),
            args: vec![],
            options: vec![
                ("Threads".into(), threads.clone()),
                ("Hash".into(), "256".into()),
                ("UCI_ShowWDL".into(), "true".into()),
            ],
        },
        Preset {
            name: "Lc0".into(),
            binary: "lc0".into(),
            args: vec![],
            options: vec![
                ("WeightsFile".into(), "".into()),
                ("Backend".into(), "".into()),
                ("Threads".into(), threads.clone()),
                ("UCI_ShowWDL".into(), "true".into()),
            ],
        },
        Preset {
            name: "GNU Chess".into(),
            binary: "gnuchess".into(),
            args: vec!["--uci".into()],
            options: vec![("Hash".into(), "256".into())],
        },
        Preset {
            name: "Berserk".into(),
            binary: "berserk".into(),
            args: vec![],
            options: vec![
                ("Threads".into(), threads.clone()),
                ("Hash".into(), "256".into()),
            ],
        },
        Preset {
            name: "Ethereal".into(),
            binary: "Ethereal".into(),
            args: vec![],
            options: vec![
                ("Threads".into(), threads.clone()),
                ("Hash".into(), "256".into()),
            ],
        },
        Preset {
            name: "Obsidian".into(),
            binary: "obsidian".into(),
            args: vec![],
            options: vec![
                ("Threads".into(), threads.clone()),
                ("Hash".into(), "256".into()),
            ],
        },
        Preset {
            name: "Caissa".into(),
            binary: "caissa".into(),
            args: vec![],
            options: vec![
                ("Threads".into(), threads.clone()),
                ("Hash".into(), "256".into()),
            ],
        },
        Preset {
            name: "Koivisto".into(),
            binary: "Koivisto".into(),
            args: vec![],
            options: vec![
                ("Threads".into(), threads),
                ("Hash".into(), "256".into()),
            ],
        },
        Preset {
            name: "Custom".into(),
            binary: "".into(),
            args: vec![],
            options: vec![],
        },
    ]
}

fn to_abs_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| {
        if p.is_absolute() {
            p.to_path_buf()
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(p)
        } else {
            p.to_path_buf()
        }
    })
}

pub fn detect(preset_name: &str) -> Option<String> {
    if preset_name.trim().is_empty() || preset_name.eq_ignore_ascii_case("custom") {
        return None;
    }

    if preset_name.contains('/') {
        let p = Path::new(preset_name);
        if p.is_file() {
            return Some(to_abs_path(p).to_string_lossy().into_owned());
        }
        return None;
    }

    let presets = list_presets();
    let target_binary = if let Some(p) = presets
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(preset_name) || p.binary.eq_ignore_ascii_case(preset_name))
    {
        if p.binary.is_empty() {
            return None;
        }
        p.binary.as_str()
    } else {
        preset_name
    };

    let path_var = std::env::var_os("PATH")?;

    let mut candidates = vec![
        target_binary.to_string(),
        target_binary.to_lowercase(),
        target_binary.to_uppercase(),
    ];
    let mut chars = target_binary.chars();
    if let Some(first) = chars.next() {
        let title = first.to_uppercase().collect::<String>() + chars.as_str().to_lowercase().as_str();
        if !candidates.contains(&title) {
            candidates.push(title);
        }
    }

    for dir in std::env::split_paths(&path_var) {
        for cand in &candidates {
            let p = dir.join(cand);
            if p.is_file() {
                return Some(to_abs_path(&p).to_string_lossy().into_owned());
            }
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_file() || ft.is_symlink() {
                        let name = entry.file_name();
                        if name.to_string_lossy().eq_ignore_ascii_case(target_binary) {
                            let p = entry.path();
                            if p.is_file() {
                                return Some(to_abs_path(&p).to_string_lossy().into_owned());
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

pub fn default_engine_config(preset_name: &str) -> EngineConfig {
    let presets = list_presets();
    let found = presets.into_iter().find(|p| p.name.eq_ignore_ascii_case(preset_name));

    let (preset_str, args, options) = match found {
        Some(p) => (p.name, p.args, p.options),
        None => (preset_name.to_string(), vec![], vec![]),
    };

    EngineConfig {
        preset: preset_str,
        path: detect(preset_name).unwrap_or_default(),
        args,
        options,
        limit: LimitCfg::Movetime(2000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_presets() {
        let presets = list_presets();
        assert_eq!(presets.len(), 9);

        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Stockfish",
                "Lc0",
                "GNU Chess",
                "Berserk",
                "Ethereal",
                "Obsidian",
                "Caissa",
                "Koivisto",
                "Custom",
            ]
        );

        let gnuchess = presets.iter().find(|p| p.name == "GNU Chess").unwrap();
        assert_eq!(gnuchess.args, vec!["--uci"]);
        assert!(gnuchess.options.iter().any(|(k, v)| k == "Hash" && v == "256"));

        let lc0 = presets.iter().find(|p| p.name == "Lc0").unwrap();
        assert!(lc0.options.iter().any(|(k, v)| k == "WeightsFile" && v.is_empty()));
        assert!(lc0.options.iter().any(|(k, v)| k == "UCI_ShowWDL" && v == "true"));

        let custom = presets.iter().find(|p| p.name == "Custom").unwrap();
        assert_eq!(custom.binary, "");
        assert!(custom.args.is_empty());
        assert!(custom.options.is_empty());
    }
    #[test]
    fn test_default_engine_config_shape() {
        let sf_cfg = default_engine_config("Stockfish");
        assert_eq!(sf_cfg.preset, "Stockfish");
        assert_eq!(sf_cfg.limit, LimitCfg::Movetime(2000));
        assert!(sf_cfg.options.iter().any(|(k, _)| k == "Threads"));
        assert!(sf_cfg.options.iter().any(|(k, v)| k == "Hash" && v == "256"));
        assert!(sf_cfg.options.iter().any(|(k, v)| k == "UCI_ShowWDL" && v == "true"));

        let gnu_cfg = default_engine_config("GNU Chess");
        assert_eq!(gnu_cfg.preset, "GNU Chess");
        assert_eq!(gnu_cfg.args, vec!["--uci"]);
        assert_eq!(gnu_cfg.limit, LimitCfg::Movetime(2000));

        let custom_cfg = default_engine_config("Custom");
        assert_eq!(custom_cfg.preset, "Custom");
        assert_eq!(custom_cfg.path, "");
        assert!(custom_cfg.args.is_empty());
        assert!(custom_cfg.options.is_empty());
        assert_eq!(custom_cfg.limit, LimitCfg::Movetime(2000));
    }

    #[test]
    fn test_detect_case_variant_with_temp_dir() {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = match LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let unique = format!(
            "chess_battle_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let temp_dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create lowercase "ethereal" file in temp_dir, while preset binary is "Ethereal"
        let bin_path = temp_dir.join("ethereal");
        std::fs::write(&bin_path, "#!/bin/sh\n").unwrap();

        let old_path = std::env::var_os("PATH");
        let new_path = match &old_path {
            Some(curr) => {
                let mut joined = temp_dir.clone().into_os_string();
                joined.push(":");
                joined.push(curr);
                joined
            }
            None => temp_dir.clone().into_os_string(),
        };

        // SAFETY: Test serializes access via mutex, restoring PATH immediately after.
        unsafe {
            std::env::set_var("PATH", &new_path);
        }

        let detected = detect("Ethereal");

        // Restore PATH before assertions
        unsafe {
            if let Some(old) = old_path {
                std::env::set_var("PATH", old);
            } else {
                std::env::remove_var("PATH");
            }
        }
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert!(detected.is_some(), "Expected to detect ethereal in PATH");
        let detected_path = detected.unwrap();
        assert!(
            detected_path.ends_with("ethereal"),
            "Detected path should end with ethereal: {}",
            detected_path
        );
    }
}
