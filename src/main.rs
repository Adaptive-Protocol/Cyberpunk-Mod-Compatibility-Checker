#![cfg_attr(windows, windows_subsystem = "windows")]
//! Cyberpunk Mod Compatibility Checker: warns before launch about installed mods likely to break on the current game build.
//! Offline: it never connects to anything. It changes files only when the user asks (remove.rs).
mod check;
mod locate;
mod remove;

use check::{Manifest, Part, Report, Status};
use eframe::egui::{self, Align, Color32, Layout, RichText, ThemePreference};
use std::{env, path::{Path, PathBuf}, process::Command, sync::{Arc, atomic::{AtomicBool, Ordering}}, thread::{self, JoinHandle}, time::Duration};

const TITLE: &str = "Cyberpunk Mod Compatibility Checker";
const ABOUT: &str = concat!("Cyberpunk Mod Compatibility Checker v", env!("CARGO_PKG_VERSION"), " by FauxNoodles on Nexus Mods, MIT license. \
    It never connects to the internet: \"Open on Nexus\" hands the mod's page to your own web browser. \
    It changes files only when you ask: Back up and remove moves a broken mod into _mod_backups in your game folder, \
    and Delete removes it for good. Both show every file first, and neither touches the game's own files or the frameworks.\n\n\
    The compatibility list is curated by the maintainer and built into each release. A newer compat.toml placed next to the app is used instead. \
    Besides your mods, the app reads the logs from your last launch (RED4ext's, redscript's, your mods' own, and Advanced Crash Reporter's if you use it) \
    and, on Windows, the version of the Visual C++ runtime.\n\n\
    Design inspired by red4-conflicts by rfuzzo (github.com/rfuzzo/Cyberpunk-utility): the dark theme and theme switch \
    and the rows with coloured status circles. Framework background from the RED Modding wiki (wiki.redmodding.org). \
    Backing a mod up instead of deleting it follows the reversible disable in mod managers such as Vortex and Mod Organizer 2, \
    and the confirmation that lists every file comes from the maintainer's Nexus Mod Duplication Cleanup. \
    The verdicts, list format and layout are this app's own.\n\n\
    Built with egui/eframe, rfd, serde and toml. Their licenses are in THIRD-PARTY-NOTICES.txt.\n\n\
    Started with --launch (from Steam's launch options), it starts the game after the check.\n\n\
    The code was written by an AI assistant (Claude, made by Anthropic) at the maintainer's direction.");
const LAST_TIP: &str = "From the logs of the last time the game ran. They update the next time you launch it.";
const REMOVE_TIP: &str = "Move it out of the game folder until it's updated, or delete it. You see every file first.";
const REMOVE_INFO: &str = "Back up and remove moves the ticked items into _mod_backups in your game folder, keeping their folder paths, \
    so the Backups window can put them back once the mod is updated. Close the game first.";
const BACKUPS_INFO: &str = "Mods you backed up and removed, kept in _mod_backups in your game folder. Restore puts one back. \
    If you've installed an update since, delete its backup instead.";
const UNLISTED_TIP: &str = "Native plugins hook the game directly and often break after a patch. These aren't in the compatibility list.";

type Job = JoinHandle<Result<Report, String>>;

/// An entry the Remove dialog lists: (path, why it's offered, ticked).
type Entry = (String, Part, bool);

/// The Remove dialog: each mod (name, why, entries, Mod Organizer 2 mods to untick), and whether the user chose Delete.
struct Removal { mods: Vec<(String, String, Vec<Entry>, Vec<String>)>, delete: bool }

struct App {
    game: String, mo2: String, filter: String,
    list: Manifest, source: String, warning: Option<String>,
    report: Option<Result<Report, String>>,
    job: Option<Job>,
    theme: Option<ThemePreference>,      // last theme sent to the native title bar
    removal: Option<Removal>,
    backups: Option<Vec<remove::Backup>>,   // the Backups window, while open
    discard: Option<PathBuf>,            // the backup waiting for "Delete for good"
    n_backups: usize,
    notice: Option<(bool, String)>,      // (succeeded, what happened) after a file action
    launch: Option<Arc<AtomicBool>>,     // --launch: set to start the game once the window closes
    auto: bool,                          // --launch: the first scan hasn't been judged yet
}

