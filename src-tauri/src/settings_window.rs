use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

pub const SETTINGS_LABEL: &str = "settings";

pub fn open<R: Runtime>(app: &AppHandle<R>) {
    // Окно создаётся по требованию: держать его в памяти ради пары настроек
    // незачем, а приложение всё время висит в трее.
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let built = WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::App("index.html".into()))
        .title("Настройки — Gemini Tray")
        .inner_size(520.0, 720.0)
        .min_inner_size(460.0, 520.0)
        .resizable(true)
        .center()
        // Панель — always-on-top, и при предпросмотре она накрыла бы настройки,
        // оборвав перетаскивание ползунка. Держим настройки выше неё.
        .always_on_top(true)
        .build();

    if let Err(e) = built {
        eprintln!("не удалось открыть окно настроек: {e}");
    }
}
