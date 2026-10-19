use chrono::{Local, Utc};
use eframe::egui::{self, Align, Color32, Layout, Margin, RichText, Rounding, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

const CONFIG_FILE: &str = "scanner_config.json";
const BAUD_RATES: [u32; 5] = [9600, 19200, 38400, 57600, 115200];

fn default_dark_mode() -> bool {
    true
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

// ==========================================
// Modèles de données & Configuration
// ==========================================

#[derive(Serialize, Deserialize, Clone, Debug)]
struct AppConfig {
    webhook_url: String,
    device_id: String,
    selected_port: String,
    baud_rate: u32,
    #[serde(default = "default_dark_mode")]
    dark_mode: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            webhook_url: String::new(),
            device_id: "Netum-CS7501".to_string(),
            selected_port: String::new(),
            baud_rate: 9600,
            dark_mode: true,
        }
    }
}

impl AppConfig {
    fn load() -> Self {
        if let Ok(data) = fs::read_to_string(CONFIG_FILE) {
            if let Ok(cfg) = serde_json::from_str(&data) {
                return cfg;
            }
        }
        Self::default()
    }

    fn save(&self) {
        if let Ok(data) = serde_json::to_string_pretty(self) {
            let _ = fs::write(CONFIG_FILE, data);
        }
    }
}

#[derive(Serialize)]
struct WebhookPayload {
    barcode: String,
    timestamp: String,
    device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_test: Option<bool>,
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

enum AppEvent {
    Log(LogMessage),
    ScanReceived {
        barcode: String,
        time: String,
    },
    HttpResult {
        is_success: bool,
        status_code: Option<u16>,
        error_msg: Option<String>,
    },
}

struct LastScan {
    barcode: String,
    time: String,
    status_text: String,
    is_success: bool,
}

#[derive(Default)]
struct AppStats {
    total_scanned: usize,
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
    last_scan: Option<LastScan>,
    stats: AppStats,
    auto_scroll: bool,
    copied_time: Option<Instant>,
    webhook_url_shared: Arc<RwLock<String>>,
    device_id_shared: Arc<RwLock<String>>,
}

impl ScannerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut config = AppConfig::load();

        // Appliquer immédiatement le thème choisi (bloque l'écrasement par le thème système)
        Self::apply_theme(&cc.egui_ctx, config.dark_mode);

        let (event_tx, event_rx) = channel();

        // Récupérer les ports disponibles
        let mut available_ports = Vec::new();
        if let Ok(ports) = serialport::available_ports() {
            available_ports = ports.into_iter().map(|p| p.port_name).collect();
            if config.selected_port.is_empty() && !available_ports.is_empty() {
                config.selected_port = available_ports[0].clone();
            }
        }

        let webhook_url_shared = Arc::new(RwLock::new(config.webhook_url.clone()));
        let device_id_shared = Arc::new(RwLock::new(config.device_id.clone()));

        let mut app = Self {
            config,
            available_ports,
            is_listening: false,
            running_flag: Arc::new(AtomicBool::new(false)),
            event_rx,
            event_tx: event_tx.clone(),
            logs: Vec::new(),
            last_scan: None,
            stats: AppStats::default(),
            auto_scroll: true,
            copied_time: None,
            webhook_url_shared,
            device_id_shared,
        };

        app.add_log(
            LogLevel::Info,
            "SYSTÈME",
            "Application initialisée. Prêt pour l'écoute série.",
        );

