use std::time::{Duration, Instant};

use tauri::webview::WebviewBuilder;
use tauri::window::WindowBuilder;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    Rect, Runtime, Url, Webview, WebviewUrl, Window,
};
use tauri_plugin_opener::OpenerExt;

use crate::config::{Anchor, Pin, Settings, MAX_PINS};
use crate::AppState;

pub const PANEL_LABEL: &str = "panel";
/// Полоса вкладок — отдельный вебвью с локальной страницей. Встраивать её
/// в страницу Gemini нельзя: чужая вёрстка меняется, и накладка ломалась бы.
pub const TABS_LABEL: &str = "tabs";
/// Вебвью с самим чатом.
pub const CHAT_LABEL: &str = "chat";

/// Высота полосы вкладок в логических пикселях.
pub const TAB_BAR: f64 = 38.0;

/// Отступ панели от края рабочей области и от иконки трея.
const MARGIN: i32 = 12;

/// Некоторые WM присылают `Focused(false)` сразу после `show()` — без этой паузы
/// панель схлопывалась бы в тот же момент, когда её вызвали.
const BLUR_GUARD: Duration = Duration::from_millis(400);

/// Клик по иконке трея сначала уводит фокус с панели, и она прячется сама;
/// пришедший следом toggle видит уже скрытое окно и открыл бы его заново.
/// В этом окне считаем, что панель ещё «была открыта», то есть клик её закрыл.
const RECENT_AUTO_HIDE: Duration = Duration::from_millis(300);

/// Хосты, которым разрешено открываться внутри панели. Всё остальное уходит
/// в системный браузер: иначе клик по ссылке в ответе Gemini уводит панель
/// на посторонний сайт и вернуться в чат уже нечем.
const ALLOWED_HOSTS: &[&str] = &[
    "google.com",
    "gstatic.com",
    "googleusercontent.com",
    "googleapis.com",
    "ggpht.com",
    "youtube.com",
];

/// Esc прячет панель. Страница удалённая, поэтому обработчик инжектируется
/// движком до загрузки документа (CSP Google на такие скрипты не действует),
/// а достучаться до Rust ему позволяет capability `gemini-remote`,
/// выдающая ровно одно право — `core:window:allow-hide`.
const ESC_TO_HIDE: &str = r#"
(function () {
  window.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape') return;
    var ipc = window.__TAURI_INTERNALS__;
    if (!ipc || typeof ipc.invoke !== 'function') return;
    ipc.invoke('plugin:window|hide', { label: 'panel' });
  }, true);
})();
"#;

fn host_allowed(url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => {}
        // about:blank и служебные схемы webview трогать не надо.
        _ => return true,
    }
    let Some(host) = url.host_str() else {
        return true;
    };
    ALLOWED_HOSTS
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
}

pub fn window<R: Runtime>(app: &AppHandle<R>) -> Option<Window<R>> {
    app.get_window(PANEL_LABEL)
}

pub fn chat<R: Runtime>(app: &AppHandle<R>) -> Option<Webview<R>> {
    app.get_webview(CHAT_LABEL)
}

