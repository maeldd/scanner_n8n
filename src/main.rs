#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod model;
mod outbox;

use catalog::Catalog;
use chrono::{DateTime, Local, Utc};
use eframe::egui::{self, Align, Color32, Layout, Margin, RichText, Rounding, Stroke, Vec2};
use model::{Action, MovementState, PendingMovement, ScanResult, parse_scan};
use outbox::{EventStatus, Outbox, QueuedEvent};
use qrcodegen::{QrCode as QrCodeGen, QrCodeEcc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const CONFIG_FILE: &str = "scanner_config.json";
const OUTBOX_FILE: &str = "outbox.json";
const CATALOG_FILE: &str = "references.csv";
const LOG_LIMIT: usize = 5_000;
const BAUD_RATES: [u32; 5] = [9600, 19200, 38400, 57600, 115200];

/// Contenu du QR code d'annulation imprimé et affiché dans l'application.
/// Il est reconnu avant l'analyse `IN|...` / `OUT|...` : le scanner peut donc
/// annuler la saisie en cours sans toucher au clavier.
const CANCEL_QR_CODE: &str = "ANNULER";
const CANCEL_LABEL_FILE: &str = "etiquette-annulation.html";

/// Dossier de configuration propre à l'utilisateur (utilisé quand l'app est
/// lancée depuis un bundle macOS ou un raccourci Windows, sans dossier courant
/// fiable). En développement, le fichier local à côté du binaire reste prioritaire.
fn config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support/scanner_n8n");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("scanner_n8n");
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("scanner_n8n");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".config/scanner_n8n");
        }
    }
    PathBuf::from(".")
}

fn config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SCANNER_N8N_CONFIG") {
        return PathBuf::from(path);
    }

    let local = PathBuf::from(CONFIG_FILE);
    if local.exists() {
        return local;
    }

    config_dir().join(CONFIG_FILE)
}

/// Fichier de données (file d'attente, etc.) placé à côté de la configuration.
fn data_path(file: &str) -> PathBuf {
    let config = config_path();
    config
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(file))
        .unwrap_or_else(|| PathBuf::from(file))
}

fn catalog_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SCANNER_N8N_CATALOG") {
        return PathBuf::from(path);
    }

    let user = config_dir().join(CATALOG_FILE);
    if user.exists() {
        return user;
    }

    let local = PathBuf::from(CATALOG_FILE);
    if local.exists() {
        return local;
    }

    user
}

fn default_station_id() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "POSTE-01".to_string())
}

/// Vrai si le code scanné est le QR d'annulation (et non un mouvement).
fn is_cancel_command(code: &str) -> bool {
    code.trim().eq_ignore_ascii_case(CANCEL_QR_CODE)
}

/// Matrice du QR d'annulation, calculée une seule fois au premier affichage.
struct QrMatrix {
    size: usize,
    modules: Vec<bool>,
}

fn cancel_qr() -> &'static QrMatrix {
    static MATRIX: OnceLock<QrMatrix> = OnceLock::new();
    MATRIX.get_or_init(|| {
        let code = QrCodeGen::encode_text(CANCEL_QR_CODE, QrCodeEcc::Medium)
            .expect("le QR d'annulation doit toujours être encodable");
        let size = code.size() as usize;
        let mut modules = Vec::with_capacity(size * size);
        for y in 0..size {
            for x in 0..size {
                modules.push(code.get_module(x as i32, y as i32));
            }
        }
        QrMatrix { size, modules }
    })
}

/// Dessine le QR d'annulation dans l'interface, avec une marge blanche de
/// 4 modules pour rester lisible par le scanner.
fn draw_cancel_qr(ui: &mut egui::Ui, size: f32) -> egui::Response {
    const QUIET: usize = 4;

    let matrix = cancel_qr();
    let total = (matrix.size + QUIET * 2) as f32;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, Rounding::same(4.0), Color32::WHITE);

    let module = size / total;
    for y in 0..matrix.size {
        for x in 0..matrix.size {
            if !matrix.modules[y * matrix.size + x] {
                continue;
            }
            let min =
                rect.min + egui::vec2((x + QUIET) as f32 * module, (y + QUIET) as f32 * module);
            painter.rect_filled(
                egui::Rect::from_min_size(min, Vec2::splat(module + 0.25)),
                0.0,
                Color32::BLACK,
            );
        }
    }

    response
}

/// Version SVG du QR, utilisée dans la page HTML imprimable.
fn qr_svg(matrix: &QrMatrix) -> String {
    use std::fmt::Write as _;

    const QUIET: usize = 4;
    let total = matrix.size + QUIET * 2;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {total} {total}\" \
         width=\"100%\" height=\"100%\" shape-rendering=\"crispEdges\" role=\"img\" \
         aria-label=\"QR code annuler la saisie\">\
         <rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>"
    );

    for y in 0..matrix.size {
        for x in 0..matrix.size {
            if matrix.modules[y * matrix.size + x] {
                let _ = write!(
                    svg,
                    "<rect x=\"{}\" y=\"{}\" width=\"1\" height=\"1\" fill=\"#000000\"/>",
                    x + QUIET,
                    y + QUIET
                );
            }
        }
    }

    svg.push_str("</svg>");
    svg
}

fn html_escape(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            _ => character.to_string(),
        })
        .collect()
}

/// Étiquette 50 × 30 mm prête à imprimer sur une imprimante thermique.
/// Une seule étiquette par page, sans marge, pour ne pas décaler le format.
/// Le navigateur ouvre directement la boîte d'impression (`window.print()` au
/// chargement).
fn build_cancel_label_html(station: &str) -> String {
    let station = html_escape(station.trim());
    let station = if station.is_empty() {
        "POSTE-01".to_string()
    } else {
        station
    };
    let qr = qr_svg(cancel_qr());

    let css = r#"
  @page { size: 50mm 30mm; margin: 0; }
  * { box-sizing: border-box; }
  html, body { margin: 0; padding: 0; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Arial, sans-serif;
    color: #000;
    background: #e2e8f0;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 6mm;
    padding: 6mm;
    font-size: 4mm;
    color: #334155;
  }
  .toolbar button {
    font: inherit;
    font-weight: 600;
    padding: 3mm 8mm;
    border: 0;
    border-radius: 2mm;
    background: #4f46e5;
    color: #fff;
    cursor: pointer;
  }
  .sheet {
    display: flex;
    justify-content: center;
    padding: 6mm;
  }
  .label {
    display: flex;
    align-items: center;
    gap: 1.5mm;
    width: 50mm;
    height: 30mm;
    padding: 1.2mm 1.4mm;
    overflow: hidden;
    background: #fff;
    color: #000;
  }
  .qr { flex: 0 0 22mm; width: 22mm; height: 22mm; }
  .copy {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
  }
  .title {
    font-size: 3.9mm;
    line-height: 0.95;
    font-weight: 800;
    letter-spacing: 0.05mm;
  }
  .hint { margin: 0.8mm 0 0; font-size: 2.35mm; line-height: 1.12; }
  .line {
    margin-top: 1.2mm;
    padding-top: 0.4mm;
    border-top: 0.25mm solid #000;
    font-size: 2.3mm;
    line-height: 1.05;
    white-space: nowrap;
  }
  .station {
    margin: 0.7mm 0 0;
    font-size: 1.65mm;
    line-height: 1;
    color: #444;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  @media print {
    body { background: #fff; width: 50mm; height: 30mm; }
    .toolbar { display: none; }
    .sheet { display: block; padding: 0; }
    .label { margin: 0; }
  }
"#;

    format!(
        r#"<!doctype html>
<html lang="fr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Étiquette · Annuler la saisie</title>
<style>{css}</style>
</head>
<body onload="window.print()">
  <div class="toolbar">
    <button type="button" onclick="window.print()">Imprimer</button>
    <span>Étiquette 50 × 30 mm pour imprimante thermique · 1 étiquette par page.</span>
  </div>
  <div class="sheet">
    <section class="label">
      <div class="qr">{qr}</div>
      <div class="copy">
        <div class="title">ANNULER<br>LA SAISIE</div>
        <div class="hint">Scanner pour annuler la saisie en cours.</div>
        <div class="line">Ligne : ______________</div>
        <div class="station">Poste : {station}</div>
      </div>
    </section>
  </div>
</body>
</html>"#
    )
}

/// Ouvre un fichier avec l'application par défaut du système (navigateur pour
/// la page d'impression).
fn open_path(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

// Icônes embarquées (variantes sombre et claire)
const ICON_DARK_PNG: &[u8] = include_bytes!("../assets/icon-dark.png");
const ICON_LIGHT_PNG: &[u8] = include_bytes!("../assets/icon-light.png");

fn icon_png(dark: bool) -> &'static [u8] {
    if dark { ICON_DARK_PNG } else { ICON_LIGHT_PNG }
}

fn load_icon(dark: bool) -> egui::IconData {
    eframe::icon_data::from_png_bytes(icon_png(dark))
        .expect("Impossible de charger l'icône de l'application")
}

/// Détecte l'apparence du système avant la création de la fenêtre,
/// afin d'afficher directement la bonne variante d'icône.
#[cfg(target_os = "macos")]
fn system_prefers_dark() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .eq_ignore_ascii_case("Dark")
        })
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn system_prefers_dark() -> bool {
    use std::os::windows::process::CommandExt;

    // Évite l'apparition fugitive d'une console au lancement (app GUI).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = std::process::Command::new("reg");
    cmd.args([
        "query",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "/v",
        "AppsUseLightTheme",
    ]);
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .last()
                == Some("0x0")
        })
        .unwrap_or(true)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_prefers_dark() -> bool {
    std::env::var("GTK_THEME")
        .map(|theme| theme.to_lowercase().contains("dark"))
        .unwrap_or(true)
}

/// Applique l'icône de la fenêtre et, sur macOS, celle du Dock.
fn apply_app_icon(ctx: &egui::Context, dark: bool) {
    #[cfg(target_os = "macos")]
    set_macos_app_icon(icon_png(dark));

    let icon = load_icon(dark);
    ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(Arc::new(icon))));
}

#[cfg(target_os = "macos")]
fn set_macos_app_icon(png: &'static [u8]) {
    use objc2::ClassType as _;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    unsafe {
        let data = NSData::with_bytes(png);
        if let Some(image) = NSImage::initWithDataIgnoringOrientation(NSImage::alloc(), &data) {
            NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
        }
    }
}

// ==========================================
// Modèles de données & Configuration
// ==========================================

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
enum ThemeMode {
    /// Suit automatiquement l'apparence du système (par défaut).
    #[default]
    #[serde(alias = "System")]
    System,
    #[serde(alias = "Dark")]
    Dark,
    #[serde(alias = "Light")]
    Light,
}

impl ThemeMode {
    fn next(self) -> Self {
        match self {
            Self::System => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::System,
        }
    }

    fn icon_and_label(self) -> (&'static str, &'static str) {
        match self {
            Self::System => ("🌗", "Auto"),
            Self::Dark => ("🌙", "Sombre"),
            Self::Light => ("☀️", "Clair"),
        }
    }

