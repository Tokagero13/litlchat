use tauri::{AppHandle, LogicalSize, Manager, State};

use crate::config::{Anchor, Settings, MAX_PINS, MAX_HEIGHT, MAX_WIDTH, MIN_HEIGHT, MIN_WIDTH};
use crate::{panel, settings_window, tray, AppState};

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

/// Адрес, открытый в панели прямо сейчас — им заполняется новая закладка.
#[tauri::command]
pub fn get_current_url(app: AppHandle) -> Option<String> {
    panel::current_url(&app)
}

#[tauri::command]
pub fn max_pins() -> usize {
    MAX_PINS
}

/// Возвращает предупреждение, если всё сохранилось, но горячую клавишу
/// назначить не удалось.
#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<Option<String>, String> {
    let mut settings = settings;
    settings.sanitize();

    // Неудача с горячей клавишей не отменяет сохранение остального: комбинацию
    // может держать другое приложение, и это не повод запирать пользователя
    // без возможности поменять размер панели или список закладок. Занятость
    // временна — настройки переживут перезапуск и применятся, когда клавиша
    // освободится.
    let hotkey_warning = crate::apply_hotkey(&app, &settings.hotkey).err();

    settings
        .save(&app)
        .map_err(|e| format!("не удалось записать settings.json: {e}"))?;

    if let Some(state) = app.try_state::<AppState>() {
        *state.settings.lock().unwrap() = settings.clone();
    }

    panel::apply_settings(&app, &settings);
    tray::refresh(&app);
    panel::notify_tabs(&app);
    Ok(hotkey_warning)
}

/// Кладёт текущий чат в первый свободный слот. Возвращает обновлённые настройки,
/// чтобы окну не пришлось перечитывать их отдельным вызовом.
#[tauri::command]
pub fn pin_current(app: AppHandle) -> Result<Settings, String> {
    let settings = panel::pin_current(&app)?;
    tray::refresh(&app);
    panel::notify_tabs(&app);
    Ok(settings)
}

/// Состояние полосы вкладок одним запросом — ей больше ничего знать не нужно.
#[derive(serde::Serialize)]
pub struct Tabs {
    pins: Vec<crate::config::Pin>,
    active: Option<usize>,
    max: usize,
}

#[tauri::command]
pub fn get_tabs(state: State<'_, AppState>) -> Tabs {
    Tabs {
        pins: state.settings.lock().unwrap().pins.clone(),
        active: *state.active_pin.lock().unwrap(),
        max: MAX_PINS,
    }
}

/// `None` — вкладка «Новый чат», иначе индекс закладки.
#[tauri::command]
pub fn select_tab(app: AppHandle, index: Option<usize>) {
    match index {
        Some(i) => panel::open_pin(&app, i),
        None => panel::new_chat(&app),
    }
}

#[tauri::command]
pub fn unpin(app: AppHandle, index: usize) -> Result<Settings, String> {
    let settings = panel::unpin(&app, index)?;
    tray::refresh(&app);
    Ok(settings)
}

#[tauri::command]
pub fn hide_panel(app: AppHandle) {
    panel::hide(&app);
}

/// Команда обязана быть асинхронной. Синхронные команды Tauri выполняет
/// на главном потоке, а создание вебвью на Windows ждёт, пока главный цикл
/// прокрутит сообщения — из главного же потока он их не крутит, и окно
/// остаётся пустым и висит. Асинхронная команда уходит в рабочий поток,
/// создание уезжает в главный, цикл продолжает работать.
/// Из меню трея тот же код работает: там обработчик вызывается уже из цикла.
#[tauri::command]
pub async fn open_settings(app: AppHandle) {
    settings_window::open(&app);
}

#[tauri::command]
pub fn rename_pin(app: AppHandle, index: usize, title: String) -> Result<(), String> {
    panel::rename_pin(&app, index, &title)?;
    tray::refresh(&app);
    Ok(())
}

/// Показывает панель с ещё не сохранёнными размером и углом — чтобы правки
/// в настройках было видно сразу, а не после нажатия «Сохранить».
#[tauri::command]
pub fn preview_panel(app: AppHandle, anchor: Anchor, width: f64, height: f64) {
    let size = LogicalSize::new(
        width.clamp(MIN_WIDTH, MAX_WIDTH),
        height.clamp(MIN_HEIGHT, MAX_HEIGHT),
    );
    panel::preview(&app, anchor, size);
}