pub fn build<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> tauri::Result<()> {
    let url: Url = settings.url.parse().map_err(tauri::Error::InvalidUrl)?;

    // Без явного data_directory wry на Linux не создаёт постоянное хранилище
    // cookies, и вход в Google слетал бы при каждом перезапуске.
    let data_dir = app.path().app_data_dir()?.join("webview");
    std::fs::create_dir_all(&data_dir)?;

    let window = WindowBuilder::new(app, PANEL_LABEL)
        .title("Gemini")
        .inner_size(settings.width, settings.height)
        .min_inner_size(360.0, 420.0)
        .visible(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(true)
        .build()?;

    let (w, h) = (settings.width, settings.height);

    window.add_child(
        WebviewBuilder::new(TABS_LABEL, WebviewUrl::App("tabs.html".into())),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(w, TAB_BAR),
    )?;

    let nav_app = app.clone();
    let mut chat = WebviewBuilder::new(CHAT_LABEL, WebviewUrl::External(url))
        .data_directory(data_dir)
        .zoom_hotkeys_enabled(true)
        .initialization_script(ESC_TO_HIDE)
        .on_navigation(move |url| {
            if host_allowed(url) {
                return true;
            }
            if let Err(e) = nav_app.opener().open_url(url.as_str(), None::<&str>) {
                eprintln!("не удалось открыть {url} во внешнем браузере: {e}");
            }
            false
        });

    let ua = settings.user_agent.trim();
    if !ua.is_empty() {
        chat = chat.user_agent(ua);
    }

    window.add_child(
        chat,
        LogicalPosition::new(0.0, TAB_BAR),
        LogicalSize::new(w, (h - TAB_BAR).max(1.0)),
    )?;

    pin_tab_bar_height(&window);
    Ok(())
}

/// На Linux tauri кладёт дочерние вебвью в `GtkBox` и всегда с `expand = true`,
/// поэтому заданные при создании границы игнорируются и окно делится поровну.
/// Просим GTK не растягивать первый вебвью и держать фиксированную высоту —
/// второй сам займёт остальное.
#[cfg(target_os = "linux")]
fn pin_tab_bar_height<R: Runtime>(window: &Window<R>) {
    use gtk::prelude::*;

    let Ok(vbox) = window.default_vbox() else {
        return;
    };
    let children = vbox.children();
    let Some(tabs) = children.first() else {
        return;
    };
    tabs.set_size_request(-1, TAB_BAR as i32);
    vbox.set_child_packing(tabs, false, false, 0, gtk::PackType::Start);
}

#[cfg(not(target_os = "linux"))]
fn pin_tab_bar_height<R: Runtime>(_window: &Window<R>) {}

/// Раскладывает вебвью после изменения размеров окна: полоса сверху во всю
/// ширину, чат — всё остальное.
#[cfg(target_os = "linux")]
pub fn layout<R: Runtime>(_app: &AppHandle<R>) {
    // Раскладку держит GTK — см. `pin_tab_bar_height`.
}

#[cfg(not(target_os = "linux"))]
pub fn layout<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = window(app) else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical = size.to_logical::<f64>(scale);

    if let Some(tabs) = app.get_webview(TABS_LABEL) {
        let _ = tabs.set_position(LogicalPosition::new(0.0, 0.0));
        let _ = tabs.set_size(LogicalSize::new(logical.width, TAB_BAR));
    }
    if let Some(chat) = app.get_webview(CHAT_LABEL) {
        let _ = chat.set_position(LogicalPosition::new(0.0, TAB_BAR));
        let _ = chat.set_size(LogicalSize::new(
            logical.width,
            (logical.height - TAB_BAR).max(1.0),
        ));
    }
}

pub fn toggle<R: Runtime>(app: &AppHandle<R>, anchor: Option<Rect>) {
    let Some(window) = window(app) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        hide(app);
        return;
    }
    if let Some(state) = app.try_state::<AppState>() {
        if state.auto_hidden_at.lock().unwrap().elapsed() < RECENT_AUTO_HIDE {
            return;
        }
    }
    show(app, anchor);
}

pub fn show<R: Runtime>(app: &AppHandle<R>, anchor: Option<Rect>) {
    let Some(window) = window(app) else {
        return;
    };
    let (want, size) = app
        .try_state::<AppState>()
        .map(|s| {
            let settings = s.settings.lock().unwrap();
            (
                settings.anchor,
                LogicalSize::new(settings.width, settings.height),
            )
        })
        .unwrap_or_else(|| {
            let d = Settings::default();
            (d.anchor, LogicalSize::new(d.width, d.height))
        });

    let placement = effective_size(&window, size).and_then(|px| position(&window, want, px, anchor));
    if let Err(e) = placement {
        eprintln!("не удалось спозиционировать панель: {e}");
    }
    if let Some(state) = app.try_state::<AppState>() {
        *state.blur_guard_until.lock().unwrap() = Instant::now() + BLUR_GUARD;
    }
    // После предпросмотра панель могла остаться нефокусируемой.
    let _ = window.set_focusable(true);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = window(app) else {
        return;
    };
    // Размер сохраняем при скрытии, а не по каждому Resized — событий на
    // перетаскивании рамки приходят сотни.
    if let (Ok(size), Some(state)) = (window.inner_size(), app.try_state::<AppState>()) {
        let scale = window.scale_factor().unwrap_or(1.0);
        let logical = size.to_logical::<f64>(scale);
        let mut settings = state.settings.lock().unwrap();
        if (settings.width - logical.width).abs() > 1.0
            || (settings.height - logical.height).abs() > 1.0
        {
            settings.width = logical.width;
            settings.height = logical.height;
            let _ = settings.save(app);
        }
    }
    let _ = window.hide();
}

