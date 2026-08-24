mod commands;
mod config;
mod panel;
mod settings_window;
mod tray;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{Manager, WindowEvent};

use config::Settings;

pub struct AppState {
    pub settings: Mutex<Settings>,
    /// До этого момента расфокус не прячет панель — см. `panel::BLUR_GUARD`.
    pub blur_guard_until: Mutex<Instant>,
    /// Когда панель в последний раз спряталась сама, потеряв фокус.
    pub auto_hidden_at: Mutex<Instant>,
    /// Комбинация, реально зарегистрированная в системе. Может отличаться от
    /// той, что в настройках: занятую другим приложением зарегистрировать нельзя.
    pub registered_hotkey: Mutex<Option<String>>,
    /// Панель поднята только ради предпросмотра в настройках — значит при их
    /// закрытии её надо убрать обратно.
    pub previewing: Mutex<bool>,
    /// Индекс закреплённого чата, открытого сейчас; None — вкладка «Новый чат».
    pub active_pin: Mutex<Option<usize>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::get_current_url,
            commands::pin_current,
            commands::max_pins,
            commands::preview_panel,
            commands::get_tabs,
            commands::select_tab,
            commands::unpin,
            commands::hide_panel,
            commands::open_settings,
            commands::rename_pin,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let (settings, first_run) = Settings::load(&handle);

            app.manage(AppState {
                settings: Mutex::new(settings.clone()),
                blur_guard_until: Mutex::new(Instant::now()),
                // Вычитание, а не Instant::now(): иначе первый же клик по трею
                // сразу после старта попал бы в окно RECENT_AUTO_HIDE и не открыл
                // панель. checked_sub — на случай запуска в первую минуту после
                // загрузки системы, где монотонные часы ещё близки к нулю.
                auto_hidden_at: Mutex::new(
                    Instant::now()
                        .checked_sub(Duration::from_secs(60))
                        .unwrap_or_else(Instant::now),
                ),
                registered_hotkey: Mutex::new(None),
                previewing: Mutex::new(false),
                active_pin: Mutex::new(None),
            });

            panel::build(&handle, &settings)?;
            init_hotkey_plugin(&handle);
            if let Err(e) = apply_hotkey(&handle, &settings.hotkey) {
                // Занятая комбинация — не фатально: трей продолжает работать,
                // а поменять её можно в окне настроек.
                eprintln!("{e}");
            }
            tray::build(&handle)?;

            // Первый запуск: показываем панель сразу. Иначе приложение выглядит
            // как не запустившееся, а войти в Google-аккаунт всё равно негде.
            if first_run {
                panel::show(&handle, None);
            }

            // Запасные входы для систем без системного трея (GNOME без
            // расширения AppIndicator, WSLg): иконки там не будет, а если ещё
            // и горячая клавиша не зарегистрировалась — приложение оказалось бы
            // запущенным, но полностью недоступным.
            let args: Vec<String> = std::env::args().collect();
            if args.iter().any(|a| a == "--settings") {
                settings_window::open(&handle);
            }
            if args.iter().any(|a| a == "--show") {
                panel::show(&handle, None);
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == settings_window::SETTINGS_LABEL {
                if matches!(event, WindowEvent::Destroyed) {
                    panel::end_preview(window.app_handle());
                }
                return;
            }
            if window.label() != panel::PANEL_LABEL {
                return;
            }
            match event {
                // Крестик и Alt+F4 прячут панель: приложение живёт в трее.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    panel::hide(window.app_handle());
                }
                WindowEvent::Focused(false) => panel::on_blur(window.app_handle()),
                // Полоса вкладок и чат — отдельные вебвью, сами за размером
                // окна они не следят.
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    panel::layout(window.app_handle())
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("не удалось собрать приложение Tauri")
        .run(|_app, event| {
            // Панель скрыта — это не повод завершаться. Выход только явный,
            // через пункт меню (он вызывает exit с кодом).
            if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

/// Плагин глобальных клавиш поднимается один раз за запуск; сами комбинации
/// потом регистрируются и снимаются через `apply_hotkey`.
fn init_hotkey_plugin(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::ShortcutState;

    let plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, event| {
            // Реагируем только на нажатие, иначе toggle срабатывает дважды.
            if event.state() == ShortcutState::Pressed {
                panel::toggle(app, None);
            }
        })
        .build();

    if let Err(e) = app.plugin(plugin) {
        eprintln!("плагин глобальных горячих клавиш не поднялся: {e}");
    }
}

/// Ставит новую комбинацию и снимает прежнюю. Старая снимается только после
/// успешной регистрации новой — иначе неудачная правка в настройках оставила бы
/// приложение вообще без горячей клавиши.
pub fn apply_hotkey(app: &tauri::AppHandle, hotkey: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let Some(state) = app.try_state::<AppState>() else {
        return Err("состояние приложения недоступно".into());
    };
    let previous = state.registered_hotkey.lock().unwrap().clone();
    if previous.as_deref() == Some(hotkey) {
        return Ok(());
    }

    app.global_shortcut().register(hotkey).map_err(|e| {
        format!(
            "не удалось назначить «{}»: {e}. Скорее всего комбинация занята другим приложением.",
            tray::pretty_hotkey(hotkey)
        )
    })?;

    if let Some(old) = previous {
        let _ = app.global_shortcut().unregister(old.as_str());
    }
    *state.registered_hotkey.lock().unwrap() = Some(hotkey.to_string());
    Ok(())
}
