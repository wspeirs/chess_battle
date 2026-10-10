mod arena;
mod config;
mod engine;
mod evaluator;
mod game;
mod presets;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            use tauri::Manager;
            let arena = arena::Arena::new(app.handle().clone());
            app.manage(arena);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            arena::list_presets,
            arena::detect_engine,
            arena::probe_engine,
            arena::get_config,
            arena::set_config,
            arena::set_side_boards,
            arena::new_game,
            arena::set_mode,
            arena::next,
            arena::move_now,
            arena::abort,
            arena::save_pgn,
            arena::save_fen,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