/// Скрытие по потере фокуса — с оговорками: закреплённую панель не трогаем,
/// не реагируем на расфокус сразу после показа и не мешаем предпросмотру.
pub fn on_blur<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.settings.lock().unwrap().pinned {
        return;
    }
    if Instant::now() < *state.blur_guard_until.lock().unwrap() {
        return;
    }
    // Пока открыты настройки, фокус живёт в них, а панель нужна на виду
    // ради предпросмотра.
    if app
        .get_webview_window(crate::settings_window::SETTINGS_LABEL)
        .is_some()
    {
        return;
    }
    let visible = window(app).and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if !visible {
        return;
    }
    *state.auto_hidden_at.lock().unwrap() = Instant::now();
    hide(app);
}

/// Применяет к уже открытой панели то, что поменяли в настройках.
pub fn apply_settings<R: Runtime>(app: &AppHandle<R>, settings: &Settings) {
    let Some(window) = window(app) else {
        return;
    };
    let _ = window.set_size(LogicalSize::new(settings.width, settings.height));
    layout(app);
    // Пересчитываем место только у видимой панели: скрытая всё равно
    // спозиционируется заново при следующем показе.
    if window.is_visible().unwrap_or(false) {
        let size = LogicalSize::new(settings.width, settings.height);
        let placement =
            effective_size(&window, size).and_then(|px| position(&window, settings.anchor, px, None));
        if let Err(e) = placement {
            eprintln!("не удалось спозиционировать панель: {e}");
        }
    }
}

/// Показывает панель с ещё не сохранёнными размером и углом, чтобы правки
/// в настройках было видно сразу.
pub fn preview<R: Runtime>(app: &AppHandle<R>, want: Anchor, size: LogicalSize<f64>) {
    let Some(window) = window(app) else {
        return;
    };
    let _ = window.set_size(size);
    layout(app);
    let scale = window.scale_factor().unwrap_or(1.0);
    if let Err(e) = position(&window, want, size.to_physical(scale), None) {
        eprintln!("не удалось спозиционировать панель: {e}");
    }
    // Забрать фокус обратно после показа не выходит: GTK выдаёт его асинхронно
    // и перебивает наш вызов. Поэтому на время предпросмотра панель просто
    // не принимает фокус — править настройки надо продолжать в том же окне.
    let _ = window.set_focusable(false);
    let _ = window.show();
    if let Some(state) = app.try_state::<AppState>() {
        *state.previewing.lock().unwrap() = true;
    }
}

/// Закрыли настройки — панель, поднятую только ради предпросмотра, убираем.
pub fn end_preview<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let was_preview = std::mem::replace(&mut *state.previewing.lock().unwrap(), false);
    if let Some(window) = window(app) {
        let _ = window.set_focusable(true);
    }
    if was_preview && !state.settings.lock().unwrap().pinned {
        hide(app);
    }
}

/// Переход на закреплённый чат.
pub fn open_pin<R: Runtime>(app: &AppHandle<R>, index: usize) {
    let pin = app
        .try_state::<AppState>()
        .and_then(|s| s.settings.lock().unwrap().pins.get(index).cloned());
    let Some(pin) = pin else {
        return;
    };
    if let (Some(chat), Ok(url)) = (chat(app), pin.url.parse()) {
        let _ = chat.navigate(url);
    }
    set_active(app, Some(index));
    show(app, None);
}

/// Возврат на стартовый адрес — это и есть вкладка «Новый чат».
pub fn new_chat<R: Runtime>(app: &AppHandle<R>) {
    let url = app
        .try_state::<AppState>()
        .map(|s| s.settings.lock().unwrap().url.clone())
        .unwrap_or_else(|| crate::config::DEFAULT_URL.to_string());
    if let (Some(chat), Ok(url)) = (chat(app), url.parse()) {
        let _ = chat.navigate(url);
    }
    set_active(app, None);
}

pub fn reload<R: Runtime>(app: &AppHandle<R>) {
    if let Some(chat) = chat(app) {
        let _ = chat.reload();
    }
}

/// Текущий адрес чата — им заполняется новая закладка.
pub fn current_url<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    chat(app).and_then(|c| c.url().ok()).map(|u| u.to_string())
}

fn set_active<R: Runtime>(app: &AppHandle<R>, index: Option<usize>) {
    if let Some(state) = app.try_state::<AppState>() {
        *state.active_pin.lock().unwrap() = index;
    }
    notify_tabs(app);
}

/// Полоса вкладок живёт в своём вебвью и о смене закладок сама не узнает.
pub fn notify_tabs<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit("tabs-changed", ());
}

