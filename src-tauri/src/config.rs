use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

pub const DEFAULT_URL: &str = "https://gemini.google.com/app?hl=ru";
pub const DEFAULT_HOTKEY: &str = "CmdOrCtrl+Shift+KeyG";

pub const MIN_WIDTH: f64 = 320.0;
pub const MIN_HEIGHT: f64 = 300.0;
pub const MAX_WIDTH: f64 = 2400.0;
pub const MAX_HEIGHT: f64 = 2000.0;

/// Закладок ровно три: они живут в меню трея, и длинный список там
/// превращается в то же самое рытьё, ради ухода от которого всё затевалось.
pub const MAX_PINS: usize = 3;

/// Куда прижимать панель при показе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    /// К иконке в трее. На Linux ayatana-appindicator координат иконки
    /// не отдаёт, поэтому там это равносильно правому нижнему углу.
    Tray,
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
    Center,
}

impl Default for Anchor {
    fn default() -> Self {
        // Правый нижний угол, а не иконка в трее: он ведёт себя одинаково
        // на обеих системах, тогда как координаты иконки доступны только в Windows.
        Self::BottomRight
    }
}

/// Закреплённый чат: адрес конкретной беседы в Gemini и подпись для меню.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pin {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Стартовый адрес панели. Пункт «Новый чат» возвращает именно сюда.
    pub url: String,
    /// Глобальная горячая клавиша, синтаксис `Ctrl+Shift+KeyG`.
    pub hotkey: String,
    /// Пусто — User-Agent движка по умолчанию, и так должно быть в норме.
    /// Заполнять только если Google отказывается пускать в аккаунт; см. README.
    pub user_agent: String,
    pub anchor: Anchor,
    pub width: f64,
    pub height: f64,
    /// Закреплённая панель не прячется при потере фокуса.
    pub pinned: bool,
    /// Закреплённые чаты, не больше `MAX_PINS`.
    pub pins: Vec<Pin>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_string(),
            hotkey: DEFAULT_HOTKEY.to_string(),
            user_agent: String::new(),
            anchor: Anchor::default(),
            width: 460.0,
            height: 680.0,
            pinned: false,
            pins: Vec::new(),
        }
    }
}

impl Settings {
    fn path<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<PathBuf> {
        Ok(app.path().app_config_dir()?.join("settings.json"))
    }

    /// Битый или отсутствующий файл — не повод падать: берём умолчания.
    /// Второе значение — признак самого первого запуска: настроек ещё не было,
    /// значит пользователь и в аккаунт не входил.
    pub fn load<R: Runtime>(app: &AppHandle<R>) -> (Self, bool) {
        let Ok(path) = Self::path(app) else {
            return (Self::default(), true);
        };
        match std::fs::read_to_string(&path) {
            Ok(raw) => {
                let mut settings: Self = serde_json::from_str(&raw).unwrap_or_else(|e| {
                    eprintln!("settings.json не разобран ({e}), беру значения по умолчанию");
                    Self::default()
                });
                settings.sanitize();
                (settings, false)
            }
            Err(_) => {
                let defaults = Self::default();
                let _ = defaults.save(app);
                (defaults, true)
            }
        }
    }

    /// Приводит значения в границы, из которых окно ещё можно вернуть:
    /// правленый вручную файл не должен превращать панель в полоску.
    pub fn sanitize(&mut self) {
        if self.url.trim().is_empty() {
            self.url = DEFAULT_URL.to_string();
        } else {
            self.url = self.url.trim().to_string();
        }
        if self.hotkey.trim().is_empty() {
            self.hotkey = DEFAULT_HOTKEY.to_string();
        } else {
            self.hotkey = self.hotkey.trim().to_string();
        }
        self.user_agent = self.user_agent.trim().to_string();
        self.width = self.width.clamp(MIN_WIDTH, MAX_WIDTH);
        self.height = self.height.clamp(MIN_HEIGHT, MAX_HEIGHT);

        // Пустые слоты из окна настроек приходят как есть — выкидываем их здесь,
        // чтобы ниже по коду закладка всегда означала рабочий адрес.
        for pin in &mut self.pins {
            pin.title = pin.title.trim().to_string();
            pin.url = pin.url.trim().to_string();
        }
        self.pins
            .retain(|p| p.url.starts_with("http://") || p.url.starts_with("https://"));
        self.pins.truncate(MAX_PINS);
        for (i, pin) in self.pins.iter_mut().enumerate() {
            if pin.title.is_empty() {
                pin.title = format!("Чат {}", i + 1);
            }
        }
    }

    pub fn save<R: Runtime>(&self, app: &AppHandle<R>) -> tauri::Result<()> {
        let path = Self::path(app)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        Ok(())
    }
}
