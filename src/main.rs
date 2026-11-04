#![cfg_attr(windows, windows_subsystem = "windows")]
//! Cyberpunk Mod Compatibility Checker: warns before launch about installed mods likely to break on the current game build.
//! Read-only and offline: it never changes a file and never connects to anything.
mod check;
mod locate;

use check::{Manifest, Report, Status};
use eframe::egui::{self, Align, Color32, Layout, RichText, ThemePreference};
use std::{env, path::{Path, PathBuf}, thread::{self, JoinHandle}, time::Duration};

const TITLE: &str = "Cyberpunk Mod Compatibility Checker";
const ABOUT: &str = concat!("Cyberpunk Mod Compatibility Checker v", env!("CARGO_PKG_VERSION"), ", MIT license. \
    It only reads files: it never changes, moves or deletes anything, and never connects to the internet. \
    \"Open on Nexus\" hands the mod's page to your own web browser.\n\n\
    The compatibility list is curated by the maintainer and built into each release. A newer compat.toml placed next to the app is used instead.\n\n\
    Design inspired by red4-conflicts by rfuzzo (github.com/rfuzzo/Cyberpunk-utility): the dark theme and theme switch \
    and the rows with coloured status circles. Framework background from the RED Modding wiki (wiki.redmodding.org). \
    The verdicts, list format and layout are this app's own.\n\n\
    Built with egui/eframe, rfd, serde and toml. Their licenses are in THIRD-PARTY-NOTICES.txt.\n\n\
    The code was written by an AI assistant (Claude, made by Anthropic) at the maintainer's direction.");
const UNLISTED_TIP: &str = "Native plugins hook the game directly and often break after a patch. These aren't in the compatibility list.";

type Job = JoinHandle<Result<Report, String>>;

struct App {
    game: String, mo2: String, filter: String,
    list: Manifest, source: String, warning: Option<String>,
    report: Option<Result<Report, String>>,
    job: Option<Job>,
    theme: Option<ThemePreference>,      // last theme sent to the native title bar
}

impl App {
    fn new(cc: &eframe::CreationContext, start: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_theme(cc.storage.and_then(|s| eframe::get_value(s, "theme")).unwrap_or(ThemePreference::Dark));
        let (mut game, mut mo2): (String, String) = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        if let Some(s) = start { game = s.display().to_string() }   // a folder dropped onto the exe
        else if !locate::is_game(Path::new(&game)) && let Some(g) = locate::find_game() { game = g.display().to_string() }
        if mo2.is_empty() && let Some(m) = locate::find_mo2(Path::new(&game)) { mo2 = m.display().to_string() }
        let (list, source, warning) = Manifest::load(exe_dir().as_deref());
        let mut app = Self { game, mo2, filter: String::new(), list, source, warning, report: None, job: None, theme: None };
        app.rescan();
        app
    }

    /// Scans on a background thread, so a slow drive never freezes the window.
    fn rescan(&mut self) {
        let clean = |s: &str| PathBuf::from(s.trim().trim_matches('"'));   // tolerates a pasted "quoted path"
        let (game, mo2, list) = (clean(&self.game), clean(&self.mo2), self.list.clone());
        if game.as_os_str().is_empty() { return self.report = None }
        self.job = Some(thread::spawn(move || {
            let mods = if mo2.as_os_str().is_empty() { vec![] } else { locate::mo2_mods(&mo2)? };
            check::check(&list, &game, &mods)
        }));
    }

    fn poll(&mut self) {
        if self.job.as_ref().is_some_and(|j| j.is_finished()) {
            self.report = Some(self.job.take().unwrap().join().unwrap_or_else(|_| Err("The scan stopped unexpectedly.".into())));
        }
    }