/// Кладёт открытый сейчас чат в первый свободный слот и возвращает обновлённые
/// настройки. Меню трея после этого надо пересобрать — это делает вызывающий.
pub fn pin_current<R: Runtime>(app: &AppHandle<R>) -> Result<Settings, String> {
    let url = current_url(app).ok_or("панель ещё не открыта")?;
    let parsed: Url = url
        .parse()
        .map_err(|_| format!("этот адрес нельзя закрепить: {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("этот адрес нельзя закрепить: {url}"));
    }
    if !is_specific_chat(&parsed) {
        return Err(
            "сначала откройте нужный чат: у нового своего адреса ещё нет, \
             Gemini выдаёт его после первого сообщения"
                .into(),
        );
    }
    let state = app
        .try_state::<AppState>()
        .ok_or("состояние приложения недоступно")?;

    let (settings, index) = {
        let mut settings = state.settings.lock().unwrap();
        if let Some(i) = settings.pins.iter().position(|p| p.url == url) {
            return Err(format!("этот чат уже закреплён как «{}»", settings.pins[i].title));
        }
        if settings.pins.len() >= MAX_PINS {
            return Err(format!(
                "закреплено уже {MAX_PINS} из {MAX_PINS} — освободите слот"
            ));
        }
        let title = format!("Чат {}", settings.pins.len() + 1);
        settings.pins.push(Pin { title, url });
        (settings.clone(), settings.pins.len() - 1)
    };

    settings
        .save(app)
        .map_err(|e| format!("не удалось записать settings.json: {e}"))?;
    set_active(app, Some(index));
    Ok(settings)
}

/// У нового чата собственного адреса нет — Gemini присваивает его только после
/// первого сообщения. Закреплять такую ссылку бессмысленно: получилась бы
/// вторая вкладка «Новый чат», что и произошло до этой проверки.
fn is_specific_chat(url: &Url) -> bool {
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    !matches!(segments.as_slice(), [] | ["app"])
}

/// Переименование закладки прямо из полосы вкладок.
pub fn rename_pin<R: Runtime>(app: &AppHandle<R>, index: usize, title: &str) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("название не может быть пустым".into());
    }
    let state = app
        .try_state::<AppState>()
        .ok_or("состояние приложения недоступно")?;
    let settings = {
        let mut settings = state.settings.lock().unwrap();
        let pin = settings.pins.get_mut(index).ok_or("такой закладки нет")?;
        pin.title = title.to_string();
        settings.clone()
    };
    settings
        .save(app)
        .map_err(|e| format!("не удалось записать settings.json: {e}"))?;
    notify_tabs(app);
    Ok(())
}

/// Снимает закладку. Если сняли активную — остаёмся на той же странице,
/// просто без подсветки: уводить пользователя с открытого чата незачем.
pub fn unpin<R: Runtime>(app: &AppHandle<R>, index: usize) -> Result<Settings, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or("состояние приложения недоступно")?;
    let settings = {
        let mut settings = state.settings.lock().unwrap();
        if index >= settings.pins.len() {
            return Err("такой закладки нет".into());
        }
        settings.pins.remove(index);
        settings.clone()
    };
    settings
        .save(app)
        .map_err(|e| format!("не удалось записать settings.json: {e}"))?;

    let mut active = state.active_pin.lock().unwrap();
    *active = match *active {
        Some(i) if i == index => None,
        Some(i) if i > index => Some(i - 1),
        other => other,
    };
    drop(active);
    notify_tabs(app);
    Ok(settings)
}

pub fn set_pinned<R: Runtime>(app: &AppHandle<R>, pinned: bool) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let mut settings = state.settings.lock().unwrap();
    settings.pinned = pinned;
    let _ = settings.save(app);
}

/// Прямоугольник иконки трея достоверен только на Windows; ayatana-appindicator
/// на Linux отдаёт нули, и тогда мы просто садимся в угол рабочей области.
fn usable_anchor(rect: &Rect, scale: f64) -> Option<(PhysicalPosition<i32>, i32, i32)> {
    let pos = rect.position.to_physical::<i32>(scale);
    let size = rect.size.to_physical::<u32>(scale);
    if size.width == 0 || size.height == 0 {
        return None;
    }
    Some((pos, size.width as i32, size.height as i32))
}