impl App {
    fn new(cc: &eframe::CreationContext, start: Option<PathBuf>, launch: Option<Arc<AtomicBool>>) -> Self {
        cc.egui_ctx.set_theme(cc.storage.and_then(|s| eframe::get_value(s, "theme")).unwrap_or(ThemePreference::Dark));
        let (mut game, mut mo2): (String, String) = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        if let Some(s) = start { game = s.display().to_string() }   // a folder dropped onto the exe
        else if !locate::is_game(Path::new(&game)) && let Some(g) = locate::find_game() { game = g.display().to_string() }
        if mo2.is_empty() && let Some(m) = locate::find_mo2(Path::new(&game)) { mo2 = m.display().to_string() }
        let (list, source, warning) = Manifest::load(exe_dir().as_deref());
        let mut app = Self { game, mo2, filter: String::new(), list, source, warning, report: None, job: None, theme: None,
                             removal: None, backups: None, discard: None, n_backups: 0, notice: None, auto: launch.is_some(), launch };
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
            self.n_backups = match &self.report { Some(Ok(r)) => remove::list(&r.game).len(), _ => 0 };
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
        let mut ask: Option<Vec<&check::Row>> = None;          // rows whose Remove… was pressed
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if let Some(l) = &r.last {
                let n = l.errors.len() + [l.crash.is_some(), l.memory.is_some(), l.lead.is_some()].into_iter().filter(|&x| x).count();
                let mut what = vec![];
                if l.crash.is_some() { what.push("it crashed".to_string()) } else if l.memory.is_some() { what.push("memory ran nearly full".into()) }
                if !l.errors.is_empty() || what.is_empty() { what.push(format!("mods logged {}", count(l.errors.len(), "error"))) }
                let title = format!("Your last launch, {}: {}", l.when, what.join(", and "));
                group(ui, "last", Color32::RED, title, n, true, |ui| {
                    ui.weak(LAST_TIP);
                    if let Some((c, report)) = &l.crash {
                        ui.horizontal(|ui| { dot(ui, Color32::RED, report); ui.add(egui::Label::new(RichText::new(c).strong()).truncate().show_tooltip_when_elided(false)).on_hover_text(format!("{c}\n\nReport: {report}")) });
                    }
                    for m in [&l.lead, &l.memory].into_iter().flatten() { ui.horizontal(|ui| { dot(ui, Color32::RED, ""); ui.add(egui::Label::new(RichText::new(m).strong()).truncate()) }); }
                    for (who, msg) in &l.errors {
                        ui.horizontal(|ui| {
                            dot(ui, Color32::RED, "");
                            cell(ui, 190.0, |ui| ui.add(egui::Label::new(RichText::new(who).strong()).truncate()));
                            ui.add(egui::Label::new(msg).truncate());          // shows the whole line on hover when cut
                        });
                    }
                });
            }
            for (s, title, open) in [(Status::Red, "Likely to break", true), (Status::Gray, "Not confirmed", true), (Status::Green, "OK", false)] {
                let list = of(s);
                group(ui, title, color(s), format!("{title}: {}", count(list.len(), "mod")), list.len(), open, |ui| {
                    let can: Vec<&check::Row> = list.iter().copied().filter(|x| !x.remove.is_empty()).collect();
                    if can.len() > 1 && ui.button(format!("Remove all {}…", can.len())).on_hover_text(REMOVE_TIP).clicked() { ask = Some(can) }
                    for x in &list {
                    ui.horizontal(|ui| {
                        dot(ui, color(s), &x.why);
                        cell(ui, 190.0, |ui| ui.label(RichText::new(&x.name).strong()));
                        cell(ui, 60.0, |ui| ui.weak(x.version.as_deref().unwrap_or("")));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !x.remove.is_empty() {
                                if ui.small_button("Remove…").on_hover_text(REMOVE_TIP).clicked() { ask = Some(vec![x]) }
                            } else if !x.mo2.is_empty() {
                                ui.weak("in MO2").on_hover_text(format!("Its files are in Mod Organizer 2 ({}). Untick it there.", x.mo2.join(", ")));
                            }
                            if let Some(id) = x.nexus_id {
                                let url = check::nexus_url(id);
                                if ui.small_button("Copy link").on_hover_text(&url).clicked() { ui.ctx().copy_text(url.clone()) }
                                if ui.small_button("Open on Nexus").on_hover_text("Opens the mod's page in your web browser").clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                                }
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| ui.add(egui::Label::new(&x.why).truncate()));
                        });
                    });
                    for n in &x.notes { ui.horizontal(|ui| { ui.add_space(24.0); ui.label(format!("• {} ({}, {})", n.text, n.source, n.date)) }); }
                    }
                });
            }
            group(ui, "unlisted", Color32::GRAY, format!("Unlisted native plugins: {}", unlisted.len()), unlisted.len(), true, |ui| {
                ui.weak(UNLISTED_TIP);
                for n in &unlisted { ui.horizontal(|ui| { dot(ui, Color32::GRAY, UNLISTED_TIP); ui.label(n.as_str()) }); }
            });
            if rows.len() + unlisted.len() == 0 { ui.weak(if q.is_empty() { "None of the listed frameworks or mods are installed." } else { "No mods match the filter." }); }
        });
        if let Some(rows) = ask {                                // each entry once, even when two mods share it
            let mut seen: Vec<&str> = vec![];
            let mods = rows.iter().map(|x| (x.name.clone(), x.why.clone(), x.remove.iter().filter(|(p, _)| {
                let new = !seen.contains(&p.as_str());
                seen.push(p);
                new
            }).map(|(p, part)| (p.clone(), part.clone(), true)).collect(), x.mo2.clone())).collect();
            self.removal = Some(Removal { mods, delete: false });
        }
    }

    /// The Remove dialog. Nothing changes until the user presses Back up and remove, or Delete and then Delete for good.
    fn removal_dialog(&mut self, ctx: &egui::Context) {
        let (Some(mut rm), Some(Ok(r))) = (self.removal.take(), &self.report) else { return };
        let game = r.game.clone();
        let ticked = rm.mods.iter().flat_map(|m| &m.2).filter(|e| e.2).count();
        let (mut run, mut close) = (None, false);              // run: Some(delete?) once confirmed
        let screen = ctx.screen_rect().size();                  // fits the smallest window too
        let modal = egui::Modal::new(egui::Id::new("remove")).show(ctx, |ui| {
            ui.set_width((screen.x - 80.0).clamp(300.0, 640.0));
            ui.heading(match rm.mods.as_slice() { [one] => format!("Remove {}", one.0), all => format!("Remove {} mods", all.len()) });
            ui.label(REMOVE_INFO);
            if folder_row(ui, "Backups go to", &game) { self.notice = open_folder(&game) }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height((screen.y - 240.0).clamp(100.0, 340.0)).show(ui, |ui| for (name, why, entries, mo2) in &mut rm.mods {
                ui.label(RichText::new(name.as_str()).strong()).on_hover_text(why.as_str());
                for (p, part, tick) in entries.iter_mut() {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.checkbox(tick, RichText::new(p.as_str()).monospace());
                        match part {
                            Part::Own => {}
                            Part::Kin => { ui.weak("same name").on_hover_text("Found elsewhere under the same name, so likely part of this mod"); }
                            Part::Uses(b) => { ui.weak(format!("builds on {b}")).on_hover_text(format!("Part of a mod whose scripts use {b}'s code. \
                                Left in place without {b}, redscript can't compile it, and that stops every script mod.")); }
                        }
                    });
                }
                if !mo2.is_empty() {
                    ui.horizontal(|ui| { ui.add_space(14.0); ui.weak(format!("Also in Mod Organizer 2: {}. Untick {} there.", mo2.join(", "), if mo2.len() == 1 { "it" } else { "them" })) });
                }
            });
            ui.separator();
            ui.horizontal(|ui| if rm.delete {
                ui.colored_label(BAD, format!("Delete the {ticked} ticked {} for good? This can't be undone.", if ticked == 1 { "item" } else { "items" }));
                if ui.add_enabled(ticked > 0, egui::Button::new(RichText::new("Delete for good").color(BAD))).clicked() { run = Some(true) }
                if ui.button("Back").clicked() { rm.delete = false }
            } else {
                if ui.add_enabled(ticked > 0, egui::Button::new(RichText::new("Back up and remove").strong())).clicked() { run = Some(false) }
                if ui.add_enabled(ticked > 0, egui::Button::new("Delete permanently…")).clicked() { rm.delete = true }
                if ui.button("Cancel").clicked() { close = true }
            });
        });
        let Some(delete) = run else { if !close && !modal.should_close() { self.removal = Some(rm) } return };
        let (mut done, mut failed) = (vec![], vec![]);
        for (name, why, entries, _) in &rm.mods {
            let paths: Vec<String> = entries.iter().filter(|e| e.2).map(|e| e.0.clone()).collect();
            if paths.is_empty() { continue }
            let r = if delete { remove::delete(&game, name, why, &paths) } else { remove::backup(&game, name, why, &paths).map(|_| ()) };
            match r { Ok(()) => done.push(name.as_str()), Err(e) => failed.push(format!("{name}: {e}")) }
        }
        let mut t = match (done.is_empty(), delete) {
            (true, _) => String::new(),
            (false, true) => format!("Deleted {}.", done.join(", ")),
            (false, false) => format!("Backed up and removed {} to {}. The Backups window can put {} back.", done.join(", "), game.join(remove::DIR).display(), if done.len() == 1 { "it" } else { "them" }),
        };
        for f in &failed { t += &format!("\n{f}") }
        self.notice = Some((failed.is_empty(), t.trim().to_string())).filter(|n| !n.1.is_empty());
        self.rescan();
    }

    /// Backups: restore a backed-up mod, or delete its backup.
    fn backups_window(&mut self, ctx: &egui::Context) {
        let (Some(list), Some(Ok(r))) = (&self.backups, &self.report) else { return };
        let game = r.game.clone();
        let (mut open, mut act) = (true, None);                 // act: (backup, restore?)
        egui::Window::new("Backups").open(&mut open).default_width(620.0).collapsible(false).show(ctx, |ui| {
            ui.weak(BACKUPS_INFO);
            if folder_row(ui, "Kept in", &game) { self.notice = open_folder(&game) }
            ui.separator();
            if list.is_empty() { ui.label("No backups."); }
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| for b in list {
                let asked = self.discard.as_ref() == Some(&b.dir);
                ui.horizontal(|ui| {
                    ui.weak(&b.date);
                    ui.label(RichText::new(&b.name).strong()).on_hover_text(format!("{}\n\n{}\n\nIn {}", b.why, b.paths.join("\n"), b.dir.display()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| if asked {
                        if ui.small_button("Keep").clicked() { self.discard = None }
                        if ui.small_button(RichText::new("Delete for good").color(BAD)).clicked() { act = Some((b.clone(), false)) }
                    } else {
                        if ui.small_button("Delete…").on_hover_text("Deletes this backup for good").clicked() { self.discard = Some(b.dir.clone()) }
                        if ui.small_button("Restore").on_hover_text("Moves its files back into the game folder").clicked() { act = Some((b.clone(), true)) }
                    });
                });
                if asked {
                    let n = b.paths.len();
                    ui.colored_label(BAD, if n == 1 { "Delete this backed-up item for good? This can't be undone.".into() } else { format!("Delete these {n} backed-up items for good? This can't be undone.") });
                    for p in &b.paths { ui.horizontal(|ui| { ui.add_space(14.0); ui.monospace(p) }); }
                }
            });
        });
        if let Some((b, restore)) = act {
            self.notice = Some(match if restore { remove::restore(&game, &b) } else { remove::discard(&game, &b) } {
                Ok(()) if restore => (true, format!("Restored {}.", b.name)),
                Ok(()) => (true, format!("Deleted the backup of {}.", b.name)),
                Err(e) => (false, e),
            });
            self.discard = None;
            let fresh = remove::list(&game);
            self.n_backups = fresh.len();
            self.backups = Some(fresh);
            if restore { self.rescan() }
        }
        if !open { self.backups = None; self.discard = None }
    }

    /// --launch: says why the game hasn't started yet, and lets the user start it anyway or not at all.
    fn launch_bar(&mut self, ui: &mut egui::Ui) {
        let n = match &self.report { Some(Ok(r)) => Some(reds(r)), _ => None };
        let text = match n {
                _ if self.job.is_some() => "Checking your mods before the game starts…".to_string(),
                Some(0) => "Nothing is likely to break now.".into(),
                Some(1) => "The game hasn't started: 1 mod is likely to break. Fix it below, or launch anyway.".into(),
                Some(n) => format!("The game hasn't started: {n} mods are likely to break. Fix them below, or launch anyway."),
                None => "The game hasn't started: your mods couldn't be checked. Choose your game folder, or launch anyway.".into(),
        };
        egui::Frame::new().fill(Color32::from_rgba_unmultiplied(220, 50, 50, 40)).inner_margin(8.0).corner_radius(4.0).show(ui, |ui| {
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 20.0), Layout::right_to_left(Align::Center), |ui| {   // buttons first, so a narrow window cuts the text
                if ui.button("Don't launch").clicked() { ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close) }
                if ui.add_enabled(self.job.is_none(), egui::Button::new(RichText::new(if n == Some(0) { "Launch the game" } else { "Launch anyway" }).strong())).clicked() { self.go(ui.ctx()) }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| ui.add(egui::Label::new(RichText::new(text).strong()).truncate()));
            });
        });
    }

    fn go(&self, ctx: &egui::Context) {
        if let Some(go) = &self.launch { go.store(true, Ordering::Relaxed) }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn start_page(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading("Choose your Cyberpunk 2077 folder to begin");
            ui.label("It's the folder that holds bin, archive and r6. Nothing in it is changed unless you ask.");
            ui.add_space(12.0);
            if ui.add(egui::Button::new(RichText::new("Browse…").size(18.0)).min_size(egui::vec2(160.0, 36.0))).clicked() { self.pick(false) }
            if let Some(Err(e)) = &self.report { ui.add_space(12.0); ui.colored_label(Color32::RED, e); }
        });
    }
}