    fn hover_text(self) -> &'static str {
        match self {
            Self::System => {
                "Thème automatique : suit le mode clair/sombre du système. Cliquer pour forcer le mode sombre."
            }
            Self::Dark => "Mode sombre forcé. Cliquer pour forcer le mode clair.",
            Self::Light => {
                "Mode clair forcé. Cliquer pour revenir au suivi automatique du système."
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
struct AppConfig {
    webhook_url: String,
    device_id: String,
    selected_port: String,
    baud_rate: u32,
    #[serde(default)]
    theme_mode: ThemeMode,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    auto_start: bool,
    #[serde(default = "default_auto_commit_secs")]
    auto_commit_secs: u64,
    #[serde(default = "default_debounce_ms")]
    debounce_ms: u64,
}

fn default_auto_commit_secs() -> u64 {
    5
}

fn default_debounce_ms() -> u64 {
    300
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            webhook_url: String::new(),
            device_id: default_station_id(),
            selected_port: String::new(),
            baud_rate: 9600,
            theme_mode: ThemeMode::System,
            api_key: String::new(),
            auto_start: false,
            auto_commit_secs: default_auto_commit_secs(),
            debounce_ms: default_debounce_ms(),
        }
    }
}

impl AppConfig {
    /// Charge la configuration. Retourne aussi un message d'erreur si le
    /// fichier existe mais est illisible/invalide, afin de ne pas perdre
    /// silencieusement l'URL du webhook ou le port série.
    fn load() -> (Self, Option<String>) {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<Self>(&data) {
                Ok(config) => (config, None),
                Err(error) => (
                    Self::default(),
                    Some(format!(
                        "Configuration invalide dans {} : {error}. Valeurs par défaut utilisées.",
                        path.display()
                    )),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(error) => (
                Self::default(),
                Some(format!(
                    "Impossible de lire {} : {error}. Valeurs par défaut utilisées.",
                    path.display()
                )),
            ),
        }
    }

    fn save(&self) {
        if let Ok(data) = serde_json::to_string_pretty(self) {
            let path = config_path();
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            // Écriture atomique : évite un fichier tronqué si l'application est
            // fermée pendant la sauvegarde.
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, data).is_ok() {
                #[cfg(target_os = "windows")]
                {
                    let _ = fs::remove_file(&path);
                }
                let _ = fs::rename(tmp, path);
            }
        }
    }
}

// ==========================================
// Thème & Palette Dynamique (Dark / Light)
// ==========================================

#[derive(Clone, Copy)]
struct ThemeColors {
    is_dark: bool,
    bg_panel: Color32,
    bg_card: Color32,
    border_card: Color32,
    text_primary: Color32,
    text_muted: Color32,
    bg_console: Color32,
    border_console: Color32,
    accent: Color32,
}

impl ThemeColors {
    fn new(is_dark: bool) -> Self {
        if is_dark {
            Self {
                is_dark: true,
                bg_panel: Color32::from_rgb(14, 16, 24),
                bg_card: Color32::from_rgb(22, 26, 38),
                border_card: Color32::from_rgb(38, 44, 64),
                text_primary: Color32::from_rgb(241, 245, 249),
                text_muted: Color32::from_rgb(148, 163, 184),
                bg_console: Color32::from_rgb(9, 11, 16),
                border_console: Color32::from_rgb(28, 33, 48),
                accent: Color32::from_rgb(99, 102, 241),
            }
        } else {
            Self {
                is_dark: false,
                bg_panel: Color32::from_rgb(241, 245, 249),
                bg_card: Color32::from_rgb(255, 255, 255),
                border_card: Color32::from_rgb(226, 232, 240),
                text_primary: Color32::from_rgb(15, 23, 42),
                text_muted: Color32::from_rgb(100, 116, 139),
                bg_console: Color32::from_rgb(248, 250, 252),
                border_console: Color32::from_rgb(203, 213, 225),
                accent: Color32::from_rgb(79, 70, 229),
            }
        }
    }
}

#[derive(Serialize)]
struct WebhookPayload<'a> {
    /// Champs historiques conservés pour ne pas casser un workflow n8n existant.
    barcode: &'a str,
    timestamp: &'a str,
    device_id: &'a str,

    /// Nouveau contrat « mouvement de stock ».
    event_id: &'a str,
    seq: u64,
    sku: &'a str,
    action: &'a str,
    qty: u32,
    station_id: &'a str,
    scanned_at: &'a str,
    raw: &'a str,
    app_version: &'a str,
    motif: Option<&'a str>,
    cancels_event_id: Option<&'a str>,
    is_test: Option<bool>,
}

#[derive(Deserialize)]
struct WebhookResponse {
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    stock_after: Option<i64>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    available: Option<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
    Scan,
    Raw,
}

struct LogMessage {
    timestamp: String,
    level: LogLevel,
    tag: String,
    text: String,
}

struct OutboxUpdate {
    event_id: String,
    sku: String,
    action: String,
    qty: u32,
    motif: Option<String>,
    status: EventStatus,
    stock_after: Option<i64>,
    message: Option<String>,
    code: Option<String>,
    available: Option<i64>,
}

enum AppEvent {
    Log(LogMessage),
    Scan { code: String, at: DateTime<Utc> },
    OutboxUpdate(OutboxUpdate),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommitStatus {
    Queued,
    Sent,
    Rejected,
    Cancelled,
}

#[derive(Clone)]
struct LastCommit {
    event_id: String,
    sku: String,
    name: String,
    action: Action,
    qty: u32,
    status: CommitStatus,
    message: String,
    stock_after: Option<i64>,
    at: Instant,
}

struct Flash {
    color: Color32,
    text: String,
    until: Instant,
}

#[derive(Default)]
struct AppStats {
    total_movements: usize,
    sent_success: usize,
    sent_failed: usize,
}

// ==========================================
// Application Principale
// ==========================================

struct ScannerApp {
    config: AppConfig,
    available_ports: Vec<String>,
    is_listening: bool,
    running_flag: Arc<AtomicBool>,
    event_rx: Receiver<AppEvent>,
    event_tx: Sender<AppEvent>,
    logs: Vec<LogMessage>,
    stats: AppStats,
    auto_scroll: bool,
    copied_time: Option<Instant>,
    webhook_url_shared: Arc<RwLock<String>>,
    api_key_shared: Arc<RwLock<String>>,
    outbox: Arc<Mutex<Outbox>>,
    sender_running: Arc<AtomicBool>,
    catalog: Catalog,
    movement: MovementState,
    auto_commit: Duration,
    last_commit: Option<LastCommit>,
    flash: Option<Flash>,
    boot_started: bool,
    focus_reset: bool,
    icon_dark: Option<bool>,
}

impl ScannerApp {
    fn new(cc: &eframe::CreationContext<'_>, system_dark: bool) -> Self {
        let (mut config, config_error) = AppConfig::load();

        // Appliquer immédiatement le thème (Auto par défaut : suit le système)
        Self::apply_theme(&cc.egui_ctx, config.theme_mode);

        // Icône initiale : variante correspondant à l'apparence du système
        apply_app_icon(&cc.egui_ctx, system_dark);

        let (event_tx, event_rx) = channel();

        // Récupérer les ports disponibles
        let mut available_ports = Vec::new();
        if let Ok(ports) = serialport::available_ports() {
            available_ports = ports.into_iter().map(|p| p.port_name).collect();
            // Ne remplacer un port absent que si l'utilisateur n'en a jamais
            // choisi : sinon on pourrait ouvrir un autre périphérique série.
            if config.selected_port.is_empty() && !available_ports.is_empty() {
                config.selected_port = available_ports[0].clone();
            }
        }

        let webhook_url_shared = Arc::new(RwLock::new(config.webhook_url.clone()));
        let api_key_shared = Arc::new(RwLock::new(config.api_key.clone()));
        let outbox = Arc::new(Mutex::new(Outbox::load(data_path(OUTBOX_FILE))));
        let sender_running = Arc::new(AtomicBool::new(true));

        spawn_sender(
            Arc::clone(&outbox),
            Arc::clone(&sender_running),
            Arc::clone(&webhook_url_shared),
            Arc::clone(&api_key_shared),
            event_tx.clone(),
        );

        let catalog_file = catalog_path();
        let (catalog, catalog_level, catalog_message) = match Catalog::load(&catalog_file) {
            Ok(catalog) => {
                let count = catalog.len();
                (
                    catalog,
                    LogLevel::Info,
                    format!(
                        "{count} référence(s) chargée(s) depuis {}.",
                        catalog_file.display()
                    ),
                )
            }
            Err(error) => (
                Catalog::empty(),
                LogLevel::Warning,
                format!(
                    "Catalogue indisponible : {error} Lancez scripts/import-notion-csv.py pour le générer."
                ),
            ),
        };

        let auto_commit = Duration::from_secs(config.auto_commit_secs.max(1));
        let debounce = Duration::from_millis(config.debounce_ms.clamp(100, 2_000));
        let pending_count = outbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .counts()
            .pending;

        let mut app = Self {
            config,
            available_ports,
            is_listening: false,
            running_flag: Arc::new(AtomicBool::new(false)),
            event_rx,
            event_tx: event_tx.clone(),
            logs: Vec::new(),
            stats: AppStats::default(),
            auto_scroll: true,
            copied_time: None,
            webhook_url_shared,
            api_key_shared,
            outbox,
            sender_running,
            catalog,
            movement: MovementState::new(debounce, auto_commit),
            auto_commit,
            last_commit: None,
            flash: None,
            boot_started: false,
            focus_reset: false,
            icon_dark: Some(system_dark),
        };

        app.add_log(
            LogLevel::Info,
            "SYSTÈME",
            "Application initialisée. Scannez une étiquette IN|... ou OUT|....",
        );
        app.add_log(catalog_level, "CATALOGUE", &catalog_message);
        if let Some(error) = config_error {
            app.add_log(LogLevel::Warning, "CONFIG", &error);
        }
        if app.config.webhook_url.trim().is_empty() {
            app.add_log(
                LogLevel::Warning,
                "CONFIG",
                "URL de webhook vide : les mouvements resteront en file locale tant qu'elle n'est pas renseignée.",
            );
        }
        if pending_count > 0 {
            app.add_log(
                LogLevel::Info,
                "FILE",
                &format!(
                    "{pending_count} mouvement(s) en attente de synchronisation, rejeu automatique."
                ),
            );
        }

        app
    }