/// Размер, от которого считать положение. Ещё не показанное окно сообщает
/// `0x0`, и без подстраховки «справа снизу» поставило бы левый верхний угол
/// панели в угол экрана, то есть увело бы её за край.
fn effective_size<R: Runtime>(
    window: &Window<R>,
    fallback: LogicalSize<f64>,
) -> tauri::Result<PhysicalSize<u32>> {
    let size = window.outer_size()?;
    if size.width > 0 && size.height > 0 {
        return Ok(size);
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    Ok(fallback.to_physical(scale))
}

fn position<R: Runtime>(
    window: &Window<R>,
    want: Anchor,
    panel: PhysicalSize<u32>,
    anchor: Option<Rect>,
) -> tauri::Result<()> {
    let scale = window.scale_factor().unwrap_or(1.0);
    let anchor = anchor.as_ref().and_then(|r| usable_anchor(r, scale));

    // Монитор ищем по иконке трея, а если её нет — по курсору: панель должна
    // появляться на том экране, где сейчас работает пользователь.
    let probe = match (want, anchor) {
        (Anchor::Tray, Some((pos, _, _))) => Some((pos.x as f64, pos.y as f64)),
        _ => window.cursor_position().ok().map(|p| (p.x, p.y)),
    };
    let monitor = match probe {
        Some((x, y)) => window.monitor_from_point(x, y)?,
        None => None,
    }
    .or(window.primary_monitor()?);

    let Some(monitor) = monitor else {
        return Ok(());
    };
    let area = monitor.work_area();
    let (area_x, area_y) = (area.position.x, area.position.y);
    let (area_w, area_h) = (area.size.width as i32, area.size.height as i32);
    let (panel_w, panel_h) = (panel.width as i32, panel.height as i32);

    let right = area_x + area_w - panel_w - MARGIN;
    let bottom = area_y + area_h - panel_h - MARGIN;
    let left = area_x + MARGIN;
    let top = area_y + MARGIN;

    let (mut x, mut y) = match (want, anchor) {
        // К иконке трея — только если она сообщила свои координаты.
        (Anchor::Tray, Some((pos, icon_w, icon_h))) => {
            // Правый край панели равняем по правому краю иконки.
            let x = pos.x + icon_w - panel_w;
            // Трей может быть и сверху экрана — тогда роняем панель вниз от иконки.
            let y = if pos.y < area_y + area_h / 2 {
                pos.y + icon_h + MARGIN
            } else {
                pos.y - panel_h - MARGIN
            };
            (x, y)
        }
        // Linux отдаёт нулевой прямоугольник иконки — там это правый нижний угол.
        (Anchor::Tray, None) | (Anchor::BottomRight, _) => (right, bottom),
        (Anchor::BottomLeft, _) => (left, bottom),
        (Anchor::TopRight, _) => (right, top),
        (Anchor::TopLeft, _) => (left, top),
        (Anchor::Center, _) => (
            area_x + (area_w - panel_w) / 2,
            area_y + (area_h - panel_h) / 2,
        ),
    };

    x = x.clamp(area_x + MARGIN, (area_x + area_w - panel_w - MARGIN).max(area_x));
    y = y.clamp(area_y + MARGIN, (area_y + area_h - panel_h - MARGIN).max(area_y));

    window.set_position(PhysicalPosition::new(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        s.parse().unwrap()
    }

    #[test]
    fn new_chat_is_not_pinnable() {
        // Именно это и попадало в закладки до проверки: адрес нового чата,
        // то есть дубликат вкладки «Новый чат».
        assert!(!is_specific_chat(&u("https://gemini.google.com/app?hl=ru")));
        assert!(!is_specific_chat(&u("https://gemini.google.com/app")));
        assert!(!is_specific_chat(&u("https://gemini.google.com/app/")));
        assert!(!is_specific_chat(&u("https://gemini.google.com/")));
    }

    #[test]
    fn conversation_is_pinnable() {
        assert!(is_specific_chat(&u("https://gemini.google.com/app/1a2b3c")));
        assert!(is_specific_chat(&u("https://gemini.google.com/app/1a2b3c?hl=ru")));
        assert!(is_specific_chat(&u("https://gemini.google.com/gem/writer")));
    }

    #[test]
    fn only_google_hosts_open_inside_panel() {
        assert!(host_allowed(&u("https://gemini.google.com/app")));
        assert!(host_allowed(&u("https://accounts.google.com/signin")));
        assert!(host_allowed(&u("https://lh3.googleusercontent.com/a")));
        assert!(!host_allowed(&u("https://example.com/")));
        assert!(!host_allowed(&u("https://notgoogle.com/")));
        // Подделка вида evil-google.com.attacker.net не должна пройти.
        assert!(!host_allowed(&u("https://google.com.attacker.net/")));
    }
}