    fn pick(&mut self, mo2: bool) {
        let field = if mo2 { &mut self.mo2 } else { &mut self.game };
        let dlg = rfd::FileDialog::new();
        let dlg = if Path::new(field.as_str()).is_dir() { dlg.set_directory(field.as_str()) } else { dlg };
        if let Some(d) = dlg.pick_folder() { *field = d.display().to_string(); self.rescan() }
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let Some(Ok(r)) = &self.report else { return };
        ui.horizontal(|ui| {
            ui.label("Mod filter:");
            ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(300.0));
            if ui.button("x").clicked() { self.filter.clear() }
        });
        ui.separator();
        let q = self.filter.to_lowercase();
        let rows: Vec<_> = r.rows.iter().filter(|x| x.name.to_lowercase().contains(&q)).collect();
        let of = |s| rows.iter().copied().filter(move |x| x.status == s).collect::<Vec<_>>();
        let unlisted: Vec<_> = r.unlisted.iter().filter(|n| n.to_lowercase().contains(&q)).collect();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (s, title, open) in [(Status::Red, "Likely to break", true), (Status::Gray, "Not confirmed", true), (Status::Green, "OK", false)] {
                let list = of(s);
                group(ui, title, color(s), format!("{title}: {}", count(list.len(), "mod")), list.len(), open, |ui| for x in &list {
                    ui.horizontal(|ui| {
                        dot(ui, color(s), &x.why);
                        cell(ui, 190.0, |ui| ui.label(RichText::new(&x.name).strong()));
                        cell(ui, 60.0, |ui| ui.weak(x.version.as_deref().unwrap_or("")));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let url = check::nexus_url(x.nexus_id);
                            if ui.small_button("Copy link").on_hover_text(&url).clicked() { ui.ctx().copy_text(url.clone()) }
                            if ui.small_button("Open on Nexus").on_hover_text("Opens the mod's page in your web browser").clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| ui.add(egui::Label::new(&x.why).truncate())).inner.on_hover_text(&x.why);
                        });
                    });
                    for n in &x.notes { ui.horizontal(|ui| { ui.add_space(24.0); ui.label(format!("• {} ({}, {})", n.text, n.source, n.date)) }); }
                });
            }
            group(ui, "unlisted", Color32::GRAY, format!("Unlisted native plugins: {}", unlisted.len()), unlisted.len(), false, |ui| {
                ui.weak(UNLISTED_TIP);
                for n in &unlisted { ui.horizontal(|ui| { dot(ui, Color32::GRAY, UNLISTED_TIP); ui.label(n.as_str()) }); }
            });
            if rows.len() + unlisted.len() == 0 { ui.weak(if q.is_empty() { "None of the listed frameworks or mods are installed." } else { "No mods match the filter." }); }
        });
    }

    fn start_page(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading("Choose your Cyberpunk 2077 folder to begin");
            ui.label("It's the folder that holds bin, archive and r6. Nothing in it is changed.");
            ui.add_space(12.0);
            if ui.add(egui::Button::new(RichText::new("Browse…").size(18.0)).min_size(egui::vec2(160.0, 36.0))).clicked() { self.pick(false) }
            if let Some(Err(e)) = &self.report { ui.add_space(12.0); ui.colored_label(Color32::RED, e); }
        });
    }
}

fn exe_dir() -> Option<PathBuf> { env::current_exe().ok()?.parent().map(Path::to_path_buf) }

fn color(s: Status) -> Color32 { match s { Status::Red => Color32::RED, Status::Gray => Color32::GRAY, Status::Green => Color32::GREEN } }

/// A 10 px status circle, as in red4-conflicts, with hover text so colour is never the only cue.
fn dot(ui: &mut egui::Ui, color: Color32, tip: &str) {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
    if !tip.is_empty() { r.on_hover_text(tip); }
}

/// A fixed-width column, so names, versions and reasons line up from row to row.
fn cell(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui) -> egui::Response) {
    ui.allocate_ui_with_layout(egui::vec2(w, 18.0), Layout::left_to_right(Align::Center), |ui| { ui.set_min_width(w); ui.set_max_width(w); add(ui) });
}

fn count(n: usize, noun: &str) -> String { format!("{n} {noun}{}", if n == 1 { "" } else { "s" }) }