    fn apply_theme(ctx: &egui::Context, mode: ThemeMode) {
        ctx.set_theme(match mode {
            ThemeMode::System => egui::ThemePreference::System,
            ThemeMode::Dark => egui::ThemePreference::Dark,
            ThemeMode::Light => egui::ThemePreference::Light,
        });

        // Les deux palettes sont configurées : en mode System, egui bascule
        // automatiquement de l'une à l'autre quand le système change.
        let mut dark = egui::Visuals::dark();

        dark.panel_fill = Color32::from_rgb(14, 16, 24);
        dark.window_fill = Color32::from_rgb(20, 23, 34);
        dark.extreme_bg_color = Color32::from_rgb(10, 11, 17);
        dark.faint_bg_color = Color32::from_rgb(26, 30, 44);
        dark.override_text_color = Some(Color32::from_rgb(241, 245, 249));

        dark.widgets.noninteractive.rounding = Rounding::same(8.0);
        dark.widgets.inactive.rounding = Rounding::same(8.0);
        dark.widgets.hovered.rounding = Rounding::same(8.0);
        dark.widgets.active.rounding = Rounding::same(8.0);

        dark.widgets.inactive.bg_fill = Color32::from_rgb(28, 32, 48);
        dark.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(44, 50, 74));

        dark.widgets.hovered.bg_fill = Color32::from_rgb(38, 44, 66);
        dark.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(99, 102, 241));

        dark.widgets.active.bg_fill = Color32::from_rgb(79, 70, 229);

        ctx.set_visuals_of(egui::Theme::Dark, dark);

        let mut light = egui::Visuals::light();

        light.panel_fill = Color32::from_rgb(241, 245, 249);
        light.window_fill = Color32::from_rgb(255, 255, 255);
        light.extreme_bg_color = Color32::from_rgb(255, 255, 255);
        light.faint_bg_color = Color32::from_rgb(241, 245, 249);
        light.override_text_color = Some(Color32::from_rgb(15, 23, 42));

        light.widgets.noninteractive.rounding = Rounding::same(8.0);
        light.widgets.inactive.rounding = Rounding::same(8.0);
        light.widgets.hovered.rounding = Rounding::same(8.0);
        light.widgets.active.rounding = Rounding::same(8.0);