fn exe_dir() -> Option<PathBuf> { env::current_exe().ok()?.parent().map(Path::to_path_buf) }

/// The backup folder's full path, with Open folder once it exists. Returns true when Open folder is pressed.
fn folder_row(ui: &mut egui::Ui, label: &str, game: &Path) -> bool {
    let d = game.join(remove::DIR);
    ui.horizontal(|ui| {
        ui.weak(label);
        let open = ui.add_enabled(remove::home(game).is_ok_and(|d| d.is_dir()), egui::Button::new("Open folder"))
            .on_disabled_hover_text("Made with the first backup").clicked();
        ui.add(egui::Label::new(RichText::new(d.display().to_string()).monospace()).truncate()).on_hover_text(d.display().to_string());
        open
    }).inner
}
/// Shows the backup folder in the system's file manager. Only ever that folder, and only if it's a plain folder.
fn open_folder(game: &Path) -> Option<(bool, String)> {
    let d = remove::home(game).ok().filter(|d| d.is_dir())?;
    let r = if cfg!(windows) { Command::new("explorer").arg(&d).spawn() } else { Command::new("xdg-open").arg(&d).spawn() };
    r.err().map(|e| (false, format!("Couldn't open {}: {e}", d.display())))
}
fn reds(r: &Report) -> usize { r.rows.iter().filter(|x| x.status == Status::Red).count() }

