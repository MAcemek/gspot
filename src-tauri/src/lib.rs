//! GSpot: a local-first Grammarly-style writing assistant for macOS.
//! Native Tauri host + Harper engine + Accessibility overlays + LLM features.

pub mod aiphrases;
pub mod apply;
#[cfg_attr(windows, path = "ax_win.rs")]
pub mod ax;
pub mod commands;
pub mod debug_server;
pub mod engine;
pub mod features;
pub mod history;
pub mod host;
pub mod llm;
pub mod overlay;
pub mod publik;
pub mod score;
pub mod settings;
pub mod spellrank;
pub mod textmap;
pub mod updates;
pub mod wordopts;

use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

pub fn run() {
    // Builder check (publik setup skill): PUBLIK_SELF_TEST=1 prints one line and exits.
    if std::env::var_os("PUBLIK_SELF_TEST").is_some() {
        println!("{}", publik_self_test());
        return;
    }
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).try_init();

    let mut state = host::HostState::default();
    state.settings = settings::load();
    state.history = history::load();
    llm::set_fallback_enabled(state.settings.ai_fallback_local);
    let shared: host::Shared = Arc::new(Mutex::new(state));
    let dock_side = shared.lock().unwrap().settings.dock_side.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(shared.clone())
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle().clone();
            overlay::create_windows(&handle, &dock_side)?;
            // Start tucked behind the edge tab, like Grammarly. First run (no
            // Accessibility yet) starts open so the permission gate is visible.
            overlay::set_tucked(&handle, &dock_side, ax::is_trusted(), false, false);

            let open = MenuItem::with_id(app, "open", "Open GSpot", true, None::<&str>)?;
            let perms = MenuItem::with_id(app, "perms", "Accessibility permission…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit GSpot", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &perms, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main").menu(&menu).show_menu_on_left_click(true).tooltip("GSpot");
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, event| match event.id.as_ref() {
                "open" => commands::open_panel_inner(app, "review", true),
                "perms" => {
                    ax::prompt_trust();
                }
                "quit" => app.exit(0),
                _ => {}
            })
            .build(app)?;

            host::spawn(handle.clone(), shared.clone());
            debug_server::spawn(handle.clone(), shared.clone());
            let start = shared.lock().unwrap().settings.start_with_windows;
            commands::sync_autostart(&handle, start);
            updates::spawn_auto_check(handle.clone());
            if !ax::is_trusted() {
                ax::prompt_trust();
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "panel" {
                    api.prevent_close();
                    commands::tuck_panel_inner(window.app_handle());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::accept_alert,
            commands::dismiss_alert,
            commands::replace_range,
            commands::run_feature,
            commands::chat_send,
            commands::get_settings,
            commands::set_settings,
            commands::get_history,
            commands::open_panel,
            commands::hide_panel,
            commands::tuck_panel,
            commands::permission_prompt,
            commands::permission_status,
            commands::set_card_rects,
            commands::open_url,
            commands::quit_app,
            commands::ui_ready,
            commands::ui_log,
            commands::publik_status,
            commands::publik_accept_disclosure,
            commands::publik_dismiss_first_run,
            commands::publik_forget,
            commands::publik_refresh,
            commands::probe_key,
            commands::check_update,
            commands::install_update,
            commands::app_version,
        ])
        .run(tauri::generate_context!())
        .expect("error while running GSpot");
}

/// publik self-test, run by the built app with PUBLIK_SELF_TEST set. A computer that already
/// holds a credential proves it with a wallet call; a new one provisions and stores a credential.
fn publik_self_test() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let ok = |install_id: &str| format!("publik self-test: ok {} {} (install {:.8})", publik::APP_SLUG, version, install_id);
    tauri::async_runtime::block_on(async {
        if let Some(c) = publik::read_credential() {
            return match publik::wallet(&c).await {
                Ok(_) => ok(&c.install_id),
                Err(e) => format!("publik self-test: failed: {e}"),
            };
        }
        match publik::provision(None).await {
            Ok(c) => match publik::write_credential(&c) {
                Ok(()) => ok(&c.install_id),
                Err(e) => format!("publik self-test: failed: {e}"),
            },
            Err(e) => format!("publik self-test: failed: {e}"),
        }
    })
}