        light.widgets.inactive.bg_fill = Color32::from_rgb(255, 255, 255);
        light.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(203, 213, 225));

        light.widgets.hovered.bg_fill = Color32::from_rgb(241, 245, 249);
        light.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(99, 102, 241));

        light.widgets.active.bg_fill = Color32::from_rgb(79, 70, 229);

        ctx.set_visuals_of(egui::Theme::Light, light);

        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |style| {
                style.spacing.item_spacing = Vec2::new(8.0, 8.0);
                style.spacing.button_padding = Vec2::new(12.0, 7.0);
            });
        }
    }

    fn add_log(&mut self, level: LogLevel, tag: &str, text: &str) {
        self.push_log(LogMessage {
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            level,
            tag: tag.to_string(),
            text: text.to_string(),
        });
    }

    fn push_log(&mut self, message: LogMessage) {
        // Journal fichier optionnel, utile sur un poste de production où la
        // fenêtre n'est pas toujours visible : SCANNER_N8N_LOG_FILE=/tmp/scanner.log
        if let Some(path) = std::env::var_os("SCANNER_N8N_LOG_FILE") {
            use std::io::Write as _;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(
                    file,
                    "{} [{}] {}",
                    message.timestamp, message.tag, message.text
                );
            }
        }
        self.logs.push(message);
        if self.logs.len() > LOG_LIMIT {
            let excess = self.logs.len() - LOG_LIMIT;
            self.logs.drain(..excess);
        }
    }

    fn refresh_ports(&mut self) {
        if let Ok(ports) = serialport::available_ports() {
            self.available_ports = ports.into_iter().map(|p| p.port_name).collect();
            if self.config.selected_port.is_empty() && !self.available_ports.is_empty() {
                self.config.selected_port = self.available_ports[0].clone();
            }
            self.add_log(
                LogLevel::Info,
                "PORTS",
                &format!("{} port(s) détecté(s)", self.available_ports.len()),
            );
        } else {
            self.add_log(
                LogLevel::Warning,
                "PORTS",
                "Échec de l'énumération des ports série.",
            );
        }
    }

    fn test_webhook(&mut self) {
        let url = self.config.webhook_url.trim().to_string();
        if url.is_empty() {
            self.add_log(
                LogLevel::Warning,
                "CONFIG",
                "Veuillez renseigner l'URL du webhook n8n avant de tester.",
            );
            return;
        }

        self.config.save();
        let dev_id = self.config.device_id.clone();
        let api_key = self.config.api_key.clone();
        let tx = self.event_tx.clone();

        self.add_log(
            LogLevel::Info,
            "TEST",
            "Envoi d'un payload test (is_test=true). Le workflow doit l'ignorer.",
        );

        thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default();

            let event_id = Uuid::new_v4().to_string();
            let scanned_at = Utc::now().to_rfc3339();
            let payload = WebhookPayload {
                barcode: "TEST-SCAN-123456",
                timestamp: &scanned_at,
                device_id: &dev_id,
                event_id: &event_id,
                seq: 0,
                sku: "TEST-SCAN-123456",
                action: "IN",
                qty: 1,
                station_id: &dev_id,
                scanned_at: &scanned_at,
                raw: "TEST",
                app_version: env!("CARGO_PKG_VERSION"),
                motif: Some("Test"),
                cancels_event_id: None,
                is_test: Some(true),
            };

            let mut request = client
                .post(&url)
                .header("Idempotency-Key", &event_id)
                .json(&payload);
            if !api_key.is_empty() {
                request = request.header("X-Scanner-Key", &api_key);
            }

            let now = Local::now().format("%H:%M:%S").to_string();
            match request.send() {
                Ok(res) => {
                    let status = res.status();
                    let body = res.text().unwrap_or_default();
                    let parsed: Option<WebhookResponse> = serde_json::from_str(&body).ok();
                    let business_ok = parsed
                        .as_ref()
                        .and_then(|response| response.ok)
                        .unwrap_or(status.is_success());

                    if status.is_success() && business_ok {
                        let _ = tx.send(AppEvent::Log(LogMessage {
                            timestamp: now,
                            level: LogLevel::Success,
                            tag: format!("n8n {}", status.as_u16()),
                            text: "Test réussi : le webhook a répondu correctement.".to_string(),
                        }));
                    } else {
                        let message = parsed
                            .and_then(|response| response.message)
                            .filter(|message| !message.is_empty())
                            .unwrap_or_else(|| {
                                format!("statut HTTP {} — {}", status.as_u16(), body.trim())
                            });
                        let _ = tx.send(AppEvent::Log(LogMessage {
                            timestamp: now,
                            level: LogLevel::Error,
                            tag: format!("n8n {}", status.as_u16()),
                            text: format!("Test refusé : {message}"),
                        }));
                    }
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::Log(LogMessage {
                        timestamp: now,
                        level: LogLevel::Error,
                        tag: "HTTP ERR".to_string(),
                        text: format!("Erreur réseau lors du test : {error}"),
                    }));
                }
            }
        });
    }

    fn toggle_listening(&mut self) {
        if self.is_listening {
            self.running_flag.store(false, Ordering::SeqCst);
            // Le thread arrêté garde son propre Arc : un redémarrage immédiat
            // crée un nouveau drapeau et ne peut pas réveiller l'ancien thread.
            self.running_flag = Arc::new(AtomicBool::new(false));
            self.is_listening = false;
            self.add_log(LogLevel::Info, "SYSTÈME", "Arrêt de la capture demandé.");
        } else {
            if self.config.selected_port.is_empty() {
                self.add_log(
                    LogLevel::Error,
                    "ERREUR",
                    "Aucun port série sélectionné. Veuillez en choisir un.",
                );
                return;
            }

            self.config.save();
            let running = Arc::new(AtomicBool::new(true));
            self.running_flag = Arc::clone(&running);
            self.is_listening = true;

            let port_name = self.config.selected_port.clone();
            let baud = self.config.baud_rate;
            let tx = self.event_tx.clone();

            thread::spawn(move || {
                let _ = tx.send(AppEvent::Log(LogMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    level: LogLevel::Info,
                    tag: "CONNEXION".to_string(),
                    text: format!("Ouverture de {port_name} à {baud} bauds..."),
                }));

                while running.load(Ordering::SeqCst) {
                    match serialport::new(&port_name, baud)
                        .timeout(Duration::from_millis(50))
                        .open()
                    {
                        Ok(mut port) => {
                            // Activer explicitement les lignes de contrôle DTR et RTS.
                            let _ = port.write_data_terminal_ready(true);
                            let _ = port.write_request_to_send(true);

                            let _ = tx.send(AppEvent::Log(LogMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                level: LogLevel::Success,
                                tag: "CONNECTÉ".to_string(),
                                text: format!(
                                    "Prêt ! En écoute sur {port_name} (DTR/RTS actifs, timeout 50ms)."
                                ),
                            }));

                            let mut chunk = [0u8; 512];
                            let mut accumulator: Vec<u8> = Vec::new();
                            let mut last_rx_time = Instant::now();

                            while running.load(Ordering::SeqCst) {
                                match port.read(&mut chunk) {
                                    Ok(n) if n > 0 => {
                                        accumulator.extend_from_slice(&chunk[..n]);
                                        last_rx_time = Instant::now();

                                        let raw_display = String::from_utf8_lossy(&chunk[..n])
                                            .replace('\r', "\\r")
                                            .replace('\n', "\\n");

                                        let _ = tx.send(AppEvent::Log(LogMessage {
                                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                                            level: LogLevel::Raw,
                                            tag: "BRUT".to_string(),
                                            text: format!("{n} octet(s) reçus : \"{raw_display}\""),
                                        }));

                                        // Un chunk peut contenir plusieurs codes : on traite
                                        // chaque ligne complète, et on conserve le reliquat.
                                        for code in drain_delimited(&mut accumulator) {
                                            let _ = tx.send(AppEvent::Scan {
                                                code,
                                                at: Utc::now(),
                                            });
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                                        // Certains scanners n'envoient ni \r ni \n : on valide
                                        // après 100 ms de silence.
                                        if !accumulator.is_empty()
                                            && last_rx_time.elapsed() >= Duration::from_millis(100)
                                        {
                                            let code = String::from_utf8_lossy(&accumulator)
                                                .trim()
                                                .to_string();
                                            accumulator.clear();
                                            if !code.is_empty() {
                                                let _ = tx.send(AppEvent::Scan {
                                                    code,
                                                    at: Utc::now(),
                                                });
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx.send(AppEvent::Log(LogMessage {
                                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                                            level: LogLevel::Warning,
                                            tag: "PORT".to_string(),
                                            text: format!(
                                                "Liaison interrompue : {e}. Nouvelle tentative dans 2s..."
                                            ),
                                        }));
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Log(LogMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                level: LogLevel::Error,
                                tag: "ERREUR PORT".to_string(),
                                text: format!("Impossible d'ouvrir {port_name} : {e}"),
                            }));
                            thread::sleep(Duration::from_secs(2));
                        }
                    }
                }

                let _ = tx.send(AppEvent::Log(LogMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    level: LogLevel::Info,
                    tag: "SYSTÈME".to_string(),
                    text: "Écoute série terminée.".to_string(),
                }));
            });
        }
    }

    fn handle_scan(&mut self, code: String, at: DateTime<Utc>) {
        // Un scan redonne le focus au mouvement : si l'opérateur avait cliqué
        // dans un champ de configuration, le pavé numérique ne doit pas écrire
        // dans ce champ.
        self.focus_reset = true;

        // Le QR d'annulation est une commande, pas un mouvement de stock.
        if is_cancel_command(&code) {
            self.cancel_pending_input("QR code");
            return;
        }

        match parse_scan(&code) {
            Ok((action, sku, normalized)) => {
                let now = Instant::now();
                let result = self
                    .movement
                    .scan(action, sku.clone(), normalized.clone(), now, at);

                match result {
                    ScanResult::NeedsReview => {
                        self.add_log(
                            LogLevel::Warning,
                            "CORRECTION",
                            "Mouvement refusé : ajustez la quantité ou annulez avec Échap avant de scanner une autre référence.",
                        );
                        self.trigger_flash(Color32::from_rgb(251, 191, 36), "CORRIGEZ LA QUANTITÉ");
                        return;
                    }
                    ScanResult::CommittedThenStarted(previous) => {
                        self.commit_pending(previous);
                    }
                    ScanResult::Ignored => return,
                    ScanResult::Started | ScanResult::Incremented => {}
                }

                if self.catalog.lookup(&sku).is_none() {
                    self.add_log(
                        LogLevel::Warning,
                        "CATALOGUE",
                        &format!(
                            "Référence {sku} absente du catalogue local ; le mouvement sera quand même envoyé."
                        ),
                    );
                }

                let qty = self
                    .movement
                    .pending()
                    .map(|pending| pending.qty)
                    .unwrap_or(1);
                let name = self
                    .catalog
                    .lookup(&sku)
                    .map(|reference| reference.name.clone())
                    .unwrap_or_default();
                let suffix = if qty > 1 {
                    format!(" ×{qty}")
                } else {
                    String::new()
                };
                self.add_log(
                    LogLevel::Scan,
                    "SCAN",
                    &format!(
                        "{} {}{}",
                        action.label_fr(),
                        display_name(&sku, &name),
                        suffix
                    ),
                );
            }
            Err(error) => {
                self.add_log(
                    LogLevel::Error,
                    "FORMAT",
                    &format!("{error} Code reçu : {code}"),
                );
                self.trigger_flash(Color32::from_rgb(239, 68, 68), "CODE NON RECONNU");
            }
        }
    }

    fn cancel_pending_input(&mut self, source: &str) {
        match self.movement.cancel() {
            Some(pending) => {
                let name = self
                    .catalog
                    .lookup(&pending.sku)
                    .map(|reference| reference.name.clone())
                    .unwrap_or_default();
                self.add_log(
                    LogLevel::Info,
                    "ANNULÉ",
                    &format!(
                        "Saisie annulée ({source}) : {} {} ×{}.",
                        pending.action.label_fr(),
                        display_name(&pending.sku, &name),
                        pending.qty
                    ),
                );
                self.trigger_flash(Color32::from_rgb(100, 116, 139), "SAISIE ANNULÉE");
            }
            None => {
                self.add_log(
                    LogLevel::Info,
                    "ANNULÉ",
                    &format!("Aucune saisie en cours à annuler ({source})."),
                );
                self.trigger_flash(Color32::from_rgb(100, 116, 139), "AUCUNE SAISIE");
            }
        }
    }

    fn print_cancel_label(&mut self) {
        let path = data_path(CANCEL_LABEL_FILE);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let html = build_cancel_label_html(&self.config.device_id);
        match fs::write(&path, html) {
            Ok(()) => {
                self.add_log(
                    LogLevel::Info,
                    "ÉTIQUETTE",
                    &format!(
                        "Étiquette d'annulation générée (50 × 30 mm, imprimante thermique) : {}",
                        path.display()
                    ),
                );
                open_path(&path);
            }
            Err(error) => {
                self.add_log(
                    LogLevel::Error,
                    "ÉTIQUETTE",
                    &format!("Impossible d'écrire {} : {error}", path.display()),
                );
                self.trigger_flash(Color32::from_rgb(239, 68, 68), "IMPRESSION IMPOSSIBLE");
            }
        }
    }

    fn commit_pending(&mut self, pending: PendingMovement) {
        let name = self
            .catalog
            .lookup(&pending.sku)
            .map(|reference| reference.name.clone())
            .unwrap_or_default();
        let event_id = Uuid::new_v4().to_string();

        let event = QueuedEvent {
            event_id: event_id.clone(),
            seq: 0,
            sku: pending.sku.clone(),
            action: pending.action.as_str().to_string(),
            qty: pending.qty,
            station_id: self.config.device_id.clone(),
            scanned_at: pending.scanned_at.to_rfc3339(),
            raw: pending.raw.clone(),
            motif: None,
            cancels_event_id: None,
            status: EventStatus::Pending,
            attempts: 0,
            next_attempt_ms: 0,
            last_error: None,
            stock_after: None,
            message: None,
            rejected_code: None,
            rejected_available: None,
        };

        {
            let mut outbox = self
                .outbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            outbox.enqueue(event);
        }

        self.stats.total_movements += 1;
        self.last_commit = Some(LastCommit {
            event_id,
            sku: pending.sku.clone(),
            name: name.clone(),
            action: pending.action,
            qty: pending.qty,
            status: CommitStatus::Queued,
            message: "En attente d'envoi à n8n".to_string(),
            stock_after: None,
            at: Instant::now(),
        });

        self.add_log(
            LogLevel::Info,
            "MOUVEMENT",
            &format!(
                "{} {} ×{} enregistré localement (en attente d'envoi).",
                pending.action.label_fr(),
                display_name(&pending.sku, &name),
                pending.qty
            ),
        );
    }

    fn handle_outbox_update(&mut self, update: OutboxUpdate) {
        let action = Action::from_code(&update.action);
        let name = self
            .catalog
            .lookup(&update.sku)
            .map(|reference| reference.name.clone())
            .unwrap_or_default();
        let action_label = action.map(Action::label_fr).unwrap_or("MOUVEMENT");

        match update.status {
            EventStatus::Sent => {
                self.stats.sent_success += 1;

                let is_cancel = update.motif.as_deref() == Some("Annulation");
                if let Some(last) = &mut self.last_commit {
                    if last.event_id == update.event_id {
                        last.status = if is_cancel {
                            CommitStatus::Cancelled
                        } else {
                            CommitStatus::Sent
                        };
                        last.stock_after = update.stock_after;
                        last.message = update
                            .message
                            .clone()
                            .unwrap_or_else(|| "Validé par n8n".to_string());
                    }
                }

                let stock_text = update
                    .stock_after
                    .map(|stock| format!(" · stock Notion {stock}"))
                    .unwrap_or_default();
                let message = update.message.clone().unwrap_or_else(|| {
                    if is_cancel {
                        "Annulation validée".to_string()
                    } else {
                        "Mouvement validé".to_string()
                    }
                });

                if is_cancel {
                    self.add_log(
                        LogLevel::Info,
                        "ANNULÉ",
                        &format!(
                            "{} {} ×{}{} — {}",
                            action_label,
                            display_name(&update.sku, &name),
                            update.qty,
                            stock_text,
                            message
                        ),
                    );
                    self.trigger_flash(Color32::from_rgb(100, 116, 139), "ANNULÉ");
                } else {
                    self.add_log(
                        LogLevel::Success,
                        "VALIDÉ",
                        &format!(
                            "{} {} ×{}{} — {}",
                            action_label,
                            display_name(&update.sku, &name),
                            update.qty,
                            stock_text,
                            message
                        ),
                    );
                    self.trigger_flash(Color32::from_rgb(16, 185, 129), "VALIDÉ");
                }
            }
            EventStatus::Rejected => {
                self.stats.sent_failed += 1;
                let message = update
                    .message
                    .clone()
                    .or_else(|| update.code.clone())
                    .unwrap_or_else(|| "Refusé par n8n".to_string());

                if let Some(last) = &mut self.last_commit {
                    if last.event_id == update.event_id {
                        last.status = CommitStatus::Rejected;
                        last.message = message.clone();
                    }
                }

                let available = update
                    .available
                    .map(|available| format!(" ({available} disponible(s))"))
                    .unwrap_or_default();
                self.add_log(
                    LogLevel::Error,
                    "REFUSÉ",
                    &format!(
                        "{} {} ×{}{} — {}",
                        action_label,
                        display_name(&update.sku, &name),
                        update.qty,
                        available,
                        message
                    ),
                );
                self.trigger_flash(
                    Color32::from_rgb(239, 68, 68),
                    &format!("REFUSÉ · {message}"),
                );

                // Stock insuffisant : on réouvre la saisie avec la quantité
                // refusée pour que l'opérateur puisse corriger sans rescanner.
                if update.code.as_deref() == Some("INSUFFICIENT_STOCK")
                    && self.movement.pending().is_none()
                {
                    if let Some(action) = action {
                        self.movement.restore(
                            action,
                            update.sku.clone(),
                            update.qty,
                            format!("{}|{}", action.as_str(), update.sku),
                            Utc::now(),
                        );
                        self.add_log(
                            LogLevel::Warning,
                            "CORRECTION",
                            "Quantité réouverte : ajustez puis validez avec Entrée.",
                        );
                    }
                }
            }
            EventStatus::Pending | EventStatus::Sending => {}
        }
    }

    fn undo_last(&mut self) {
        let Some(last) = self.last_commit.clone() else {
            return;
        };

        if last.status == CommitStatus::Queued {
            let removed = {
                let mut outbox = self
                    .outbox
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                outbox.remove_pending(&last.event_id)
            };

            if removed {
                self.stats.total_movements = self.stats.total_movements.saturating_sub(1);
                self.last_commit = Some(LastCommit {
                    status: CommitStatus::Cancelled,
                    message: "Annulé avant envoi".to_string(),
                    ..last
                });
                self.add_log(
                    LogLevel::Info,
                    "ANNULÉ",
                    "Mouvement annulé localement, il n'a pas été envoyé.",
                );
                self.trigger_flash(Color32::from_rgb(100, 116, 139), "ANNULÉ");
            } else {
                self.add_log(
                    LogLevel::Warning,
                    "ANNULÉ",
                    "Déjà en cours d'envoi : annulation locale impossible.",
                );
            }
            return;
        }

        if last.status != CommitStatus::Sent {
            return;
        }

        let event_id = Uuid::new_v4().to_string();
        let action = last.action.opposite();
        let event = QueuedEvent {
            event_id: event_id.clone(),
            seq: 0,
            sku: last.sku.clone(),
            action: action.as_str().to_string(),
            qty: last.qty,
            station_id: self.config.device_id.clone(),
            scanned_at: Utc::now().to_rfc3339(),
            raw: format!("{}|{}", action.as_str(), last.sku),
            motif: Some("Annulation".to_string()),
            cancels_event_id: Some(last.event_id.clone()),
            status: EventStatus::Pending,
            attempts: 0,
            next_attempt_ms: 0,
            last_error: None,
            stock_after: None,
            message: None,
            rejected_code: None,
            rejected_available: None,
        };

        {
            let mut outbox = self
                .outbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            outbox.enqueue(event);
        }

        self.last_commit = Some(LastCommit {
            event_id,
            sku: last.sku.clone(),
            name: last.name.clone(),
            action,
            qty: last.qty,
            status: CommitStatus::Queued,
            message: "Annulation en attente".to_string(),
            stock_after: last.stock_after,
            at: Instant::now(),
        });
        self.add_log(
            LogLevel::Info,
            "ANNULATION",
            &format!(
                "Annulation envoyée pour {} ×{}.",
                display_name(&last.sku, &last.name),
                last.qty
            ),
        );
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }

        let events = ctx.input(|input| input.events.clone());
        let now = Instant::now();

        for event in events {
            match event {
                egui::Event::Text(text) => {
                    for character in text.chars() {
                        if let Some(digit) = character.to_digit(10) {
                            self.movement.digit(digit, now);
                        } else if character == '+' {
                            self.movement.adjust(1, now);
                        } else if character == '-' {
                            self.movement.adjust(-1, now);
                        }
                    }
                }
                egui::Event::Key {
                    key, pressed: true, ..
                } => match key {
                    egui::Key::Enter => {
                        if self
                            .movement
                            .pending()
                            .is_some_and(|pending| pending.qty == 0)
                        {
                            self.add_log(
                                LogLevel::Warning,
                                "QUANTITÉ",
                                "Quantité nulle : saisissez au moins 1.",
                            );
                        } else if let Some(pending) = self.movement.take() {
                            self.commit_pending(pending);
                        }
                    }
                    egui::Key::Escape => {
                        self.cancel_pending_input("Échap");
                    }
                    egui::Key::Backspace => self.movement.backspace(now),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn trigger_flash(&mut self, color: Color32, text: &str) {
        self.flash = Some(Flash {
            color,
            text: text.to_string(),
            until: Instant::now() + Duration::from_millis(1_200),
        });
    }
}

fn display_name(sku: &str, name: &str) -> String {
    if name.is_empty() {
        sku.to_string()
    } else {
        format!("{name} ({sku})")
    }
}

/// Extrait toutes les lignes complètes du buffer et conserve le reliquat.
fn drain_delimited(accumulator: &mut Vec<u8>) -> Vec<String> {
    let mut codes = Vec::new();

    while let Some(position) = accumulator
        .iter()
        .position(|&byte| byte == b'\n' || byte == b'\r')
    {
        let line: Vec<u8> = accumulator.drain(..=position).collect();
        let text = String::from_utf8_lossy(&line).trim().to_string();
        if !text.is_empty() {
            codes.push(text);
        }

        // Absorbe le second caractère d'un éventuel \r\n.
        while matches!(accumulator.first(), Some(b'\n') | Some(b'\r')) {
            accumulator.remove(0);
        }
    }

    codes
}

fn backoff_ms(attempts: u32) -> u64 {
    match attempts {
        0 => 500,
        1 => 1_000,
        2 => 2_000,
        3 => 4_000,
        4 => 8_000,
        5 => 15_000,
        6 => 30_000,
        _ => 60_000,
    }
}

fn spawn_sender(
    outbox: Arc<Mutex<Outbox>>,
    running: Arc<AtomicBool>,
    webhook_url: Arc<RwLock<String>>,
    api_key: Arc<RwLock<String>>,
    tx: Sender<AppEvent>,
) {
    thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                let _ = tx.send(AppEvent::Log(LogMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    level: LogLevel::Error,
                    tag: "HTTP".to_string(),
                    text: format!("Impossible de créer le client HTTP : {error}"),
                }));
                return;
            }
        };

        while running.load(Ordering::Relaxed) {
            let url = webhook_url
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .trim()
                .to_string();

            if url.is_empty() {
                thread::sleep(Duration::from_millis(500));
                continue;
            }

            let event = {
                let mut outbox = outbox
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                outbox.next_ready(outbox::now_ms())
            };

            let Some(event) = event else {
                thread::sleep(Duration::from_millis(200));
                continue;
            };

            let key = api_key
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();

            let payload = WebhookPayload {
                barcode: &event.raw,
                timestamp: &event.scanned_at,
                device_id: &event.station_id,
                event_id: &event.event_id,
                seq: event.seq,
                sku: &event.sku,
                action: &event.action,
                qty: event.qty,
                station_id: &event.station_id,
                scanned_at: &event.scanned_at,
                raw: &event.raw,
                app_version: env!("CARGO_PKG_VERSION"),
                motif: event.motif.as_deref(),
                cancels_event_id: event.cancels_event_id.as_deref(),
                is_test: None,
            };

            let mut request = client
                .post(&url)
                .header("Idempotency-Key", &event.event_id)
                .json(&payload);
            if !key.is_empty() {
                request = request.header("X-Scanner-Key", &key);
            }

            if event.attempts == 0 {
                let _ = tx.send(AppEvent::Log(LogMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    level: LogLevel::Info,
                    tag: "ENVOI".to_string(),
                    text: format!("{} {} ×{} → n8n", event.action, event.sku, event.qty),
                }));
            }

            match request.send() {
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    let parsed: Option<WebhookResponse> = serde_json::from_str(&body).ok();
                    let business_ok = parsed
                        .as_ref()
                        .and_then(|response| response.ok)
                        .unwrap_or(status.is_success());

                    if status.is_success() && business_ok {
                        let stock_after = parsed.as_ref().and_then(|response| response.stock_after);
                        let message = parsed
                            .as_ref()
                            .and_then(|response| response.message.clone());
                        {
                            let mut outbox = outbox
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            outbox.mark_sent(&event.event_id, stock_after, message.clone());
                        }
                        let _ = tx.send(AppEvent::OutboxUpdate(OutboxUpdate {
                            event_id: event.event_id.clone(),
                            sku: event.sku.clone(),
                            action: event.action.clone(),
                            qty: event.qty,
                            motif: event.motif.clone(),
                            status: EventStatus::Sent,
                            stock_after,
                            message,
                            code: None,
                            available: None,
                        }));
                    } else {
                        let code = parsed
                            .as_ref()
                            .and_then(|response| response.code.clone())
                            .or_else(|| Some(format!("HTTP {}", status.as_u16())));
                        let message = parsed
                            .as_ref()
                            .and_then(|response| response.message.clone())
                            .or_else(|| {
                                let trimmed = body.trim();
                                if trimmed.is_empty() {
                                    Some(format!("HTTP {}", status.as_u16()))
                                } else {
                                    Some(trimmed.chars().take(300).collect())
                                }
                            });
                        let available = parsed.as_ref().and_then(|response| response.available);
                        let status_code = status.as_u16();
                        // 401/403/404 sont souvent des erreurs de
                        // configuration (clé API, URL, workflow inactif) :
                        // on conserve l'événement et on réessaie, plutôt que
                        // de perdre un mouvement de stock.
                        let retryable = status.is_server_error()
                            || matches!(status_code, 401 | 403 | 404 | 408 | 429)
                            || (status.is_success()
                                && !business_ok
                                && code.as_deref() == Some("TEMPORARY"));

                        if retryable {
                            let delay = match status_code {
                                429 => 10_000,
                                401 | 403 | 404 => 30_000,
                                _ => backoff_ms(event.attempts),
                            };
                            {
                                let mut outbox = outbox
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                outbox.mark_retry(
                                    &event.event_id,
                                    message.clone().unwrap_or_default(),
                                    delay,
                                );
                            }
                            let _ = tx.send(AppEvent::Log(LogMessage {
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                level: LogLevel::Warning,
                                tag: "RETRY".to_string(),
                                text: format!(
                                    "Nouvelle tentative dans {:.1}s : {}",
                                    delay as f64 / 1000.0,
                                    message.unwrap_or_default()
                                ),
                            }));
                        } else {
                            {
                                let mut outbox = outbox
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                outbox.mark_rejected(
                                    &event.event_id,
                                    code.clone(),
                                    message.clone(),
                                    available,
                                );
                            }
                            let _ = tx.send(AppEvent::OutboxUpdate(OutboxUpdate {
                                event_id: event.event_id.clone(),
                                sku: event.sku.clone(),
                                action: event.action.clone(),
                                qty: event.qty,
                                motif: event.motif.clone(),
                                status: EventStatus::Rejected,
                                stock_after: None,
                                message,
                                code,
                                available,
                            }));
                        }
                    }
                }
                Err(error) => {
                    let delay = backoff_ms(event.attempts);
                    {
                        let mut outbox = outbox
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        outbox.mark_retry(&event.event_id, error.to_string(), delay);
                    }
                    let _ = tx.send(AppEvent::Log(LogMessage {
                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                        level: LogLevel::Warning,
                        tag: "RETRY".to_string(),
                        text: format!(
                            "Réseau indisponible ({error}), nouvelle tentative dans {:.1}s.",
                            delay as f64 / 1000.0
                        ),
                    }));
                }
            }
        }
    });
}

