use std::error::Error;

use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::config::{Settings, MAX_PINS};
use crate::{panel, settings_window, AppState};

pub const TRAY_ID: &str = "main";

const PIN_PREFIX: &str = "pin_open_";

fn snapshot<R: Runtime>(app: &AppHandle<R>) -> Settings {
    app.try_state::<AppState>()
        .map(|s| s.settings.lock().unwrap().clone())
        .unwrap_or_default()
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> Result<Menu<R>, Box<dyn Error>> {
    let toggle = MenuItemBuilder::with_id("toggle", "Показать / скрыть").build(app)?;
    let new_chat = MenuItemBuilder::with_id("new_chat", "Новый чат").build(app)?;

    let mut menu = MenuBuilder::new(app).item(&toggle).item(&new_chat);

    // Закреплённые чаты идут отдельным блоком, сразу под «Новым чатом»:
    // ради быстрого переключения между ними всё и делалось.
    if !settings.pins.is_empty() {
        menu = menu.separator();
        for (i, pin) in settings.pins.iter().enumerate() {
            let item = MenuItemBuilder::with_id(format!("{PIN_PREFIX}{i}"), &pin.title).build(app)?;
            menu = menu.item(&item);
        }
    }

    let full = settings.pins.len() >= MAX_PINS;
    let pin_current = MenuItemBuilder::with_id(
        "pin_current",
        if full {
            format!("Закрепить чат (занято {MAX_PINS} из {MAX_PINS})")
        } else {
            format!(
                "Закрепить этот чат ({} из {MAX_PINS})",
                settings.pins.len()
            )
        },
    )
    .enabled(!full)
    .build(app)?;

    let stay = CheckMenuItemBuilder::with_id("stay", "Не прятать при потере фокуса")
        .checked(settings.pinned)
        .build(app)?;
    let reload = MenuItemBuilder::with_id("reload", "Перезагрузить").build(app)?;
    let prefs = MenuItemBuilder::with_id("settings", "Настройки…").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Выход").build(app)?;

    Ok(menu
        .item(&pin_current)
        .separator()
        .item(&stay)
        .item(&reload)
        .item(&prefs)
        .separator()
        .item(&quit)
        .build()?)
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn Error>> {
    let settings = snapshot(app);
    let menu = build_menu(app, &settings)?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("в бандле нет иконки по умолчанию — нечего показать в трее")?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(format!("Gemini — {}", pretty_hotkey(&settings.hotkey)))
        .menu(&menu)
        // Левый клик должен открывать панель, а не меню.
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                panel::toggle(tray.app_handle(), Some(rect));
            }
        })
        .build(app)?;

    Ok(())
}

/// Пересобирает меню после изменения настроек: состав закладок и подписи
/// пунктов зависят от них, а меню строится один раз при создании иконки.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let settings = snapshot(app);
    match build_menu(app, &settings) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
            let _ = tray.set_tooltip(Some(format!("Gemini — {}", pretty_hotkey(&settings.hotkey))));
        }
        Err(e) => eprintln!("не удалось пересобрать меню трея: {e}"),
    }
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();

    if let Some(index) = id.strip_prefix(PIN_PREFIX).and_then(|i| i.parse::<usize>().ok()) {
        panel::open_pin(app, index);
        return;
    }

    match id {
        "toggle" => panel::toggle(app, None),
        "new_chat" => {
            panel::new_chat(app);
            panel::show(app, None);
        }
        "pin_current" => match panel::pin_current(app) {
            Ok(_) => {
                refresh(app);
                panel::notify_tabs(app);
            }
            Err(e) => eprintln!("не удалось закрепить чат: {e}"),
        },
        // Галочку переключает сама ОС — нам остаётся прочитать её состояние.
        "stay" => {
            let checked = app
                .try_state::<AppState>()
                .map(|s| !s.settings.lock().unwrap().pinned)
                .unwrap_or(false);
            panel::set_pinned(app, checked);
            refresh(app);
        }
        "reload" => panel::reload(app),
        "settings" => settings_window::open(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

/// «Ctrl+Shift+KeyG» — синтаксис парсера, а не то, что стоит показывать в трее.
pub fn pretty_hotkey(hotkey: &str) -> String {
    hotkey
        .split('+')
        .map(|token| {
            let t = token.trim();
            match t.to_ascii_lowercase().as_str() {
                "cmdorctrl" | "commandorcontrol" | "commandorctrl" | "cmdorcontrol" => {
                    "Ctrl".to_string()
                }
                "control" => "Ctrl".to_string(),
                "super" | "cmd" | "command" => "Win".to_string(),
                "option" => "Alt".to_string(),
                _ => t
                    .strip_prefix("Key")
                    .or_else(|| t.strip_prefix("Digit"))
                    .or_else(|| t.strip_prefix("Arrow"))
                    .unwrap_or(t)
                    .to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}