/// The game folder a launch command points into, e.g. Steam's `...\Cyberpunk 2077\REDprelauncher.exe`.
fn game_of(cmd: &[String]) -> Option<PathBuf> {
    cmd.iter().map(Path::new).find_map(|p| p.ancestors().find(|a| locate::is_game(a))).map(Path::to_path_buf)
}

/// Starts the game command and waits for it, so Steam sees the game running until it closes.
fn launch(cmd: &[String]) -> ! {
    match Command::new(&cmd[0]).args(&cmd[1..]).spawn() {
        Ok(mut c) => std::process::exit(c.wait().ok().and_then(|s| s.code()).unwrap_or(0)),
        Err(e) => {
            rfd::MessageDialog::new().set_title(TITLE).set_description(format!("The game couldn't be started: {e}\n\n{}", cmd.join(" "))).show();
            std::process::exit(1)
        }
    }
}

const OK: Color32 = Color32::from_rgb(40, 160, 70);
const BAD: Color32 = Color32::from_rgb(220, 60, 60);

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
    let mut clicked = false;                                     // the title opens and closes it too, not only the arrow
    let mut h = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(id), open).show_header(ui, |ui| {
        dot(ui, color, "");
        clicked = ui.add(egui::Label::new(RichText::new(title).strong()).selectable(false).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    });
    if clicked { h.toggle() }
    h.body(body);
    ui.add_space(4.0);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        if self.job.is_some() { ctx.request_repaint_after(Duration::from_millis(50)) }
        if self.auto && self.job.is_none() {                // --launch: start the game at once when nothing is red
            self.auto = false;
            if let Some(Ok(r)) = &self.report && reds(r) == 0 { self.go(ctx) }
        }
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
            if let Some(Ok(r)) = &self.report
                && ui.button(format!("Backups ({})", self.n_backups)).on_hover_text("Mods you backed up and removed").clicked() {
                self.backups = Some(remove::list(&r.game));
            }
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
                if let Some(Ok(Report { last: Some(l), .. })) = &self.report { ui.weak("·"); ui.weak(format!("Last launch {}", l.when)); }
                if self.job.is_some() { ui.spinner(); }
            });
            if let Some(w) = &self.warning { ui.colored_label(Color32::YELLOW, w); }
            if self.launch.is_some() { self.launch_bar(ui) }
            let mut seen = false;
            if let Some((ok, t)) = &self.notice {
                ui.horizontal(|ui| { ui.colored_label(if *ok { OK } else { BAD }, t); seen = ui.small_button("OK").clicked() });
            }
            if seen { self.notice = None }
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
        self.removal_dialog(ctx);
        self.backups_window(ctx);
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
    // --launch <game command>: Steam's launch options put the game's own command after it (%command%)
    let cmd = args.first().is_some_and(|a| a == "--launch").then(|| args[1..].to_vec()).filter(|c| !c.is_empty());
    let start = match &cmd { Some(c) => game_of(c), None => args.first().map(PathBuf::from).filter(|p| p.is_dir()) };
    let go = cmd.as_ref().map(|_| Arc::new(AtomicBool::new(false)));
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title(TITLE).with_inner_size([1100.0, 700.0]).with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    let flag = go.clone();
    eframe::run_native(TITLE, opts, Box::new(|cc| Ok(Box::new(App::new(cc, start, flag)))))?;
    if let (Some(c), Some(go)) = (cmd, go) && go.load(Ordering::Relaxed) { launch(&c) }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_game_in_a_launch_command() {
        let d = env::temp_dir().join(format!("cmcc-launch-{}", std::process::id()));
        std::fs::create_dir_all(d.join("bin/x64")).unwrap();
        std::fs::write(d.join("bin/x64/Cyberpunk2077.exe"), b"").unwrap();
        let s = |p: PathBuf| p.display().to_string();
        assert_eq!(game_of(&[s(d.join("REDprelauncher.exe"))]), Some(d.clone()));                    // Steam's launcher
        assert_eq!(game_of(&["proton".into(), "run".into(), s(d.join("bin/x64/Cyberpunk2077.exe"))]), Some(d.clone()));
        assert_eq!(game_of(&[s(env::temp_dir().join("elsewhere.exe"))]), None);
        std::fs::remove_dir_all(d).unwrap();
    }
}