// ==========================================
// Rendu de l'Interface Graphique (egui)
// ==========================================

fn action_color(action: Action) -> Color32 {
    match action {
        Action::In => Color32::from_rgb(16, 185, 129),
        Action::Out => Color32::from_rgb(239, 68, 68),
    }
}

fn commit_status_color(status: CommitStatus) -> Color32 {
    match status {
        CommitStatus::Queued => Color32::from_rgb(251, 191, 36),
        CommitStatus::Sent => Color32::from_rgb(16, 185, 129),
        CommitStatus::Rejected => Color32::from_rgb(239, 68, 68),
        CommitStatus::Cancelled => Color32::from_rgb(148, 163, 184),
    }
}

fn commit_status_label(status: CommitStatus) -> &'static str {
    match status {
        CommitStatus::Queued => "EN ATTENTE",
        CommitStatus::Sent => "VALIDÉ",
        CommitStatus::Rejected => "REFUSÉ",
        CommitStatus::Cancelled => "ANNULÉ",
    }
}

fn action_badge(ui: &mut egui::Ui, action: Action) {
    let (background, foreground) = match action {
        Action::In => (
            Color32::from_rgb(16, 48, 36),
            Color32::from_rgb(52, 211, 153),
        ),
        Action::Out => (
            Color32::from_rgb(60, 20, 26),
            Color32::from_rgb(248, 113, 113),
        ),
    };
    egui::Frame::default()
        .fill(background)
        .rounding(Rounding::same(5.0))
        .inner_margin(Margin {
            left: 7.0,
            right: 7.0,
            top: 2.0,
            bottom: 2.0,
        })
        .show(ui, |ui| {
            ui.label(
                RichText::new(action.label_fr())
                    .size(10.0)
                    .strong()
                    .color(foreground),
            );
        });
}