/// A collapsible group headed by a status circle; hidden when empty.
fn group(ui: &mut egui::Ui, id: &str, color: Color32, title: String, n: usize, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    if n == 0 { return }
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(id), open)
        .show_header(ui, |ui| { dot(ui, color, ""); ui.label(RichText::new(title).strong()) })
        .body(body);
    ui.add_space(4.0);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        if self.job.is_some() { ctx.request_repaint_after(Duration::from_millis(50)) }
        let pref = ctx.options(|o| o.theme_preference);
        if self.theme != Some(pref) {                       // keep the native title bar in step with the theme
            self.theme = Some(pref);
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match pref {
                ThemePreference::Dark => egui::SystemTheme::Dark,
                ThemePreference::Light => egui::SystemTheme::Light,
                ThemePreference::System => egui::SystemTheme::SystemDefault,
            }));
        }
        egui::TopBottomPanel::top("menu").show(ctx, |ui| egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("About", |ui| ui.vertical(|ui| { ui.set_max_width(440.0); ui.label(ABOUT) }));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.horizontal(egui::widgets::global_theme_preference_buttons);
                ui.separator();
                ui.weak(concat!("v", env!("CARGO_PKG_VERSION")));
            });
        }));
        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.add_space(6.0);
            let mut go = false;
            for (label, mo2) in [("Game folder:", false), ("Mod Organizer 2:", true)] {
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 20.0], egui::Label::new(label));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if mo2 && ui.button("Clear").on_hover_text("Don't use Mod Organizer 2").clicked() { self.mo2.clear(); go = true }
                        if !mo2 && ui.add_enabled(self.job.is_none(), egui::Button::new("⟳ Rescan")).clicked() { go = true }
                        if ui.button("Browse…").on_hover_text(if mo2 { "The MO2 instance folder, which holds ModOrganizer.ini" } else { "" }).clicked() { self.pick(mo2) }
                        let field = if mo2 { &mut self.mo2 } else { &mut self.game };
                        let hint = if mo2 { "not used" } else { "" };
                        let edit = ui.add(egui::TextEdit::singleline(field).hint_text(hint).desired_width(f32::INFINITY));
                        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { go = true }
                    });
                });
            }
            if go { self.rescan() }
            ui.horizontal(|ui| {
                if let Some(Ok(r)) = &self.report {
                    ui.label(format!("Game build {}", r.build.as_deref().unwrap_or("unknown")));
                    ui.weak("·");
                }
                ui.weak(format!("Compatibility list {} ({})", self.list.updated, self.source));
                if self.job.is_some() { ui.spinner(); }
            });
            if let Some(w) = &self.warning { ui.colored_label(Color32::YELLOW, w); }
            if let Some(Ok(Report { banner: Some((warn, b)), .. })) = &self.report {
                let fill = if *warn { Color32::from_rgba_unmultiplied(220, 50, 50, 40) } else { Color32::from_rgba_unmultiplied(128, 128, 128, 40) };
                egui::Frame::new().fill(fill).inner_margin(8.0).corner_radius(4.0)
                    .show(ui, |ui| { ui.set_width(ui.available_width()); ui.label(RichText::new(b).strong()) });
            }
            ui.add_space(4.0);
        });
        egui::CentralPanel::default().show(ctx, |ui| match &self.report {
            Some(Ok(_)) => self.results(ui),
            _ if self.job.is_some() => { ui.centered_and_justified(|ui| ui.spinner()); }
            _ => self.start_page(ui),
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &(&self.game, &self.mo2));
        if let Some(t) = self.theme { eframe::set_value(storage, "theme", &t) }
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--report") {          // headless check: --report <game folder> [MO2 instance folder]
        let (list, source, warning) = Manifest::load(exe_dir().as_deref());
        if let Some(w) = warning { eprintln!("{w}") }
        let game = args.get(1).map(PathBuf::from).or_else(locate::find_game).unwrap_or_default();
        let mods = args.get(2).map_or(Ok(vec![]), |m| locate::mo2_mods(Path::new(m)));
        match mods.and_then(|m| check::check(&list, &game, &m)) {
            Ok(r) => print!("{}", check::text(&r, &source, &list.updated)),
            Err(e) => { eprintln!("{e}"); std::process::exit(1) }
        }
        return Ok(());
    }
    let start = args.first().map(PathBuf::from).filter(|p| p.is_dir());
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title(TITLE).with_inner_size([1100.0, 700.0]).with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(TITLE, opts, Box::new(|cc| Ok(Box::new(App::new(cc, start)))))
}