        app
    }

    fn apply_theme(ctx: &egui::Context, dark_mode: bool) {
        if dark_mode {
            ctx.set_theme(egui::ThemePreference::Dark);
            let mut visuals = egui::Visuals::dark();

            visuals.panel_fill = Color32::from_rgb(14, 16, 24);
            visuals.window_fill = Color32::from_rgb(20, 23, 34);
            visuals.extreme_bg_color = Color32::from_rgb(10, 11, 17);
            visuals.faint_bg_color = Color32::from_rgb(26, 30, 44);
            visuals.override_text_color = Some(Color32::from_rgb(241, 245, 249));

            visuals.widgets.noninteractive.rounding = Rounding::same(8.0);
            visuals.widgets.inactive.rounding = Rounding::same(8.0);
            visuals.widgets.hovered.rounding = Rounding::same(8.0);
            visuals.widgets.active.rounding = Rounding::same(8.0);

            visuals.widgets.inactive.bg_fill = Color32::from_rgb(28, 32, 48);
            visuals.widgets.inactive.bg_stroke =
                Stroke::new(1.0_f32, Color32::from_rgb(44, 50, 74));

            visuals.widgets.hovered.bg_fill = Color32::from_rgb(38, 44, 66);
            visuals.widgets.hovered.bg_stroke =
                Stroke::new(1.0_f32, Color32::from_rgb(99, 102, 241));

            visuals.widgets.active.bg_fill = Color32::from_rgb(79, 70, 229);

            ctx.set_visuals(visuals);
        } else {
            ctx.set_theme(egui::ThemePreference::Light);
            let mut visuals = egui::Visuals::light();

            visuals.panel_fill = Color32::from_rgb(241, 245, 249);
            visuals.window_fill = Color32::from_rgb(255, 255, 255);
            visuals.extreme_bg_color = Color32::from_rgb(255, 255, 255);
            visuals.faint_bg_color = Color32::from_rgb(241, 245, 249);
            visuals.override_text_color = Some(Color32::from_rgb(15, 23, 42));

            visuals.widgets.noninteractive.rounding = Rounding::same(8.0);
            visuals.widgets.inactive.rounding = Rounding::same(8.0);
            visuals.widgets.hovered.rounding = Rounding::same(8.0);
            visuals.widgets.active.rounding = Rounding::same(8.0);

            visuals.widgets.inactive.bg_fill = Color32::from_rgb(255, 255, 255);
            visuals.widgets.inactive.bg_stroke =
                Stroke::new(1.0_f32, Color32::from_rgb(203, 213, 225));

            visuals.widgets.hovered.bg_fill = Color32::from_rgb(241, 245, 249);
            visuals.widgets.hovered.bg_stroke =
                Stroke::new(1.0_f32, Color32::from_rgb(99, 102, 241));

            visuals.widgets.active.bg_fill = Color32::from_rgb(79, 70, 229);

            ctx.set_visuals(visuals);
        }

        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 7.0);
        ctx.set_style(style);
    }

    fn add_log(&mut self, level: LogLevel, tag: &str, text: &str) {
        self.logs.push(LogMessage {
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            level,
            tag: tag.to_string(),
            text: text.to_string(),
        });
    }

    fn refresh_ports(&mut self) {
        if let Ok(ports) = serialport::available_ports() {
            self.available_ports = ports.into_iter().map(|p| p.port_name).collect();
            if !self.available_ports.is_empty()
                && !self.available_ports.contains(&self.config.selected_port)
            {
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
        let tx = self.event_tx.clone();

        self.add_log(LogLevel::Info, "TEST", "Envoi d'un payload test à n8n...");

        thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default();

            let payload = WebhookPayload {
                barcode: "TEST-SCAN-123456".to_string(),
                timestamp: Utc::now().to_rfc3339(),
                device_id: dev_id,
                is_test: Some(true),
            };

            let now = Local::now().format("%H:%M:%S").to_string();
            match client.post(&url).json(&payload).send() {
                Ok(res) if res.status().is_success() => {
                    let code = res.status().as_u16();
                    let _ = tx.send(AppEvent::Log(LogMessage {
                        timestamp: now,
                        level: LogLevel::Success,
                        tag: format!("n8n {code}"),
                        text: "Test réussi ! n8n a accepté le webhook (HTTP 200).".to_string(),
                    }));
                }
                Ok(res) => {
                    let code = res.status().as_u16();
                    let _ = tx.send(AppEvent::Log(LogMessage {
                        timestamp: now,
                        level: LogLevel::Error,
                        tag: format!("n8n {code}"),
                        text: format!("Le webhook n8n a répondu avec le statut HTTP {code}."),
                    }));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Log(LogMessage {
                        timestamp: now,
                        level: LogLevel::Error,
                        tag: "HTTP ERR".to_string(),
                        text: format!("Erreur réseau lors du test : {e}"),
                    }));
                }
            }
        });
    }

    fn toggle_listening(&mut self) {
        if self.is_listening {
            self.running_flag.store(false, Ordering::SeqCst);
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
            self.running_flag.store(true, Ordering::SeqCst);
            self.is_listening = true;

            let running = Arc::clone(&self.running_flag);
            let port_name = self.config.selected_port.clone();
            let baud = self.config.baud_rate;
            let url_shared = Arc::clone(&self.webhook_url_shared);
            let dev_id_shared = Arc::clone(&self.device_id_shared);
            let tx = self.event_tx.clone();

            thread::spawn(move || {
                let now = Local::now().format("%H:%M:%S").to_string();
                let _ = tx.send(AppEvent::Log(LogMessage {
                    timestamp: now,
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
                            // Activer explicitement les lignes de contrôle DTR et RTS
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

                            let client = reqwest::blocking::Client::builder()
                                .timeout(Duration::from_secs(5))
                                .build()
                                .unwrap_or_default();

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

                                        // Si le buffer contient un délimiteur de fin (\r ou \n)
                                        if accumulator.contains(&b'\n')
                                            || accumulator.contains(&b'\r')
                                        {
                                            let text = String::from_utf8_lossy(&accumulator)
                                                .trim()
                                                .to_string();
                                            accumulator.clear();
                                            if !text.is_empty() {
                                                let cur_url = url_shared
                                                    .read()
                                                    .unwrap_or_else(|e| e.into_inner())
                                                    .clone();
                                                let cur_dev = dev_id_shared
                                                    .read()
                                                    .unwrap_or_else(|e| e.into_inner())
                                                    .clone();
                                                process_scanned_code(
                                                    &text, &cur_url, &cur_dev, &tx, &client,
                                                );
                                            }
                                        }
                                    }
                                    Ok(_) => {} // Aucun octet lu
                                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                                        // Timeout 50ms : si des octets sont arrivés il y a > 100ms
                                        // et que le scanner n'a envoyé ni \r ni \n, on valide quand même !
                                        if !accumulator.is_empty()
                                            && last_rx_time.elapsed()
                                                >= Duration::from_millis(100)
                                        {
                                            let text = String::from_utf8_lossy(&accumulator)
                                                .trim()
                                                .to_string();
                                            accumulator.clear();
                                            if !text.is_empty() {
                                                let cur_url = url_shared
                                                    .read()
                                                    .unwrap_or_else(|e| e.into_inner())
                                                    .clone();
                                                let cur_dev = dev_id_shared
                                                    .read()
                                                    .unwrap_or_else(|e| e.into_inner())
                                                    .clone();
                                                process_scanned_code(
                                                    &text, &cur_url, &cur_dev, &tx, &client,
                                                );
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
}

fn process_scanned_code(
    raw_code: &str,
    url: &str,
    dev_id: &str,
    tx: &Sender<AppEvent>,
    client: &reqwest::blocking::Client,
) {
    let raw_code = raw_code.trim().to_string();
    if raw_code.is_empty() {
        return;
    }

    let time_str = Local::now().format("%H:%M:%S").to_string();

    let _ = tx.send(AppEvent::ScanReceived {
        barcode: raw_code.clone(),
        time: time_str.clone(),
    });

    let _ = tx.send(AppEvent::Log(LogMessage {
        timestamp: time_str,
        level: LogLevel::Scan,
        tag: "SCAN".to_string(),
        text: format!("Code détecté : {raw_code}"),
    }));

    if !url.is_empty() {
        let payload = WebhookPayload {
            barcode: raw_code.clone(),
            timestamp: Utc::now().to_rfc3339(),
            device_id: dev_id.to_string(),
            is_test: None,
        };

        match client.post(url).json(&payload).send() {
            Ok(res) if res.status().is_success() => {
                let code = res.status().as_u16();
                let _ = tx.send(AppEvent::HttpResult {
                    is_success: true,
                    status_code: Some(code),
                    error_msg: None,
                });
                let _ = tx.send(AppEvent::Log(LogMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    level: LogLevel::Success,
                    tag: format!("n8n {code}"),
                    text: "Code transmis avec succès à n8n".to_string(),
                }));
            }
            Ok(res) => {
                let code = res.status().as_u16();
                let _ = tx.send(AppEvent::HttpResult {
                    is_success: false,
                    status_code: Some(code),
                    error_msg: Some(format!("Échec n8n (HTTP {code})")),
                });
                let _ = tx.send(AppEvent::Log(LogMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    level: LogLevel::Error,
                    tag: format!("n8n {code}"),
                    text: format!("Échec HTTP (statut {code})"),
                }));
            }
            Err(e) => {
                let _ = tx.send(AppEvent::HttpResult {
                    is_success: false,
                    status_code: None,
                    error_msg: Some(e.to_string()),
                });
                let _ = tx.send(AppEvent::Log(LogMessage {
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    level: LogLevel::Error,
                    tag: "HTTP ERR".to_string(),
                    text: format!("Erreur réseau : {e}"),
                }));
            }
        }
    } else {
        let _ = tx.send(AppEvent::Log(LogMessage {
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            level: LogLevel::Warning,
            tag: "WEBHOOK".to_string(),
            text: "Code scanné mais aucun Webhook n8n n'est configuré.".to_string(),
        }));
    }
}

// ==========================================
// Rendu de l'Interface Graphique (egui)
// ==========================================

impl eframe::App for ScannerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Maintenir le thème cohérent et synchronisé
        let theme = ThemeColors::new(self.config.dark_mode);

        // Dépouillement des événements en arrière-plan
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::Log(msg) => {
                    self.logs.push(msg);
                }
                AppEvent::ScanReceived { barcode, time } => {
                    self.stats.total_scanned += 1;
                    self.last_scan = Some(LastScan {
                        barcode,
                        time,
                        status_text: "Envoi à n8n en cours...".to_string(),
                        is_success: false,
                    });
                }
                AppEvent::HttpResult {
                    is_success,
                    status_code,
                    error_msg,
                } => {
                    if is_success {
                        self.stats.sent_success += 1;
                        if let Some(ref mut scan) = self.last_scan {
                            scan.status_text =
                                format!("✓ Reçu par n8n (HTTP {})", status_code.unwrap_or(200));
                            scan.is_success = true;
                        }
                    } else {
                        self.stats.sent_failed += 1;
                        if let Some(ref mut scan) = self.last_scan {
                            scan.status_text =
                                error_msg.unwrap_or_else(|| "Échec de l'envoi".to_string());
                            scan.is_success = false;
                        }
                    }
                }
            }
        }

        // On force le fond du panneau central à la couleur exacte du thème choisi
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme.bg_panel).inner_margin(Margin::same(16.0)))
            .show(ctx, |ui| {
                // ----------------------------------------------------
                // 1. Header & Statut
                // ----------------------------------------------------
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading(
                            RichText::new("⚡ Netum CS7501 ➔ n8n")
                                .color(theme.text_primary)
                                .strong()
                                .size(20.0),
                        );
                        ui.label(
                            RichText::new("Passerelle temps réel Port Série (SPP) vers Webhook n8n")
                                .color(theme.text_muted)
                                .size(12.0),
                        );
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Badge Statut
                        let (badge_bg, badge_stroke, badge_text, badge_color) = if self.is_listening {
                            (
                                Color32::from_rgb(16, 48, 36),
                                Color32::from_rgb(16, 185, 129),
                                "● ÉCOUTE ACTIVE",
                                Color32::from_rgb(52, 211, 153),
                            )
                        } else {
                            if theme.is_dark {
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
                            }
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

                        // Bouton Bascule Thème Sombre / Clair
                        let (theme_icon, theme_label) = if self.config.dark_mode {
                            ("🌙", "Sombre")
                        } else {
                            ("☀️", "Clair")
                        };

                        if ui
                            .button(
                                RichText::new(format!("{theme_icon} {theme_label}"))
                                    .size(12.0)
                                    .color(theme.text_primary),
                            )
                            .on_hover_text("Changer de thème (Sombre / Clair)")
                            .clicked()
                        {
                            self.config.dark_mode = !self.config.dark_mode;
                            self.config.save();
                            Self::apply_theme(ctx, self.config.dark_mode);
                        }
                    });
                });

                ui.add_space(10.0);

                // ----------------------------------------------------
                // 2. Cartes de Métriques / KPIs
                // ----------------------------------------------------
                ui.columns(3, |cols| {
                    // Total scannés
                    egui::Frame::default()
                        .fill(theme.bg_card)
                        .stroke(Stroke::new(1.0_f32, theme.border_card))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::same(10.0))
                        .show(&mut cols[0], |ui| {
                            ui.label(
                                RichText::new("TOTAL SCANNÉS")
                                    .size(10.0)
                                    .color(theme.text_muted)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(self.stats.total_scanned.to_string())
                                    .size(20.0)
                                    .strong()
                                    .color(theme.text_primary),
                            );
                        });

                    // Succès n8n
                    egui::Frame::default()
                        .fill(theme.bg_card)
                        .stroke(Stroke::new(1.0_f32, theme.border_card))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::same(10.0))
                        .show(&mut cols[1], |ui| {
                            ui.label(
                                RichText::new("SUCCÈS N8N")
                                    .size(10.0)
                                    .color(Color32::from_rgb(16, 185, 129))
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(self.stats.sent_success.to_string())
                                    .size(20.0)
                                    .strong()
                                    .color(Color32::from_rgb(16, 185, 129)),
                            );
                        });

                    // Échecs
                    egui::Frame::default()
                        .fill(theme.bg_card)
                        .stroke(Stroke::new(1.0_f32, theme.border_card))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::same(10.0))
                        .show(&mut cols[2], |ui| {
                            ui.label(
                                RichText::new("ÉCHECS")
                                    .size(10.0)
                                    .color(Color32::from_rgb(239, 68, 68))
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(self.stats.sent_failed.to_string())
                                    .size(20.0)
                                    .strong()
                                    .color(Color32::from_rgb(239, 68, 68)),
                            );
                        });
                });

                ui.add_space(10.0);

                // ----------------------------------------------------
                // 3. Carte du Dernier Scan (Hero Card)
                // ----------------------------------------------------
                let hero_border = if self.last_scan.is_some() {
                    theme.accent
                } else {
                    theme.border_card
                };

                egui::Frame::default()
                    .fill(theme.bg_card)
                    .stroke(Stroke::new(1.5_f32, hero_border))
                    .rounding(Rounding::same(10.0))
                    .inner_margin(Margin::same(12.0))
                    .show(ui, |ui| {
                        if let Some(ref scan) = self.last_scan {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("📦 DERNIER CODE SCANNÉ")
                                        .size(11.0)
                                        .color(theme.accent)
                                        .strong(),
                                );
                                ui.label(
                                    RichText::new(format!("à {}", scan.time))
                                        .size(11.0)
                                        .color(theme.text_muted),
                                );

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if let Some(copied_at) = self.copied_time {
                                        if copied_at.elapsed() < Duration::from_secs(2) {
                                            ui.label(
                                                RichText::new("✓ Copié !")
                                                    .color(Color32::from_rgb(16, 185, 129))
                                                    .size(12.0)
                                                    .strong(),
                                            );
                                        }
                                    }

                                    if ui
                                        .button(
                                            RichText::new("📋 Copier")
                                                .size(12.0)
                                                .color(theme.text_primary),
                                        )
                                        .clicked()
                                    {
                                        ui.output_mut(|o| o.copied_text = scan.barcode.clone());
                                        self.copied_time = Some(Instant::now());
                                    }
                                });
                            });

                            ui.add_space(4.0);

                            // Code-barres grand format
                            ui.label(
                                RichText::new(&scan.barcode)
                                    .size(24.0)
                                    .strong()
                                    .monospace()
                                    .color(theme.text_primary),
                            );

                            ui.add_space(2.0);

                            // Statut n8n
                            let status_color = if scan.is_success {
                                Color32::from_rgb(16, 185, 129)
                            } else {
                                Color32::from_rgb(239, 68, 68)
                            };

                            ui.label(
                                RichText::new(&scan.status_text)
                                    .size(12.0)
                                    .color(status_color),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("📦 DERNIER CODE SCANNÉ")
                                        .size(11.0)
                                        .color(theme.text_muted)
                                        .strong(),
                                );
                            });
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(
                                    "Aucun code scanné pour le moment. En attente du premier scan...",
                                )
                                .size(13.0)
                                .italics()
                                .color(theme.text_muted),
                            );
                        }
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
                            let avail = ui.available_width();
                            let url_edit = ui.add(
                                egui::TextEdit::singleline(&mut self.config.webhook_url)
                                    .hint_text("https://n8n.mondomaine.com/webhook/...")
                                    .desired_width(avail),
                            );
                            if url_edit.changed() || url_edit.lost_focus() {
                                self.config.save();
                                if let Ok(mut w) = self.webhook_url_shared.write() {
                                    *w = self.config.webhook_url.clone();
                                }
                            }

                            // Assistant intelligent n8n (Test vs Production)
                            if self.config.webhook_url.contains("/webhook-test/") {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("⚠️ Mode Test (/webhook-test/)")
                                            .size(10.5)
                                            .color(Color32::from_rgb(251, 191, 36)),
                                    );
                                    if ui
                                        .button(
                                            RichText::new("Passer en Production (/webhook/)")
                                                .size(10.5)
                                                .color(Color32::from_rgb(52, 211, 153)),
                                        )
                                        .on_hover_text("Remplace /webhook-test/ par /webhook/ pour le workflow publié")
                                        .clicked()
                                    {
                                        self.config.webhook_url = self.config.webhook_url.replace("/webhook-test/", "/webhook/");
                                        self.config.save();
                                        if let Ok(mut w) = self.webhook_url_shared.write() {
                                            *w = self.config.webhook_url.clone();
                                        }
                                    }
                                });
                            } else if self.config.webhook_url.contains("/webhook/") {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("✓ Mode Production (/webhook/)")
                                            .size(10.5)
                                            .color(Color32::from_rgb(52, 211, 153)),
                                    );
                                    if ui
                                        .button(
                                            RichText::new("Passer en Test (/webhook-test/)")
                                                .size(10.5)
                                                .color(Color32::from_rgb(148, 163, 184)),
                                        )
                                        .clicked()
                                    {
                                        self.config.webhook_url = self.config.webhook_url.replace("/webhook/", "/webhook-test/");
                                        self.config.save();
                                        if let Ok(mut w) = self.webhook_url_shared.write() {
                                            *w = self.config.webhook_url.clone();
                                        }
                                    }
                                });
                            }

                            ui.add_space(4.0);

                            ui.label(
                                RichText::new("Device ID :")
                                    .size(11.0)
                                    .color(theme.text_muted),
                            );
                            let dev_edit = ui.add(
                                egui::TextEdit::singleline(&mut self.config.device_id)
                                    .hint_text("Netum-CS7501")
                                    .desired_width(avail),
                            );
                            if dev_edit.changed() || dev_edit.lost_focus() {
                                self.config.save();
                                if let Ok(mut w) = self.device_id_shared.write() {
                                    *w = self.config.device_id.clone();
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

                                if ui.button("🔄").on_hover_text("Actualiser les ports").clicked() {
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

                            ui.add_space(10.0);

                            // Bouton CTA principal
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

                            let btn = egui::Button::new(
                                RichText::new(btn_text)
                                    .strong()
                                    .size(13.0)
                                    .color(text_color),
                            )
                            .fill(btn_fill);

                            let btn_width = ui.available_width();
                            if ui.add_sized([btn_width, 32.0], btn).clicked() {
                                self.toggle_listening();
                            }
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

                        ui.checkbox(
                            &mut self.auto_scroll,
                            RichText::new("Auto-scroll").size(11.0).color(theme.text_muted),
                        );
                    });
                });

                ui.add_space(4.0);

                // Fenêtre de console type terminal
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
                                            // Horodatage
                                            ui.label(
                                                RichText::new(&log.timestamp)
                                                    .monospace()
                                                    .size(11.0)
                                                    .color(theme.text_muted),
                                            );

                                            // Badge de niveau
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

                                            // Message
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

        // Rafraîchir périodiquement pour fluidifier la réception des logs et scans
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

// ==========================================
// Point d'entrée
// ==========================================

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([740.0, 720.0])
            .with_min_inner_size([620.0, 580.0])
            .with_resizable(true)
            .with_title("Netum CS7501 ➔ n8n Bridge"),
        ..Default::default()
    };

    eframe::run_native(
        "Netum CS7501 -> n8n Bridge",
        options,
        Box::new(|cc| Ok(Box::new(ScannerApp::new(cc)))),
    )
}