fn kpi_card(
    ui: &mut egui::Ui,
    theme: ThemeColors,
    label: &str,
    value: String,
    value_color: Color32,
) {
    egui::Frame::default()
        .fill(theme.bg_card)
        .stroke(Stroke::new(1.0_f32, theme.border_card))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::same(10.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(label)
                    .size(10.0)
                    .color(theme.text_muted)
                    .strong(),
            );
            ui.label(RichText::new(value).size(22.0).strong().color(value_color));
        });
}

impl eframe::App for ScannerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Suivre l'apparence du système : variante sombre ou claire de l'icône
        if let Some(theme) = ctx.system_theme() {
            let icon_dark = theme == egui::Theme::Dark;
            if self.icon_dark != Some(icon_dark) {
                apply_app_icon(ctx, icon_dark);
                self.icon_dark = Some(icon_dark);
            }
        }

        // Maintenir le thème cohérent et synchronisé (résolu : System → dark/light)
        let is_dark = ctx.theme() == egui::Theme::Dark;
        let theme = ThemeColors::new(is_dark);

        // Dépouillement des événements en arrière-plan
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::Log(message) => self.push_log(message),
                AppEvent::Scan { code, at } => self.handle_scan(code, at),
                AppEvent::OutboxUpdate(update) => self.handle_outbox_update(update),
            }
        }

        // Un scan vient d'arriver : relâcher le focus d'un éventuel champ de
        // configuration pour que les chiffres aillent au mouvement.
        if self.focus_reset {
            self.focus_reset = false;
            if let Some(focused) = ctx.memory(|memory| memory.focused()) {
                ctx.memory_mut(|memory| memory.surrender_focus(focused));
            }
        }

        // Clavier : chiffres, +/-, Entrée, Échap, Retour arrière.
        self.handle_keyboard(ctx);

        // Validation automatique après inactivité.
        let now = Instant::now();
        if let Some(pending) = self.movement.tick(now) {
            self.commit_pending(pending);
        }

        if self.flash.as_ref().is_some_and(|flash| flash.until <= now) {
            self.flash = None;
        }

        // Démarrage automatique de la capture, une seule fois par lancement.
        if !self.boot_started {
            self.boot_started = true;
            if self.config.auto_start && !self.is_listening && !self.config.selected_port.is_empty()
            {
                self.toggle_listening();
            }
        }

        let counts = self
            .outbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .counts();
        let pending = self.movement.pending().cloned();
        let last = self.last_commit.clone();
        let auto_commit = self.auto_commit;
        let (pending_name, pending_description) = if let Some(pending) = &pending {
            match self.catalog.lookup(&pending.sku) {
                Some(reference) => (reference.name.clone(), reference.description.clone()),
                None => (String::new(), String::new()),
            }
        } else {
            (String::new(), String::new())
        };

        let hero_border = if let Some(pending) = &pending {
            action_color(pending.action)
        } else if let Some(last) = &last {
            commit_status_color(last.status)
        } else {
            theme.border_card
        };

        let mut validate = false;
        let mut cancel = false;
        let mut minus = false;
        let mut plus = false;
        let mut undo = false;
        let mut print_label = false;
        let mut copied = false;

        // On force le fond du panneau central à la couleur exacte du thème choisi
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme.bg_panel).inner_margin(Margin::same(16.0)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                // ----------------------------------------------------
                // 1. Header & Statut
                // ----------------------------------------------------
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading(
                            RichText::new("⚡ Scanner Pièces · n8n")
                                .color(theme.text_primary)
                                .strong()
                                .size(20.0),
                        );
                        ui.label(
                            RichText::new("Entrées / sorties de stock — file locale et webhook n8n")
                                .color(theme.text_muted)
                                .size(12.0),
                        );
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (badge_bg, badge_stroke, badge_text, badge_color) = if self.is_listening {
                            (
                                Color32::from_rgb(16, 48, 36),
                                Color32::from_rgb(16, 185, 129),
                                "● ÉCOUTE ACTIVE",
                                Color32::from_rgb(52, 211, 153),
                            )
                        } else if theme.is_dark {
                            (
                                Color32::from_rgb(30, 35, 48),
                                Color32::from_rgb(71, 85, 105),
                                "○ INACTIF",
                                Color32::from_rgb(148, 163, 184),
                            )
                        } else {
                            (
                                Color32::from_rgb(241, 245, 249),
                                Color32::from_rgb(203, 213, 225),
                                "○ INACTIF",
                                Color32::from_rgb(100, 116, 139),
                            )
                        };

                        egui::Frame::default()
                            .fill(badge_bg)
                            .stroke(Stroke::new(1.0_f32, badge_stroke))
                            .rounding(Rounding::same(12.0))
                            .inner_margin(Margin {
                                left: 12.0,
                                right: 12.0,
                                top: 5.0,
                                bottom: 5.0,
                            })
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(badge_text)
                                        .color(badge_color)
                                        .strong()
                                        .size(12.0),
                                );
                            });

                        let (theme_icon, theme_label) = self.config.theme_mode.icon_and_label();
                        if ui
                            .button(
                                RichText::new(format!("{theme_icon} {theme_label}"))
                                    .size(12.0)
                                    .color(theme.text_primary),
                            )
                            .on_hover_text(self.config.theme_mode.hover_text())
                            .clicked()
                        {
                            self.config.theme_mode = self.config.theme_mode.next();
                            self.config.save();
                            Self::apply_theme(ctx, self.config.theme_mode);
                        }
                    });
                });

                ui.add_space(10.0);

                // ----------------------------------------------------
                // 2. Cartes de Métriques / KPIs
                // ----------------------------------------------------
                ui.columns(4, |cols| {
                    kpi_card(
                        &mut cols[0],
                        theme,
                        "MOUVEMENTS",
                        self.stats.total_movements.to_string(),
                        theme.text_primary,
                    );
                    kpi_card(
                        &mut cols[1],
                        theme,
                        "VALIDÉS",
                        self.stats.sent_success.to_string(),
                        Color32::from_rgb(16, 185, 129),
                    );
                    kpi_card(
                        &mut cols[2],
                        theme,
                        "EN ATTENTE",
                        counts.pending.to_string(),
                        if counts.pending > 0 {
                            Color32::from_rgb(251, 191, 36)
                        } else {
                            theme.text_muted
                        },
                    );
                    kpi_card(
                        &mut cols[3],
                        theme,
                        "REFUSÉS",
                        counts.rejected.to_string(),
                        if counts.rejected > 0 {
                            Color32::from_rgb(239, 68, 68)
                        } else {
                            theme.text_muted
                        },
                    );
                });

                ui.add_space(10.0);

                // ----------------------------------------------------
                // 3. Mouvement en cours / dernier mouvement
                // ----------------------------------------------------
                egui::Frame::default()
                    .fill(theme.bg_card)
                    .stroke(Stroke::new(1.5_f32, hero_border))
                    .rounding(Rounding::same(10.0))
                    .inner_margin(Margin::same(14.0))
                    .show(ui, |ui| {
                        if let Some(pending) = &pending {
                            let action_color = action_color(pending.action);
                            ui.horizontal(|ui| {
                                action_badge(ui, pending.action);
                                ui.label(
                                    RichText::new(display_name(&pending.sku, &pending_name))
                                        .size(17.0)
                                        .strong()
                                        .color(theme.text_primary),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "à {}",
                                            pending
                                                .scanned_at
                                                .with_timezone(&Local)
                                                .format("%H:%M:%S")
                                        ))
                                        .size(11.0)
                                        .color(theme.text_muted),
                                    );
                                    if ui
                                        .small_button("📋")
                                        .on_hover_text("Copier la référence")
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(pending.sku.clone());
                                        copied = true;
                                    }
                                });
                            });

                            if !pending_description.is_empty()
                                && pending_description != pending_name
                            {
                                ui.label(
                                    RichText::new(&pending_description)
                                        .size(10.5)
                                        .color(theme.text_muted),
                                );
                                ui.add_space(2.0);
                            }

                            ui.add_space(8.0);

                            ui.horizontal(|ui| {
                                if ui
                                    .add_sized(
                                        [42.0, 42.0],
                                        egui::Button::new(
                                            RichText::new("−").size(24.0).strong(),
                                        ),
                                    )
                                    .clicked()
                                {
                                    minus = true;
                                }

                                ui.label(
                                    RichText::new(pending.qty.to_string())
                                        .size(38.0)
                                        .strong()
                                        .monospace()
                                        .color(action_color),
                                );

                                if ui
                                    .add_sized(
                                        [42.0, 42.0],
                                        egui::Button::new(
                                            RichText::new("+").size(24.0).strong(),
                                        ),
                                    )
                                    .clicked()
                                {
                                    plus = true;
                                }

                                ui.add_space(12.0);

                                ui.vertical(|ui| {
                                    if pending.needs_review {
                                        ui.label(
                                            RichText::new(
                                                "⚠ Refusé : ajuste la quantité puis valide (auto désactivée)",
                                            )
                                            .size(10.0)
                                            .color(Color32::from_rgb(239, 68, 68)),
                                        );
                                    } else {
                                        let remaining = auto_commit
                                            .saturating_sub(pending.last_input.elapsed());
                                        let fraction = remaining.as_secs_f32()
                                            / auto_commit.as_secs_f32().max(0.001);
                                        ui.add(
                                            egui::ProgressBar::new(fraction)
                                                .desired_width(190.0)
                                                .desired_height(8.0)
                                                .fill(theme.accent)
                                                .text(
                                                    RichText::new(format!(
                                                        "auto {:.1}s",
                                                        remaining.as_secs_f32()
                                                    ))
                                                    .size(9.0),
                                                ),
                                        );
                                    }
                                });
                            });

                            ui.add_space(8.0);

                            ui.horizontal(|ui| {
                                let validate_button = egui::Button::new(
                                    RichText::new("✓ Valider (Entrée)")
                                        .strong()
                                        .size(12.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(16, 185, 129));
                                if ui
                                    .add_sized([190.0, 32.0], validate_button)
                                    .clicked()
                                {
                                    validate = true;
                                }
                                if ui
                                    .add_sized(
                                        [190.0, 32.0],
                                        egui::Button::new(
                                            RichText::new("✕ Annuler la saisie (Échap)")
                                                .strong()
                                                .size(12.0)
                                                .color(Color32::from_rgb(248, 113, 113)),
                                        )
                                        .fill(if theme.is_dark {
                                            Color32::from_rgb(48, 18, 22)
                                        } else {
                                            Color32::from_rgb(254, 242, 242)
                                        }),
                                    )
                                    .on_hover_text(
                                        "Supprime la saisie affichée sans l'envoyer à n8n. \
                                         Vous pouvez aussi scanner le QR d'annulation ci-dessous.",
                                    )
                                    .clicked()
                                {
                                    cancel = true;
                                }
                                ui.with_layout(
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(
                                                "Le prochain scan valide automatiquement.",
                                            )
                                            .size(10.0)
                                            .color(theme.text_muted),
                                        );
                                    },
                                );
                            });
                        } else if let Some(last) = &last {
                            let status_color = commit_status_color(last.status);
                            ui.horizontal(|ui| {
                                action_badge(ui, last.action);
                                ui.label(
                                    RichText::new(display_name(&last.sku, &last.name))
                                        .size(16.0)
                                        .strong()
                                        .color(theme.text_primary),
                                );
                                ui.label(
                                    RichText::new(format!("×{}", last.qty))
                                        .size(18.0)
                                        .strong()
                                        .color(action_color(last.action)),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(commit_status_label(last.status))
                                            .size(11.0)
                                            .strong()
                                            .color(status_color),
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "il y a {:.0}s",
                                            last.at.elapsed().as_secs_f32()
                                        ))
                                        .size(10.0)
                                        .color(theme.text_muted),
                                    );
                                });
                            });

                            ui.add_space(4.0);

                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&last.message)
                                        .size(11.0)
                                        .color(status_color),
                                );
                                if let Some(stock) = last.stock_after {
                                    ui.label(
                                        RichText::new(format!("Stock Notion : {stock}"))
                                            .size(11.0)
                                            .color(theme.text_muted),
                                    );
                                }
                                if matches!(
                                    last.status,
                                    CommitStatus::Queued | CommitStatus::Sent
                                ) && last.at.elapsed() < Duration::from_secs(10)
                                {
                                    if ui
                                        .button(
                                            RichText::new("↩ Annuler")
                                                .size(11.0)
                                                .color(theme.text_muted),
                                        )
                                        .on_hover_text("Fenêtre d'annulation de 10 secondes")
                                        .clicked()
                                    {
                                        undo = true;
                                    }
                                }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("📦 Aucun mouvement en cours")
                                        .size(12.0)
                                        .color(theme.text_muted)
                                        .strong(),
                                );
                            });
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(
                                    "Scannez IN|DCO001 pour une entrée, OUT|DCO001 pour une sortie. \
                                     La quantité se règle au pavé numérique.",
                                )
                                .size(12.0)
                                .italics()
                                .color(theme.text_muted),
                            );
                        }

                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(8.0);

                        ui.horizontal(|ui| {
                            draw_cancel_qr(ui, 104.0);
                            ui.add_space(6.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("QR d'annulation")
                                        .strong()
                                        .size(12.0)
                                        .color(theme.text_primary),
                                );
                                ui.label(
                                    RichText::new(
                                        "Scanné par l'opérateur, ce code annule la saisie affichée \
                                         exactement comme le bouton : rien n'est envoyé à n8n.",
                                    )
                                    .size(10.5)
                                    .color(theme.text_muted),
                                );
                                ui.add_space(4.0);
                                if ui
                                    .button(
                                        RichText::new("🖨 Imprimer l'étiquette (50 × 30 mm)")
                                            .size(11.5)
                                            .color(theme.accent),
                                    )
                                    .on_hover_text(
                                        "Génère une étiquette 50 × 30 mm pour imprimante thermique, \
                                         puis ouvre la boîte d'impression.",
                                    )
                                    .clicked()
                                {
                                    print_label = true;
                                }
                            });
                        });
                    });

                ui.add_space(10.0);

                // ----------------------------------------------------
                // 4. Configuration n8n & Matériel (Deux cartes côte à côte)
                // ----------------------------------------------------
                ui.columns(2, |cols| {
                    // CARTE GAUCHE : Configuration n8n
                    egui::Frame::default()
                        .fill(theme.bg_card)
                        .stroke(Stroke::new(1.0_f32, theme.border_card))
                        .rounding(Rounding::same(10.0))
                        .inner_margin(Margin::same(12.0))
                        .show(&mut cols[0], |ui| {
                            ui.label(
                                RichText::new("🌐 Webhook n8n")
                                    .strong()
                                    .size(13.0)
                                    .color(theme.text_primary),
                            );
                            ui.add_space(6.0);

                            ui.label(
                                RichText::new("URL du Webhook :")
                                    .size(11.0)
                                    .color(theme.text_muted),
                            );
                            let available_width = ui.available_width();
                            let url_edit = ui.add(
                                egui::TextEdit::singleline(&mut self.config.webhook_url)
                                    .hint_text("https://n8n.mondomaine.com/webhook/...")
                                    .desired_width(available_width),
                            );
                            if url_edit.changed() || url_edit.lost_focus() {
                                self.config.save();
                                if let Ok(mut shared) = self.webhook_url_shared.write() {
                                    *shared = self.config.webhook_url.clone();
                                }
                            }

                            if self.config.webhook_url.contains("/webhook-test/") {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new("⚠️ Mode Test")
                                            .size(10.5)
                                            .color(Color32::from_rgb(251, 191, 36)),
                                    );
                                    if ui
                                        .button(
                                            RichText::new("Passer en Production")
                                                .size(10.5)
                                                .color(Color32::from_rgb(52, 211, 153)),
                                        )
                                        .on_hover_text(
                                            "Remplace /webhook-test/ par /webhook/ pour le workflow publié",
                                        )
                                        .clicked()
                                    {
                                        self.config.webhook_url = self
                                            .config
                                            .webhook_url
                                            .replace("/webhook-test/", "/webhook/");
                                        self.config.save();
                                        if let Ok(mut shared) = self.webhook_url_shared.write() {
                                            *shared = self.config.webhook_url.clone();
                                        }
                                    }
                                });
                            } else if self.config.webhook_url.contains("/webhook/") {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new("✓ Mode Production")
                                            .size(10.5)
                                            .color(Color32::from_rgb(52, 211, 153)),
                                    );
                                    if ui
                                        .button(
                                            RichText::new("Passer en Test")
                                                .size(10.5)
                                                .color(Color32::from_rgb(148, 163, 184)),
                                        )
                                        .clicked()
                                    {
                                        self.config.webhook_url = self
                                            .config
                                            .webhook_url
                                            .replace("/webhook/", "/webhook-test/");
                                        self.config.save();
                                        if let Ok(mut shared) = self.webhook_url_shared.write() {
                                            *shared = self.config.webhook_url.clone();
                                        }
                                    }
                                });
                            }

                            ui.add_space(4.0);

                            ui.label(
                                RichText::new("Poste / Device ID :")
                                    .size(11.0)
                                    .color(theme.text_muted),
                            );
                            let device_edit = ui.add(
                                egui::TextEdit::singleline(&mut self.config.device_id)
                                    .hint_text("POSTE-01")
                                    .desired_width(available_width),
                            );
                            if device_edit.changed() || device_edit.lost_focus() {
                                self.config.save();
                            }

                            ui.add_space(4.0);

                            ui.label(
                                RichText::new("Clé API (header X-Scanner-Key) :")
                                    .size(11.0)
                                    .color(theme.text_muted),
                            );
                            let key_edit = ui.add(
                                egui::TextEdit::singleline(&mut self.config.api_key)
                                    .hint_text("optionnel")
                                    .password(true)
                                    .desired_width(available_width),
                            );
                            if key_edit.changed() || key_edit.lost_focus() {
                                self.config.save();
                                if let Ok(mut shared) = self.api_key_shared.write() {
                                    *shared = self.config.api_key.clone();
                                }
                            }

                            ui.add_space(8.0);

                            if ui
                                .button(
                                    RichText::new("⚡ Tester le Webhook")
                                        .size(12.0)
                                        .color(theme.accent),
                                )
                                .clicked()
                            {
                                self.test_webhook();
                            }
                        });

                    // CARTE DROITE : Port Série / Bluetooth
                    egui::Frame::default()
                        .fill(theme.bg_card)
                        .stroke(Stroke::new(1.0_f32, theme.border_card))
                        .rounding(Rounding::same(10.0))
                        .inner_margin(Margin::same(12.0))
                        .show(&mut cols[1], |ui| {
                            ui.label(
                                RichText::new("🔌 Connexion Série / Bluetooth")
                                    .strong()
                                    .size(13.0)
                                    .color(theme.text_primary),
                            );
                            ui.add_space(6.0);

                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Port :")
                                        .size(11.0)
                                        .color(theme.text_muted),
                                );
                                let current_port = if self.config.selected_port.is_empty() {
                                    "Aucun port".to_string()
                                } else {
                                    self.config.selected_port.clone()
                                };

                                egui::ComboBox::from_id_salt("port_select")
                                    .selected_text(&current_port)
                                    .width(120.0)
                                    .show_ui(ui, |ui| {
                                        for port in &self.available_ports {
                                            if ui
                                                .selectable_value(
                                                    &mut self.config.selected_port,
                                                    port.clone(),
                                                    port,
                                                )
                                                .clicked()
                                            {
                                                self.config.save();
                                            }
                                        }
                                    });

                                if ui
                                    .button("🔄")
                                    .on_hover_text("Actualiser les ports")
                                    .clicked()
                                {
                                    self.refresh_ports();
                                }
                            });

                            ui.add_space(4.0);

                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Vitesse :")
                                        .size(11.0)
                                        .color(theme.text_muted),
                                );
                                egui::ComboBox::from_id_salt("baud_select")
                                    .selected_text(format!("{} bauds", self.config.baud_rate))
                                    .width(120.0)
                                    .show_ui(ui, |ui| {
                                        for &rate in &BAUD_RATES {
                                            if ui
                                                .selectable_value(
                                                    &mut self.config.baud_rate,
                                                    rate,
                                                    format!("{rate} bauds"),
                                                )
                                                .clicked()
                                            {
                                                self.config.save();
                                            }
                                        }
                                    });
                            });

                            ui.add_space(4.0);

                            if ui
                                .checkbox(
                                    &mut self.config.auto_start,
                                    RichText::new("Démarrer la capture au lancement")
                                        .size(11.0)
                                        .color(theme.text_muted),
                                )
                                .changed()
                            {
                                self.config.save();
                            }

                            ui.add_space(8.0);

                            let (btn_fill, btn_text, text_color) = if self.is_listening {
                                (
                                    Color32::from_rgb(225, 29, 72),
                                    "⏹  Arrêter la capture",
                                    Color32::from_rgb(255, 255, 255),
                                )
                            } else {
                                (
                                    Color32::from_rgb(79, 70, 229),
                                    "▶  Démarrer la capture",
                                    Color32::from_rgb(255, 255, 255),
                                )
                            };

                            let button = egui::Button::new(
                                RichText::new(btn_text)
                                    .strong()
                                    .size(13.0)
                                    .color(text_color),
                            )
                            .fill(btn_fill);

                            let button_width = ui.available_width();
                            if ui.add_sized([button_width, 32.0], button).clicked() {
                                self.toggle_listening();
                            }

                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(format!(
                                    "Catalogue : {} référence(s)",
                                    self.catalog.len()
                                ))
                                .size(10.0)
                                .color(theme.text_muted),
                            );
                        });
                });

                ui.add_space(10.0);

                // ----------------------------------------------------
                // 5. Console de Logs d'Activité
                // ----------------------------------------------------
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("📋 Journal d'activité")
                            .strong()
                            .size(13.0)
                            .color(theme.text_primary),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(
                                RichText::new("🗑 Effacer")
                                    .size(11.0)
                                    .color(theme.text_muted),
                            )
                            .clicked()
                        {
                            self.logs.clear();
                        }

                        if counts.rejected > 0 {
                            if ui
                                .button(
                                    RichText::new("Acquitter les refusés")
                                        .size(11.0)
                                        .color(Color32::from_rgb(248, 113, 113)),
                                )
                                .on_hover_text(
                                    "Retire les événements refusés de la file locale (l'historique reste dans n8n/Notion).",
                                )
                                .clicked()
                            {
                                let removed = self
                                    .outbox
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .clear_rejected();
                                if removed > 0 {
                                    self.add_log(
                                        LogLevel::Info,
                                        "FILE",
                                        &format!("{removed} refus(s) acquitté(s)."),
                                    );
                                }
                            }
                        }

                        ui.checkbox(
                            &mut self.auto_scroll,
                            RichText::new("Auto-scroll")
                                .size(11.0)
                                .color(theme.text_muted),
                        );
                    });
                });

                ui.add_space(4.0);

                egui::Frame::default()
                    .fill(theme.bg_console)
                    .stroke(Stroke::new(1.0_f32, theme.border_console))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::same(10.0))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .stick_to_bottom(self.auto_scroll)
                            .max_height(220.0)
                            .show(ui, |ui| {
                                if self.logs.is_empty() {
                                    ui.label(
                                        RichText::new("Aucun événement dans le journal.")
                                            .size(11.0)
                                            .italics()
                                            .color(theme.text_muted),
                                    );
                                } else {
                                    for log in &self.logs {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(&log.timestamp)
                                                    .monospace()
                                                    .size(11.0)
                                                    .color(theme.text_muted),
                                            );

                                            let (tag_bg, tag_fg) = match log.level {
                                                LogLevel::Scan => (
                                                    if theme.is_dark {
                                                        Color32::from_rgb(12, 44, 60)
                                                    } else {
                                                        Color32::from_rgb(224, 242, 254)
                                                    },
                                                    if theme.is_dark {
                                                        Color32::from_rgb(56, 189, 248)
                                                    } else {
                                                        Color32::from_rgb(2, 132, 199)
                                                    },
                                                ),
                                                LogLevel::Success => (
                                                    if theme.is_dark {
                                                        Color32::from_rgb(16, 44, 30)
                                                    } else {
                                                        Color32::from_rgb(220, 252, 231)
                                                    },
                                                    if theme.is_dark {
                                                        Color32::from_rgb(52, 211, 153)
                                                    } else {
                                                        Color32::from_rgb(22, 163, 74)
                                                    },
                                                ),
                                                LogLevel::Warning => (
                                                    if theme.is_dark {
                                                        Color32::from_rgb(48, 36, 16)
                                                    } else {
                                                        Color32::from_rgb(254, 243, 199)
                                                    },
                                                    if theme.is_dark {
                                                        Color32::from_rgb(251, 191, 36)
                                                    } else {
                                                        Color32::from_rgb(217, 119, 6)
                                                    },
                                                ),
                                                LogLevel::Error => (
                                                    if theme.is_dark {
                                                        Color32::from_rgb(48, 18, 22)
                                                    } else {
                                                        Color32::from_rgb(254, 226, 226)
                                                    },
                                                    if theme.is_dark {
                                                        Color32::from_rgb(248, 113, 113)
                                                    } else {
                                                        Color32::from_rgb(220, 38, 38)
                                                    },
                                                ),
                                                LogLevel::Info => (
                                                    if theme.is_dark {
                                                        Color32::from_rgb(28, 32, 50)
                                                    } else {
                                                        Color32::from_rgb(238, 242, 255)
                                                    },
                                                    if theme.is_dark {
                                                        Color32::from_rgb(165, 180, 252)
                                                    } else {
                                                        Color32::from_rgb(79, 70, 229)
                                                    },
                                                ),
                                                LogLevel::Raw => (
                                                    if theme.is_dark {
                                                        Color32::from_rgb(44, 28, 56)
                                                    } else {
                                                        Color32::from_rgb(243, 232, 255)
                                                    },
                                                    if theme.is_dark {
                                                        Color32::from_rgb(216, 180, 254)
                                                    } else {
                                                        Color32::from_rgb(147, 51, 234)
                                                    },
                                                ),
                                            };

                                            egui::Frame::default()
                                                .fill(tag_bg)
                                                .rounding(Rounding::same(4.0))
                                                .inner_margin(Margin {
                                                    left: 5.0,
                                                    right: 5.0,
                                                    top: 1.0,
                                                    bottom: 1.0,
                                                })
                                                .show(ui, |ui| {
                                                    ui.label(
                                                        RichText::new(&log.tag)
                                                            .monospace()
                                                            .strong()
                                                            .size(10.0)
                                                            .color(tag_fg),
                                                    );
                                                });

                                            ui.label(
                                                RichText::new(&log.text)
                                                    .monospace()
                                                    .size(11.0)
                                                    .color(theme.text_primary),
                                            );
                                        });
                                    }
                                }
                            });
                    });
                    });
            });

        // Appliquer les actions de la carte après le rendu, pour éviter les
        // conflits d'emprunt avec l'interface.
        let now = Instant::now();
        if minus {
            self.movement.adjust(-1, now);
        }
        if plus {
            self.movement.adjust(1, now);
        }
        if validate {
            if self
                .movement
                .pending()
                .is_some_and(|pending| pending.qty == 0)
            {
                self.add_log(
                    LogLevel::Warning,
                    "QUANTITÉ",
                    "Quantité nulle : saisissez au moins 1.",
                );
            } else if let Some(pending) = self.movement.take() {
                self.commit_pending(pending);
            }
        }
        if cancel {
            self.cancel_pending_input("bouton");
        }
        if undo {
            self.undo_last();
        }
        if print_label {
            self.print_cancel_label();
        }
        if copied {
            self.copied_time = Some(Instant::now());
        }

        // Toast de confirmation / refus, dessiné au-dessus de l'interface.
        if let Some(flash) = &self.flash {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("movement_flash"),
            ));
            let screen = ctx.screen_rect();
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(25));

            let toast = egui::Rect::from_center_size(screen.center(), egui::vec2(480.0, 86.0));
            painter.rect_filled(toast, Rounding::same(14.0), flash.color);
            painter.text(
                toast.center(),
                egui::Align2::CENTER_CENTER,
                flash.text.chars().take(70).collect::<String>(),
                egui::FontId::proportional(22.0),
                Color32::WHITE,
            );
            ctx.request_repaint();
        }

        // Rafraîchir périodiquement pour fluidifier la réception des logs et scans
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(pending) = self.movement.take() {
            // Un mouvement refusé (409) attend une correction : ne pas le
            // rejouer à la fermeture. L'événement refusé reste dans la file.
            if pending.qty > 0 && !pending.needs_review {
                self.commit_pending(pending);
            }
        }
        self.sender_running.store(false, Ordering::SeqCst);
        self.running_flag.store(false, Ordering::SeqCst);
    }
}

// ==========================================
// Point d'entrée
// ==========================================

fn main() -> eframe::Result<()> {
    // Choisir la variante d'icône correspondant à l'apparence du système
    let system_dark = system_prefers_dark();
    let icon = load_icon(system_dark);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([740.0, 720.0])
            .with_min_inner_size([620.0, 580.0])
            .with_resizable(true)
            .with_title("Scanner Pièces → n8n Bridge")
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Scanner Pieces -> n8n Bridge",
        options,
        Box::new(move |cc| Ok(Box::new(ScannerApp::new(cc, system_dark)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_buffer_splits_multiple_codes_and_keeps_partial() {
        let mut buffer = b"IN|DCO001\r\nOUT|DA001\rIN|DCO002".to_vec();
        let codes = drain_delimited(&mut buffer);
        assert_eq!(codes, vec!["IN|DCO001", "OUT|DA001"]);
        assert_eq!(buffer, b"IN|DCO002");

        let mut buffer = b"IN|DCO001\r\nOUT|DA0".to_vec();
        let codes = drain_delimited(&mut buffer);
        assert_eq!(codes, vec!["IN|DCO001"]);
        assert_eq!(buffer, b"OUT|DA0");
    }

    #[test]
    fn cancel_qr_is_recognized_before_movement_parsing() {
        assert!(is_cancel_command("ANNULER"));
        assert!(is_cancel_command("  annuler\r\n"));
        assert!(!is_cancel_command("IN|ANNULER"));
        assert!(!is_cancel_command("OUT|DCO001"));
    }

    #[test]
    fn printable_label_is_50_by_30_and_escapes_station() {
        let html = build_cancel_label_html("POSTE <01>");
        assert_eq!(html.matches("<section class=\"label\">").count(), 1);
        assert_eq!(html.matches("<svg ").count(), 1);
        assert!(html.contains("@page { size: 50mm 30mm; margin: 0; }"));
        assert!(html.contains("Poste : POSTE &lt;01&gt;"));
        assert!(html.contains("window.print()"));
    }

    #[test]
    fn webhook_payload_contains_v2_contract() {
        let payload = WebhookPayload {
            barcode: "OUT|DCO001",
            timestamp: "2026-10-26T10:00:00+00:00",
            device_id: "POSTE-01",
            event_id: "3f1c9e2a-7b4d-4e8f-9c1a-2d6b8e0f4a11",
            seq: 42,
            sku: "DCO001",
            action: "OUT",
            qty: 4,
            station_id: "POSTE-01",
            scanned_at: "2026-10-26T10:00:00+00:00",
            raw: "OUT|DCO001",
            app_version: "1.2.3",
            motif: Some("Annulation"),
            cancels_event_id: Some("previous-event"),
            is_test: None,
        };

        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["event_id"], "3f1c9e2a-7b4d-4e8f-9c1a-2d6b8e0f4a11");
        assert_eq!(json["sku"], "DCO001");
        assert_eq!(json["action"], "OUT");
        assert_eq!(json["qty"], 4);
        assert_eq!(json["station_id"], "POSTE-01");
        assert_eq!(json["barcode"], "OUT|DCO001");
        assert_eq!(json["is_test"], serde_json::Value::Null);
        assert_eq!(json["motif"], "Annulation");
        assert_eq!(json["cancels_event_id"], "previous-event");
    }

    #[test]
    fn webhook_response_parses_stock_error() {
        let response: WebhookResponse = serde_json::from_str(
            r#"{"ok":false,"code":"INSUFFICIENT_STOCK","available":2,"message":"Stock insuffisant"}"#,
        )
        .unwrap();
        assert_eq!(response.ok, Some(false));
        assert_eq!(response.code.as_deref(), Some("INSUFFICIENT_STOCK"));
        assert_eq!(response.available, Some(2));
        assert_eq!(response.message.as_deref(), Some("Stock insuffisant"));
    }
}
