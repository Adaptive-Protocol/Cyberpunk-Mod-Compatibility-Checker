//! The compatibility manifest, the install scan and the verdicts.
//! Read-only: nothing here writes, moves or deletes a file.
use serde::Deserialize;
use std::{cmp::Ordering, collections::BTreeMap, fs, io::Read, path::{Path, PathBuf}, time::SystemTime};

pub const BUILT_IN: &str = include_str!("../compat.toml");

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub updated: String,
    #[serde(default)] pub build: Vec<Build>,
    #[serde(default)] pub framework: Vec<Entry>,
    #[serde(default, rename = "mod")] pub mods: Vec<Entry>,
    #[serde(default)] pub pattern: Vec<Pattern>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Build { pub game: String, #[serde(default)] pub confirmed: bool }

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub nexus_id: u32,
    pub detect: Vec<String>,
    #[serde(default)] pub parts: Vec<String>,          // other files it needs to load, such as its loader in bin/x64
    #[serde(default)] pub vcredist: Option<String>,    // lowest Microsoft Visual C++ runtime it runs on (Windows)
    #[serde(default)] pub build_locked: bool,
    #[serde(default)] pub min: BTreeMap<String, String>,
    #[serde(default)] pub needs: Vec<String>,
    #[serde(default)] pub notes: Vec<Note>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Note { pub build: String, pub text: String, pub source: String, pub date: String }

/// A known clash between files, such as an old version left behind next to its replacement: red when every path in
/// `all` is installed and none in `none` is.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub name: String,
    pub all: Vec<String>,
    #[serde(default)] pub none: Vec<String>,
    #[serde(default)] pub build: Option<String>,       // only on this game build
    #[serde(default)] pub nexus_id: Option<u32>,
    pub text: String,
    pub source: String,
    pub date: String,
}

/// The mod's Nexus page. Built from the numeric id, so a manifest can never point a link anywhere else.
pub fn nexus_url(id: u32) -> String { format!("https://www.nexusmods.com/cyberpunk2077/mods/{id}") }

/// A path that stays inside the game folder on every OS: relative, no `.` or `..`, no drive. `*` matches within a name.
fn inside(p: &str) -> bool {
    !p.starts_with(['/', '\\']) && p.split(['/', '\\']).all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains(':'))
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let m: Self = toml::from_str(text).map_err(|e| e.message().to_string())?;
        if m.schema != 1 { return Err(format!("unsupported schema {}", m.schema)) }
        let outside = |name: &str, p: &String| format!("{name}: path {p:?} must stay inside the game folder");
        let mut known: Vec<&str> = vec![];
        for (i, e) in m.framework.iter().chain(&m.mods).enumerate() {
            if e.nexus_id == 0 || e.detect.is_empty() { return Err(format!("{}: needs a nexus_id and a detect path", e.name)) }
            if let Some(p) = e.detect.iter().chain(&e.parts).find(|p| !inside(p)) { return Err(outside(&e.name, p)) }
            if let Some(n) = e.needs.iter().find(|n| !known.contains(&n.as_str())) {
                return Err(format!("{}: needs {n:?}, which isn't a framework listed above it", e.name))
            }
            if i < m.framework.len() { known.push(&e.name) }
        }
        for p in &m.pattern {
            if p.all.is_empty() || p.nexus_id == Some(0) { return Err(format!("{}: a pattern needs paths in `all` and a real nexus_id", p.name)) }
            if let Some(x) = p.all.iter().chain(&p.none).find(|x| !inside(x)) { return Err(outside(&p.name, x)) }
        }
        Ok(m)
    }

    /// The built-in list, or a valid `compat.toml` in `dir` with a later `updated` date.
    /// Returns the list, where it came from, and a warning when a copy in `dir` was ignored.
    pub fn load(dir: Option<&Path>) -> (Self, String, Option<String>) {
        let built_in = Self::parse(BUILT_IN).expect("built-in compat.toml is valid");
        let Some(path) = dir.map(|d| d.join("compat.toml")).filter(|p| p.is_file()) else { return (built_in, "built in".into(), None) };
        match read(&path, 4 << 20).ok_or_else(|| "unreadable".to_string()).and_then(|b| Self::parse(&String::from_utf8_lossy(&b))) {
            Ok(m) if m.updated > built_in.updated => (m, "from the compat.toml next to the app".into(), None),
            Ok(_) => (built_in, "built in".into(), Some(format!("{} is older than the built-in list, so it was ignored.", path.display()))),
            Err(e) => (built_in, "built in".into(), Some(format!("{} couldn't be read ({e}), so the built-in list is used.", path.display()))),
        }
    }
}

/// At most `cap` bytes of a file, so an oversized file can't exhaust memory.
fn read(path: &Path, cap: u64) -> Option<Vec<u8>> {
    let mut b = vec![];
    fs::File::open(path).ok()?.take(cap).read_to_end(&mut b).ok()?;
    Some(b)
}

/// "1.28.6.58207" -> [1, 28, 6, 58207]; anything that isn't a digit separates parts.
fn parts(v: &str) -> Vec<u64> { v.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect() }

/// Compares versions with missing parts read as 0, so "2.5" == "2.5.0.0".
pub fn cmp_ver(a: &str, b: &str) -> Ordering {
    let (a, b) = (parts(a), parts(b));
    (0..a.len().max(b.len())).map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0))).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

/// (file version, product version) from a Windows exe or dll's VS_FIXEDFILEINFO, as four numbers each.
pub fn pe_version(path: &Path) -> Option<([u64; 4], [u64; 4])> {
    const SIG: [u8; 8] = [0xBD, 0x04, 0xEF, 0xFE, 0, 0, 1, 0];   // signature + structure version 1.0
    let b = read(path, 512 << 20)?;
    let i = b.windows(8).position(|w| w == SIG)?;
    let d = |k: usize| b.get(i + 8 + 4 * k..i + 12 + 4 * k).map(|s| u64::from(u32::from_le_bytes(s.try_into().unwrap())));
    let four = |hi: u64, lo: u64| [hi >> 16, hi & 0xffff, lo >> 16, lo & 0xffff];
    Some((four(d(0)?, d(1)?), four(d(2)?, d(3)?)))
}

/// The game runtimes a RED4ext plugin was built for. Its Query() fills in PluginInfo::runtime, the game exe's file version
/// as four u16s, with a `mov reg, imm64`. Only values close to `game` count, which keeps unrelated constants out.
fn runtimes(b: &[u8], game: [u64; 4]) -> Vec<[u64; 4]> {
    let mut v: Vec<[u64; 4]> = b.windows(10).filter(|w| matches!(w[0], 0x48 | 0x49) && (0xB8..=0xBF).contains(&w[1])).map(|w| {
        let u = |i: usize| u64::from(u16::from_le_bytes([w[2 + 2 * i], w[3 + 2 * i]]));
        [u(0), u(1), u(2), u(3)]
    }).filter(|r| r[..2] == game[..2] && r[2].abs_diff(game[2]) <= 40 && r[3] >= 1000).collect();
    v.sort();
    v.dedup();
    v
}

/// [2, 5, 0, 0] -> "2.5"; keeps at least `min` parts. Mod versions drop the fourth (build) number.
fn show(v: [u64; 4], min: usize) -> String {
    let n = v.iter().rposition(|&x| x != 0).map_or(0, |i| i + 1).max(min);
    v[..n].iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

/// Does `name` match `pat`, where `*` stands for any run of characters? Case-insensitive.
fn wild(pat: &str, name: &str) -> bool {
    let (pat, name) = (pat.to_lowercase(), name.to_lowercase());
    let mut bits: Vec<&str> = pat.split('*').collect();
    let first = bits.remove(0);
    let Some(mut rest) = name.strip_prefix(first) else { return false };
    let Some(last) = bits.pop() else { return rest.is_empty() };
    for b in bits { match rest.find(b) { Some(i) => rest = &rest[i + b.len()..], None => return false } }
    rest.ends_with(last)
}

/// `rel` under `root`, matching names without regard to case (mods are made on Windows; Linux folders are case-sensitive).
fn resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        if part == "." || part == ".." { return None }
        let exact = p.join(part);
        p = if !part.contains('*') && exact.exists() { exact } else {
            fs::read_dir(&p).ok()?.flatten().map(|e| e.path()).find(|e| e.file_name().is_some_and(|n| wild(part, &n.to_string_lossy())))?
        };
    }
    Some(p)
}

fn files(dir: &Path) -> impl Iterator<Item = PathBuf> + use<> { fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) }
fn name(p: &Path) -> String { p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()) }

/// The RED4ext plugin or redscript mod folder a path points into: "red4ext/plugins/ArchiveXL/ArchiveXL.dll" -> "red4ext/plugins/archivexl".
fn mod_folder(d: &str) -> Option<String> {
    let s: Vec<String> = d.split(['/', '\\']).take(3).map(str::to_lowercase).collect();
    matches!(s.as_slice(), [a, b, _] if (a == "red4ext" && b == "plugins") || (a == "r6" && b == "scripts")).then(|| s.join("/"))
}

/// The newest time a folder or any file directly in it was modified or created. Logs don't count: mods write those as they run.
fn changed(p: &Path) -> Option<SystemTime> {
    let t = |m: fs::Metadata| [m.modified().ok(), m.created().ok()].into_iter().flatten().max();
    let own = files(p).filter(|f| !name(f).to_lowercase().contains(".log"));
    fs::metadata(p).ok().and_then(t).into_iter().chain(own.filter_map(|f| fs::metadata(f).ok()).filter_map(t)).max()
}

/// "red4ext-2026-11-04-03-19-14.log" -> ("red4ext", "20261104031914"): a log named `<who>-<date>-<time>.log`.
fn log_name(file: &str) -> Option<(&str, String)> {
    let s = file.strip_suffix(".log")?;
    let cut = s.len().checked_sub(20)?;
    let (who, ts) = (s.get(..cut)?, s.get(cut..)?.strip_prefix('-')?);
    let d: String = ts.chars().filter(char::is_ascii_digit).collect();
    (d.len() == 14 && ts.len() == 19 && !who.is_empty()).then_some((who, d))
}

/// "[ERROR - Wed, 4 Nov 2026 03:28:29 -0500] ..." -> "20261104032829", the local time a redscript log line was written.
fn redscript_stamp(l: &str) -> Option<String> {
    let w: Vec<&str> = l.strip_prefix('[')?.split(']').next()?.split(" - ").nth(1)?.split_whitespace().collect();
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| w.get(2).is_some_and(|x| x.eq_ignore_ascii_case(m)))? + 1;
    let time: String = w.get(4)?.chars().filter(char::is_ascii_digit).collect();
    Some(format!("{}{month:02}{:02}{time}", w.get(3)?, w.get(1)?.parse::<u32>().ok()?)).filter(|s| s.len() == 14)
}

/// "20261104031914" -> "2026-11-04 03:19"
fn when(d: &str) -> String { format!("{}-{}-{} {}:{}", &d[..4], &d[4..6], &d[6..8], &d[8..10], &d[10..12]) }

/// An error line of a spdlog-style mod log, "[time] [error] [thread] [Who] message", as (who, message).
fn error_line(l: &str) -> Option<(Option<&str>, &str)> {
    let (mut rest, mut error, mut who) = (l.trim_start(), false, None);
    while who.is_none() && let Some(r) = rest.strip_prefix('[') {
        let (tag, after) = r.split_once(']')?;
        let t = tag.trim();
        match t.to_ascii_lowercase().as_str() {
            "error" | "err" | "critical" | "fatal" => error = true,
            "info" | "warning" | "warn" | "debug" | "trace" => return None,
            _ if t.starts_with(|c: char| c.is_ascii_digit()) => {}   // time or thread id
            _ => who = Some(t),
        }
        rest = after.trim_start();
    }
    (error && !rest.is_empty()).then_some((who, rest))
}

/// A mod the game won't run: a plugin RED4ext refused or one built for another game runtime, or a script mod
/// redscript couldn't compile. `folder` is its folder, lowercase, such as "red4ext/plugins/archivexl".
#[derive(Clone, Debug)]
pub struct Refused { pub folder: String, pub name: String, pub version: Option<String>, pub why: String }

/// What the logs say about the last launch, read only when that launch ran this game exe.
#[derive(Debug, Default)]
pub struct Last {
    pub when: String,                         // local time from the log's name, e.g. "2026-11-04 03:19"
    pub refused: Vec<Refused>,                // plugins RED4ext refused to load
    pub errors: Vec<(String, String)>,        // (who, message) from the mods' own logs
    pub crash: Option<(String, String)>,      // (what Advanced Crash Reporter concluded, its report)
}

fn older(r: &str, exe: &str) -> &'static str { if cmp_ver(r, exe).is_lt() { "an older" } else { "a newer" } }

fn count(n: usize, noun: &str) -> String { format!("{n} {noun}{}", if n == 1 { "" } else { "s" }) }

/// Has the mod in `folder` been removed or changed since `at`, when a log reported it?
fn gone(roots: &[&Path], folder: &str, at: SystemTime) -> bool {
    roots.iter().find_map(|r| resolve(r, folder)).and_then(|p| changed(&p)).is_none_or(|t| t > at)
}

/// RED4ext's newest log (and redscript's, the mod logs and the crash report from the same launch), if that launch ran `exe`.
fn last_launch(roots: &[&Path], exe: &str) -> Option<Last> {
    let logs: Vec<PathBuf> = roots.iter().filter_map(|r| resolve(r, "red4ext/logs")).flat_map(|d| files(&d)).collect();
    let (log, stamp) = logs.iter().filter_map(|p| log_name(&name(p)).filter(|(w, _)| w.eq_ignore_ascii_case("red4ext")).map(|(_, d)| (p, d)))
        .max_by(|a, b| a.1.cmp(&b.1))?;
    let text = String::from_utf8_lossy(&read(log, 16 << 20)?).into_owned();
    if !text.lines().any(|l| l.split("File version: ").nth(1).is_some_and(|v| v.trim() == exe)) { return None }
    let at = fs::metadata(log).and_then(|m| m.modified()).ok()?;
    let mut last = Last { when: when(&stamp), ..Default::default() };

    let mut folder = String::new();
    for l in text.lines() {
        if let Some(p) = l.split("Loading plugin from '").nth(1) {
            let bits: Vec<&str> = p.split(['/', '\\']).collect();
            folder = bits.iter().rposition(|b| b.eq_ignore_ascii_case("plugins")).and_then(|i| bits.get(i + 1))
                .map_or(String::new(), |f| format!("red4ext/plugins/{}", f.trim_end_matches("'...").to_lowercase()));
        } else if let Some((who, tail)) = l.split("[RED4ext] ").nth(1).and_then(|r| r.split_once(" is incompatible with the current patch"))
            && let Some((name, version)) = who.rsplit_once(" (version: ") {
            let runtime = tail.split("runtime of the plugin is ").nth(1).map(str::trim).unwrap_or("");
            if !folder.is_empty() && !gone(roots, &folder, at) {
                last.refused.push(Refused { folder: folder.clone(), name: name.to_string(), version: Some(version.trim_end_matches(')').to_string()),
                    why: format!("RED4ext refused to load it at your last launch ({}): it's built for {} game version.", last.when, older(runtime, exe)) });
            }
        }
    }

    let rs = roots.iter().find_map(|r| resolve(r, "r6/logs/redscript_rCURRENT.log"));
    if let Some(rs) = rs && let Some(b) = read(&rs, 16 << 20) && let Ok(at) = fs::metadata(&rs).and_then(|m| m.modified()) {
        let t = String::from_utf8_lossy(&b);
        if t.lines().rev().find_map(redscript_stamp).is_some_and(|d| d >= stamp) {
            let lines: Vec<&str> = t.lines().collect();
            let mut failed: Vec<(String, String, usize, String)> = vec![];      // (folder, name, errors, first message)
            for (i, l) in lines.iter().enumerate().filter(|(_, l)| l.starts_with("[ERROR")) {
                let Some(path) = l.split(" At ").nth(1) else { continue };
                let bits: Vec<&str> = path.split(['/', '\\']).collect();
                let Some(f) = bits.iter().rposition(|b| b.eq_ignore_ascii_case("scripts")).and_then(|i| bits.get(i + 1)).filter(|f| !f.contains(".reds")) else { continue };
                let msg = lines.get(i + 3).filter(|_| lines.get(i + 2).is_some_and(|c| c.trim_start().starts_with('^'))).map_or("", |m| m.trim());
                match failed.iter_mut().find(|x| x.1 == *f) { Some(x) => x.2 += 1, None => failed.push((format!("r6/scripts/{}", f.to_lowercase()), f.to_string(), 1, msg.to_string())) }
            }
            failed.retain(|x| !gone(roots, &x.0, at));
            if !failed.is_empty() {
                last.errors.push(("redscript".into(), format!("Couldn't compile {}, so no script mod ran.", count(failed.len(), "mod"))));
            }
            for (folder, name, n, msg) in failed {
                last.refused.push(Refused { folder, name, version: None,
                    why: format!("redscript couldn't compile it at your last launch ({}), which stops every script mod: {} such as \"{msg}\". It likely needs an update for this game version.", last.when, count(n, "error")) });
            }
        }
    }

    let plugin_logs = roots.iter().filter_map(|r| resolve(r, "red4ext/plugins")).flat_map(|d| files(&d)).filter(|d| d.is_dir()).flat_map(|d| files(&d));
    let mut sources: Vec<PathBuf> = logs.iter().cloned().chain(plugin_logs).collect();
    sources.sort_by_key(|p| (log_name(&name(p)).is_none_or(|(w, _)| !w.eq_ignore_ascii_case("red4ext")), p.clone()));   // RED4ext's own first
    let mut per: BTreeMap<String, (usize, String)> = BTreeMap::new();   // log owner -> (errors, name as shown)
    for p in sources {
        let Some((owner, d)) = log_name(&name(&p)).map(|(w, d)| (w.to_string(), d)) else { continue };
        if d < stamp { continue }
        let Some(b) = read(&p, 16 << 20) else { continue };
        for l in String::from_utf8_lossy(&b).lines() {
            let Some((tag, msg)) = error_line(l) else { continue };
            // RED4ext copies the game's own crash report into its log, one error line per field: keep only its message
            if msg.trim_matches(['-', '=', ' ']).is_empty() || ["Crash report", "File:", "Line:", "Condition:"].iter().any(|s| msg.starts_with(s)) { continue }
            let msg = msg.strip_prefix("Message: ").map_or_else(|| msg.to_string(), |m| format!("The game's crash report says: {m}"));
            let (who, msg) = match tag {                                  // a log is its owner's, whatever part of it wrote the line
                Some(t) if t.eq_ignore_ascii_case(&owner) => (t.to_string(), msg),
                Some(t) => (owner.clone(), format!("[{t}] {msg}")),
                None => (owner.clone(), msg),
            };
            let n = per.entry(who.to_lowercase()).or_insert((0, who.clone()));
            n.0 += 1;
            if n.0 <= 3 && last.errors.len() < 30 && !last.errors.iter().any(|(w, m)| *w == who && *m == msg) { last.errors.push((who, msg)) }
        }
    }
    for (n, who) in per.into_values().filter(|(n, _)| *n > 3) { last.errors.push((who, format!("...and {} more in its log.", n - 3))) }

    let reports = roots.iter().find_map(|r| resolve(r, "bin/x64/plugins/AdvancedCrashReporter/reports"));
    let newest = reports.iter().flat_map(|d| files(d)).filter_map(|p| {
        let n = name(&p);
        let d: String = n.get(..15)?.chars().filter(char::is_ascii_digit).collect();
        (n.ends_with(".txt") && d.len() == 14 && d >= stamp).then_some((d, p))
    }).max();
    if let Some((_, p)) = newest && let Some(b) = read(&p, 4 << 20) {
        let t = String::from_utf8_lossy(&b);
        let field = |k: &str| t.lines().take(40).find_map(|l| l.strip_prefix(k)?.trim_start().strip_prefix(':').map(str::trim));
        if field("Game version").is_some_and(|v| v.contains(exe)) && let Some(v) = field("Verdict").or_else(|| field("Summary")) {
            let kind = if name(&p).contains("-hang-") { "froze" } else { "crashed" };
            let rel = roots.iter().find_map(|r| p.strip_prefix(r).ok()).unwrap_or(&p).display().to_string();
            let hooks = call_path_hooks(&t);
            let also = if hooks.is_empty() { String::new() } else { format!(" Also hooked in on the call path: {}.", hooks.join(", ")) };
            last.crash = Some((format!("Your last launch {kind}. Advanced Crash Reporter's verdict: {}.{also}", v.trim_end_matches('.')), rel));
        }
    }
    Some(last)
}

/// The mods Advanced Crash Reporter lists under "Mod hooks on the call path". They ran on the way to the crash, so one of
/// them can be the real cause when the verdict names a framework that many mods call into.
fn call_path_hooks(report: &str) -> Vec<String> {
    report.lines().skip_while(|l| !l.starts_with("Mod hooks on the call path")).skip(1).take_while(|l| !l.trim().is_empty())
        .filter_map(|l| l.trim().split_once('[').filter(|(_, tag)| tag.starts_with("MOD")).map(|(m, _)| m.trim()))
        .map(|m| m.rsplit_once('.').map_or(m, |(stem, _)| stem).to_string()).collect()
}

/// RED4ext plugins built for another game runtime than `exe`, so RED4ext won't load them. Read from the dlls themselves,
/// so this holds before the game is ever launched on a new build.
fn wrong_runtime(roots: &[&Path], exe: [u64; 4]) -> Vec<Refused> {
    let (e, mut out, mut seen) = (show(exe, 4), vec![], vec![]);
    for dir in roots.iter().filter_map(|r| resolve(r, "red4ext/plugins")) {
        for f in files(&dir) {
            let folder = format!("red4ext/plugins/{}", name(&f).to_lowercase());
            if seen.contains(&folder) { continue }                   // the first root (highest MO2 priority) wins
            seen.push(folder.clone());
            let dlls: Vec<PathBuf> = if f.is_dir() { files(&f).collect() } else { vec![f.clone()] };
            for dll in dlls.iter().filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll"))) {
                let Some(b) = read(dll, 64 << 20) else { continue };
                let r = runtimes(&b, exe);
                if r.is_empty() || r.contains(&exe) { continue }
                let r = show(*r.last().unwrap(), 4);
                out.push(Refused { folder: folder.clone(), name: name(&f), version: pe_version(dll).map(|(v, _)| show([v[0], v[1], v[2], 0], 3)),
                    why: format!("{} is built for {} game version (exe {r}, yours is {e}), so RED4ext won't load it. It needs an update.", name(dll), older(&r, &e)) });
                break;
            }
        }
    }
    out
}

/// The Microsoft Visual C++ runtime's version (msvcp140.dll in System32): None where it can't be checked (outside
/// Windows, or unreadable), Some(None) when it isn't installed.
fn vc_runtime() -> Option<Option<String>> {
    if !cfg!(windows) { return None }
    let dll = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("msvcp140.dll");
    if !dll.is_file() { return Some(None) }
    pe_version(&dll).map(|(f, _)| Some(show(f, 3)))
}

/// What the scan needs from outside the game folder.
pub struct Env {
    pub vc: Option<Option<String>>,     // see vc_runtime
    pub wine: Option<Vec<String>>,      // DLLs Wine loads from the game folder (Linux, Steam/Proton); None if unknown
}

impl Env {
    pub fn of(game: &Path) -> Self { Self { vc: vc_runtime(), wine: crate::locate::wine_overrides(game) } }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Status { Red, Gray, Green }

pub struct Row { pub name: String, pub nexus_id: Option<u32>, pub version: Option<String>, pub status: Status, pub why: String, pub notes: Vec<Note> }

pub struct Report {
    pub build: Option<String>,      // e.g. "2.5"
    pub exe: Option<String>,        // the exe's file version, e.g. "3.0.88.46235"
    pub banner: Option<(bool, String)>,   // (is a warning, text); a draft list is only a notice
    pub rows: Vec<Row>,             // sorted: red, gray, green, then by name
    pub unlisted: Vec<String>,      // native plugins the list doesn't know
    pub last: Option<Last>,         // what the logs say about the last launch
}

/// Checks the game folder, plus any Mod Organizer 2 mod folders (each laid over the game folder), against the manifest.
pub fn check(m: &Manifest, game: &Path, mo2: &[PathBuf]) -> Result<Report, String> { scan(m, game, mo2, &Env::of(game)) }

pub fn scan(m: &Manifest, game: &Path, mo2: &[PathBuf], env: &Env) -> Result<Report, String> {
    use Status::*;
    let exe = resolve(game, "bin/x64/Cyberpunk2077.exe")
        .ok_or_else(|| format!("Cyberpunk2077.exe wasn't found in {}. Choose the folder that holds bin, archive and r6.", game.display()))?;
    let vi = pe_version(&exe);
    let build = vi.map(|v| show(v.1, 2));
    let exe_v = vi.map(|v| show(v.0, 4));
    let roots: Vec<&Path> = mo2.iter().map(PathBuf::as_path).chain([game]).collect();
    let find = |rel: &str| roots.iter().find_map(|r| resolve(r, rel));
    let last = exe_v.as_deref().and_then(|e| last_launch(&roots, e));
    let mut refused = vi.map_or(vec![], |v| wrong_runtime(&roots, v.0));
    for r in last.iter().flat_map(|l| &l.refused) { if !refused.iter().any(|x| x.folder == r.folder) { refused.push(r.clone()) } }

    let entry = build.as_deref().and_then(|b| m.build.iter().find(|x| cmp_ver(&x.game, b).is_eq()));
    let latest = m.build.iter().map(|x| x.game.as_str()).max_by(|a, b| cmp_ver(a, b));
    let b = build.clone().unwrap_or_else(|| "unknown".into());
    let banner = match (entry, latest) {
        _ if build.is_none() => Some((true, "The game build couldn't be read from Cyberpunk2077.exe, so nothing can be confirmed.".to_string())),
        (Some(e), _) if !e.confirmed => Some((false, format!("The list for game {b} is a draft that hasn't passed a launch test yet, so nothing is marked OK."))),
        (Some(_), _) => None,
        (None, Some(l)) if cmp_ver(&b, l).is_gt() => Some((true, format!(
            "Your game ({b}) is newer than this compatibility list (up to {l}). Frameworks aren't confirmed for it yet, so hold off on launching with mods until the list is updated."))),
        (None, _) => Some((true, format!("This compatibility list doesn't cover game {b}, so nothing can be confirmed."))),
    };

    let mut state: BTreeMap<&str, Option<Status>> = BTreeMap::new();   // framework -> status, None = not installed
    let mut rows = vec![];
    let mut claimed: Vec<String> = vec![];                             // plugin folders that have a row
    for (i, e) in m.framework.iter().chain(&m.mods).enumerate() {
        let fw = i < m.framework.len();
        let Some(_) = e.detect.iter().find_map(|d| find(d)) else { if fw { state.insert(&e.name, None); } continue };
        let is_pe = |p: &PathBuf| p.extension().is_some_and(|x| ["dll", "exe", "asi"].iter().any(|e| x.eq_ignore_ascii_case(e)));
        let version = e.detect.first().and_then(|d| find(d)).filter(|p| p.is_file() && is_pe(p)).and_then(|p| pe_version(&p)).map(|(f, _)| show([f[0], f[1], f[2], 0], 3));
        let folders: Vec<String> = e.detect.iter().filter_map(|d| mod_folder(d)).collect();
        claimed.extend(folders.iter().cloned());
        let refusal = refused.iter().find(|r| folders.contains(&r.folder));
        let loader = |p: &String| p.split(['/', '\\']).collect::<Vec<_>>().as_slice().try_into().ok()
            .filter(|[a, b, _]: &[&str; 3]| a.eq_ignore_ascii_case("bin") && b.eq_ignore_ascii_case("x64"))
            .and_then(|[_, _, f]| f.to_lowercase().strip_suffix(".dll").map(str::to_string));
        let no_override = env.wine.as_ref().and_then(|w| e.parts.iter().filter_map(loader).find(|l| !w.contains(l)));
        let vc = e.vcredist.as_ref().zip(env.vc.as_ref()).filter(|(need, have)| have.as_ref().is_none_or(|h| cmp_ver(h, need).is_lt()));
        let need = |s: Option<Status>| e.needs.iter().find(|n| state.get(n.as_str()).copied().flatten() == s);
        let missing = e.needs.iter().find(|n| state.get(n.as_str()).is_some_and(Option::is_none));
        let notes: Vec<Note> = e.notes.iter().filter(|n| build.as_deref().is_some_and(|b| cmp_ver(&n.build, b).is_eq())).cloned().collect();
        let min = build.as_deref().and_then(|b| e.min.iter().find(|(k, _)| cmp_ver(k, b).is_eq())).map(|(_, v)| v);
        let (status, why) = if let Some(r) = refusal { (Red, r.why.clone()) }
            else if let Some(p) = e.parts.iter().find(|p| find(p).is_none()) { (Red, format!("{p} is missing, so it can't load. Reinstall {}.", e.name)) }
            else if let Some(l) = no_override { (Red, format!(
                "Wine loads its own {l}.dll unless told otherwise, so this won't load. Add the Steam launch option WINEDLLOVERRIDES=\"winmm,version=n,b\" %command%.")) }
            else if let Some((need, have)) = vc { (Red, match have {
                Some(h) => format!("Needs the Microsoft Visual C++ Redistributable (x64) {need} or newer, but this PC has {h}."),
                None => format!("Needs the Microsoft Visual C++ Redistributable (x64) {need} or newer, which isn't installed."),
            }) }
            else if !notes.is_empty() { (Red, format!("Known problem on game {b}.")) }
            else if let Some(n) = missing { (Red, format!("Needs {n}, which isn't installed.")) }
            else if let Some(n) = need(Some(Red)) { (Red, format!("Needs {n}, which is likely to break.")) }
            else if entry.is_none() && e.build_locked { (Red, format!("Each release is built for one game build, and this list has none for {b} yet. Expect it to need an update.")) }
            else if entry.is_none() { (Gray, format!("Not confirmed for game {b} yet.")) }
            else if let (Some(min), Some(v)) = (min, &version) && cmp_ver(v, min).is_lt() { (Red, format!("Version {v} is older than {min}, the lowest known to work on game {b}.")) }
            else if let (Some(min), None) = (min, &version) { (Gray, format!("Its version couldn't be read. It should be {min} or newer.")) }
            else if !entry.is_some_and(|x| x.confirmed) { (Gray, format!("Matches the list for game {b}, which isn't confirmed by a launch test yet.")) }
            else if let Some(n) = need(Some(Gray)) { (Gray, format!("Needs {n}, which isn't confirmed.")) }
            else { (Green, format!("OK for game {b}.")) };
        if fw { state.insert(&e.name, Some(status)); }
        rows.push(Row { name: e.name.clone(), nexus_id: Some(e.nexus_id), version, status, why, notes });
    }
    for f in &m.framework {                                  // missing frameworks matter only when something installed needs them
        let users: Vec<&str> = rows.iter().filter_map(|r| m.framework.iter().chain(&m.mods).find(|e| e.name == r.name))
            .filter(|e| e.needs.contains(&f.name)).map(|e| e.name.as_str()).collect();
        if state.get(f.name.as_str()).is_some_and(Option::is_none) && !users.is_empty() {
            rows.push(Row { name: f.name.clone(), nexus_id: Some(f.nexus_id), version: None, status: Red, notes: vec![],
                            why: format!("Not installed, but {} need{} it.", users.join(", "), if users.len() == 1 { "s" } else { "" }) });
        }
    }
    for r in refused.iter().filter(|r| !claimed.contains(&r.folder)) {   // unlisted plugins RED4ext won't load
        rows.push(Row { name: r.name.clone(), nexus_id: None, version: r.version.clone(), status: Red, why: r.why.clone(), notes: vec![] });
    }
    for p in m.pattern.iter().filter(|p| p.build.as_deref().is_none_or(|pb| build.as_deref().is_some_and(|b| cmp_ver(pb, b).is_eq()))) {
        if p.all.iter().all(|x| find(x).is_some()) && !p.none.iter().any(|x| find(x).is_some()) {
            rows.push(Row { name: p.name.clone(), nexus_id: p.nexus_id, version: None, status: Red, notes: vec![],
                            why: format!("{} ({}, {})", p.text, p.source, p.date) });
        }
    }
    rows.sort_by_key(|r| (r.status, r.name.to_lowercase()));

    let listed = |p: &Path| m.framework.iter().chain(&m.mods).flat_map(|e| &e.detect).any(|d| {
        let d = d.replace('\\', "/").to_lowercase();
        roots.iter().any(|r| p.strip_prefix(r).is_ok_and(|rel| { let rel = rel.to_string_lossy().replace('\\', "/").to_lowercase(); d == rel || d.starts_with(&format!("{rel}/")) }))
    });
    let mut unlisted: Vec<String> = roots.iter().flat_map(|r| {
        let dirs = resolve(r, "red4ext/plugins").into_iter().flat_map(|p| files(&p)).filter(|p| p.is_dir());
        let asi = resolve(r, "bin/x64/plugins").into_iter().flat_map(|p| files(&p))
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("asi")));
        dirs.chain(asi).collect::<Vec<_>>()
    }).filter(|p| !listed(p)).map(|p| name(&p)).filter(|n| !rows.iter().any(|r| r.status == Red && r.name.eq_ignore_ascii_case(n))
        && !refused.iter().any(|r| r.folder == format!("red4ext/plugins/{}", n.to_lowercase()))).collect();
    unlisted.sort_by_key(|n| n.to_lowercase());
    unlisted.dedup();
    Ok(Report { build, exe: exe_v, banner, rows, unlisted, last })
}

/// Plain-text report, for `--report` and the tests.
pub fn text(r: &Report, source: &str, updated: &str) -> String {
    let mut s = format!("Game build {} (exe {}). Compatibility list {updated}, {source}.\n",
                        r.build.as_deref().unwrap_or("unknown"), r.exe.as_deref().unwrap_or("unknown"));
    if let Some((_, b)) = &r.banner { s += &format!("! {b}\n") }
    if let Some(l) = &r.last {
        s += &format!("Last launch: {}\n", l.when);
        if let Some((c, p)) = &l.crash { s += &format!("! {c} ({p})\n") }
        for (w, m) in &l.errors { s += &format!("\terror: {w}: {m}\n") }
    }
    for x in &r.rows {
        s += &format!("{:?}\t{}\t{}\t{}\t{}\n", x.status, x.name, x.version.as_deref().unwrap_or("-"), x.why, x.nexus_id.map_or("-".into(), nexus_url));
        for n in &x.notes { s += &format!("\tnote: {} ({}, {})\n", n.text, n.source, n.date) }
    }
    if !r.unlisted.is_empty() { s += &format!("Unlisted native plugins: {}\n", r.unlisted.join(", ")) }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A fake game folder: a Cyberpunk2077.exe reporting `game`, plus `files` (path, file version or None).
    fn install(name: &str, game: [u16; 4], files: &[(&str, Option<[u16; 4]>)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ccc-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let pe = |v: [u16; 4], p: [u16; 4]| {
            let w = |a: [u16; 4], i: usize| ((u32::from(a[i]) << 16) | u32::from(a[i + 1])).to_le_bytes();
            [&b"MZ junk "[..], &[0xBD, 0x04, 0xEF, 0xFE, 0, 0, 1, 0], &w(v, 0), &w(v, 2), &w(p, 0), &w(p, 2)].concat()
        };
        for (rel, v, p) in [("bin/x64/Cyberpunk2077.exe", Some([3, 0, 88, 46235]), game)].into_iter().chain(files.iter().map(|&(r, v)| (r, v, [0; 4]))) {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, v.map_or(vec![], |v| pe(v, p))).unwrap();
        }
        root
    }

    /// A RED4ext plugin whose Query() sets PluginInfo::runtime to `rt` (mov rax, imm64), as compilers emit it.
    fn plugin(rt: [u16; 4]) -> Vec<u8> {
        let mut b = b"MZ code \x48\xB8".to_vec();
        b.extend(rt.iter().flat_map(|x| x.to_le_bytes()));
        b.extend(b"\x48\x89\x41\x30 \x48\xB8\x03\x00\x00\x00\x26\x00\x00\x00");   // + an unrelated constant
        b
    }

    const LIST: &str = r#"
        schema = 1
        updated = "2026-11-04"
        build = [{ game = "2.5", confirmed = true }]
        [[framework]]
        name = "RED4ext"
        nexus_id = 2380
        detect = ["red4ext/RED4ext.dll"]
        min = { "2.5" = "1.31.0" }
        [[framework]]
        name = "CET"
        nexus_id = 107
        detect = ["bin/x64/plugins/cyber_engine_tweaks.asi"]
        build_locked = true
        [[framework]]
        name = "ArchiveXL"
        nexus_id = 4198
        detect = ["red4ext/plugins/ArchiveXL/ArchiveXL.dll"]
        needs = ["RED4ext"]
        [[mod]]
        name = "Equipment-EX"
        nexus_id = 6945
        detect = ["r6/scripts/EquipmentEx"]
        needs = ["ArchiveXL"]
        [[mod]]
        name = "Rain"
        nexus_id = 1
        detect = ["archive/pc/mod/rain.archive"]
        notes = [{ build = "2.5", text = "Flickers at night", source = "Nexus comments", date = "2026-11-04" }]
    "#;

    const NO_ENV: Env = Env { vc: None, wine: None };

    fn run(list: &str, game: [u16; 4], files: &[(&str, Option<[u16; 4]>)], name: &str, env: &Env) -> (Report, Vec<(String, Status)>) {
        let r = scan(&Manifest::parse(list).unwrap(), &install(name, game, files), &[], env).unwrap();
        let v = r.rows.iter().map(|x| (x.name.clone(), x.status)).collect();
        (r, v)
    }

    fn verdicts(game: [u16; 4], files: &[(&str, Option<[u16; 4]>)], name: &str) -> (Report, Vec<(String, Status)>) { run(LIST, game, files, name, &NO_ENV) }

    #[test]
    fn versions() {
        assert!(cmp_ver("2.5", "2.5.0.0").is_eq() && cmp_ver("1.28.6.58207", "1.28.6").is_gt() && cmp_ver("1.9", "1.10").is_lt());
        assert_eq!(show([2, 5, 0, 0], 2), "2.5");
        assert_eq!(show([1, 31, 0, 0], 3), "1.31.0");
    }

    #[test]
    fn built_in_list_is_valid() { Manifest::parse(BUILT_IN).unwrap(); }

    #[test]
    fn confirmed_build() {
        let (r, v) = verdicts([2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("bin/x64/plugins/cyber_engine_tweaks.asi", Some([1, 38, 1, 0])),
                                             ("R6/Scripts/equipmentex/x.reds", None), ("archive/pc/mod/rain.archive", None), ("red4ext/plugins/Other/o.dll", None)], "ok");
        assert_eq!(r.build.as_deref(), Some("2.5"));
        assert!(r.banner.is_none() && r.last.is_none());
        assert_eq!(v, [("ArchiveXL".into(), Status::Red), ("Equipment-EX".into(), Status::Red), ("Rain".into(), Status::Red),
                       ("CET".into(), Status::Green), ("RED4ext".into(), Status::Green)]);
        assert!(r.rows[0].why.contains("Equipment-EX"), "missing ArchiveXL names who needs it");
        assert_eq!(r.rows[2].notes.len(), 1);
        assert_eq!(r.unlisted, ["Other"]);
    }

    #[test]
    fn old_framework_is_red() {
        let (_, v) = verdicts([2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 30, 0, 0])), ("red4ext/plugins/ArchiveXL/ArchiveXL.dll", Some([1, 28, 6, 0]))], "old");
        assert_eq!(v, [("ArchiveXL".into(), Status::Red), ("RED4ext".into(), Status::Red)]);
    }

    #[test]
    fn newer_game_than_list() {
        let (r, v) = verdicts([2, 6, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("bin/x64/plugins/cyber_engine_tweaks.asi", Some([1, 38, 1, 0]))], "new");
        assert!(r.banner.unwrap().1.contains("newer than this compatibility list"));
        assert_eq!(v, [("CET".into(), Status::Red), ("RED4ext".into(), Status::Gray)]);
    }

    #[test]
    fn mo2_folders_count_as_installed() {
        let game = install("mo2game", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0]))]);
        let m = install("mo2mod", [0; 4], &[("red4ext/plugins/ArchiveXL/ArchiveXL.dll", Some([1, 28, 6, 0]))]);
        let r = scan(&Manifest::parse(LIST).unwrap(), &game, &[m], &NO_ENV).unwrap();
        assert!(r.rows.iter().any(|x| x.name == "ArchiveXL" && x.status == Status::Green));
    }

    #[test]
    fn rejects_unsafe_or_broken_lists() {
        for bad in ["../../outside", r"..\\..\\outside", "/etc/passwd", "C:/Windows", "r6//x", "./r6"] {   // the raw one is ..\..\outside once TOML unescapes it
            assert!(Manifest::parse(&LIST.replace("r6/scripts/EquipmentEx", bad)).unwrap_err().contains("inside the game folder"), "{bad}");
        }
        assert!(Manifest::parse(&LIST.replace(r#"needs = ["RED4ext"]"#, r#"needs = ["Nope"]"#)).is_err());
        assert!(Manifest::parse(&LIST.replace("schema = 1", "schema = 1\nurl = \"https://example.com\"")).is_err());
        let pattern = |all: &str| format!("{LIST}\n[[pattern]]\nname = \"P\"\nall = [{all}]\ntext = \"t\"\nsource = \"s\"\ndate = \"d\"\n");
        assert!(Manifest::parse(&pattern(r#""archive/pc/mod/a*.archive""#)).is_ok());
        assert!(Manifest::parse(&pattern(r#""../a""#)).unwrap_err().contains("inside the game folder"));
        assert!(Manifest::parse(&pattern("")).is_err());
    }

    #[test]
    fn wildcards() {
        assert!(wild("###-Glass*.archive", "###-GlassFix.ARCHIVE") && wild("a*b*c", "aXbYc") && wild("abc", "ABC"));
        assert!(!wild("a*b", "ac") && !wild("abc", "abcd") && !wild("*x", "y"));
    }

    #[test]
    fn plugins_built_for_another_runtime() {
        assert_eq!(runtimes(&plugin([3, 0, 80, 51928]), [3, 0, 88, 46235]), [[3, 0, 80, 51928]]);
        assert!(runtimes(b"\x48\xB8\x03\x00\x00\x00\x26\x00\x00\x00 \x48\xB8\x03\x00\x00\x00\x00\x80\x01\x00", [3, 0, 88, 46235]).is_empty());
        let root = install("runtime", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("r6/scripts/EquipmentEx/x.reds", None)]);
        for (rel, rt) in [("red4ext/plugins/ArchiveXL/ArchiveXL.dll", [3, 0, 80, 51928]), ("red4ext/plugins/Fresh/f.dll", [3, 0, 88, 46235]),
                          ("red4ext/plugins/Stale/s.dll", [3, 0, 80, 51928])] {
            fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
            fs::write(root.join(rel), plugin(rt)).unwrap();
        }
        let r = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap();
        let red: Vec<_> = r.rows.iter().filter(|x| x.status == Status::Red).map(|x| (x.name.as_str(), x.nexus_id)).collect();
        assert_eq!(red, [("ArchiveXL", Some(4198)), ("Equipment-EX", Some(6945)), ("Stale", None)]);
        assert!(r.rows[0].why.contains("older game version (exe 3.0.80.51928, yours is 3.0.88.46235)"), "{}", r.rows[0].why);
        assert_eq!(r.unlisted, ["Fresh"]);
    }

    #[test]
    fn loaders_runtime_and_wine() {
        let list = LIST.replace(r#"detect = ["red4ext/RED4ext.dll"]"#, "detect = [\"red4ext/RED4ext.dll\"]\nparts = [\"bin/x64/winmm.dll\"]")
            .replace("build_locked = true", "build_locked = true\nparts = [\"bin/x64/version.dll\"]\nvcredist = \"14.40.33810\"");
        let files = [("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("bin/x64/plugins/cyber_engine_tweaks.asi", Some([1, 38, 1, 0])),
                     ("bin/x64/version.dll", None), ("red4ext/plugins/ArchiveXL/ArchiveXL.dll", None)];
        let status = |env: &Env, files: &[(&str, Option<[u16; 4]>)], who: &str| {
            let (r, _) = run(&list, [2, 5, 0, 0], files, "loaders", env);
            r.rows.iter().find(|x| x.name == who).map(|x| (x.status, x.why.clone())).unwrap()
        };
        let (s, why) = status(&NO_ENV, &files, "RED4ext");
        assert_eq!(s, Status::Red);
        assert!(why.starts_with("bin/x64/winmm.dll is missing"), "{why}");
        assert!(status(&NO_ENV, &files, "ArchiveXL").1.contains("Needs RED4ext, which is likely to break"));
        let all = [&files[..], &[("bin/x64/winmm.dll", None)]].concat();
        assert_eq!(status(&NO_ENV, &all, "RED4ext").0, Status::Green);
        let old_vc = Env { vc: Some(Some("14.38.33135".into())), wine: None };
        assert!(status(&old_vc, &all, "CET").1.contains("this PC has 14.38.33135"));
        assert!(status(&Env { vc: Some(None), wine: None }, &all, "CET").1.contains("isn't installed"));
        assert_eq!(status(&Env { vc: Some(Some("14.42.34433".into())), wine: None }, &all, "CET").0, Status::Green);
        let wine = |w: &[&str]| Env { vc: None, wine: Some(w.iter().map(|s| s.to_string()).collect()) };
        assert!(status(&wine(&["winmm"]), &all, "CET").1.contains("its own version.dll"));
        assert_eq!(status(&wine(&["winmm", "version"]), &all, "CET").0, Status::Green);
    }

    #[test]
    fn last_launch_logs() {
        let root = install("last", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("red4ext/plugins/ArchiveXL/ArchiveXL.dll", None),
                                                  ("red4ext/plugins/Gone/g.dll", None), ("red4ext/plugins/Old/o.dll", None)]);
        let logs = root.join("red4ext/logs");
        fs::create_dir_all(&logs).unwrap();
        let p = "D:\\Games\\Cyberpunk 2077\\red4ext\\plugins";
        fs::write(logs.join("red4ext-2026-11-04-03-05-54.log"), format!(
            "[2026-11-04 03:05:54.374] [info    ] [ 1] [RED4ext] File version: 3.0.88.46235\n\
             [2026-11-04 03:05:55.900] [info    ] [ 1] [RED4ext] Loading plugin from '{p}\\Old\\o.dll'...\n\
             [2026-11-04 03:06:08.367] [warning ] [ 1] [RED4ext] Old Plugin (version: 1.0.0) is incompatible with the current patch. The requested runtime of the plugin is 3.0.80.51928\n\
             [2026-11-04 03:06:08.400] [info    ] [ 1] [RED4ext] Loading plugin from '{p}\\Moved\\m.dll'...\n\
             [2026-11-04 03:06:08.401] [warning ] [ 1] [RED4ext] Moved (version: 2.0.0) is incompatible with the current patch. The requested runtime of the plugin is 3.0.80.51928\n\
             [2026-11-04 03:08:04.551] [error   ] [ 1] [RED4ext] Crash report\n[2026-11-04 03:08:04.551] [error   ] [ 1] [RED4ext] ------------\n\
             [2026-11-04 03:08:04.551] [error   ] [ 1] [RED4ext] File: E:\\src\\engineWatchdog.cpp\n[2026-11-04 03:08:04.551] [error   ] [ 1] [RED4ext] Line: 224\n\
             [2026-11-04 03:08:04.551] [error   ] [ 1] [RED4ext] Message: Watchdog timeout! (120 seconds)\n")).unwrap();
        fs::write(logs.join("red4ext-2026-11-03-20-00-00.log"), "[RED4ext] File version: 3.0.88.46235\n").unwrap();
        fs::write(logs.join("glassfixnative-2026-11-04-03-06-07.log"), "[2026-11-04 03:06:07.036] [error   ] [ 1] [GlassFixNative] Glass Fix 1.x is still installed.\n\
            [2026-11-04 03:06:07.036] [info    ] [ 1] [GlassFixNative] fine\n").unwrap();
        fs::write(logs.join("oldmod-2026-11-03-20-00-01.log"), "[x] [error] [OldMod] from an earlier launch\n").unwrap();
        let axl = (1..=4).map(|i| format!("[2026-11-04 03:10:46.000] [37264] [error] [ResourceManager] Resource {i} is missing.\n")).collect::<String>();
        fs::write(root.join("red4ext/plugins/ArchiveXL/ArchiveXL-2026-11-04-03-10-45.log"), format!("[2026-11-04 03:10:45.789] [37264] [error] Can't patch.\n{axl}")).unwrap();
        let reports = root.join("bin/x64/plugins/AdvancedCrashReporter/reports");
        fs::create_dir_all(&reports).unwrap();
        fs::write(reports.join("20261104-030610-crash-tid1-01.txt"), "# REPORT\nGame version   : 2.5 (3.0.88.46235, built 2026-10-19)\nVerdict        : Likely culprit: ArchiveXL\n\n\
            Mod hooks on the call path (NOT running):\n  HookMod.dll                   [MOD-RED4EXT ] frame #11\n    in red4ext/plugins/HookMod\n\
              RED4ext.dll                    [LOADER      ] frame #30\n  -> These intercept an engine function [further up]\n\n\
            Mods present on OTHER threads (NOT evidence):\n  audioware.dll                  [MOD-RED4EXT ] 35 threads\n").unwrap();
        fs::write(reports.join("20261103-200000-crash-tid1-01.txt"), "Game version   : 2.5 (3.0.88.46235)\nVerdict        : Likely culprit: Older\n").unwrap();
        let r = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap();
        let l = r.last.as_ref().unwrap();
        assert_eq!(l.when, "2026-11-04 03:05");
        assert_eq!(l.refused.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Old Plugin"], "a plugin no longer installed is left out");
        let e = |w: &str, m: &str| (w.to_string(), m.to_string());
        assert_eq!(l.errors, [e("RED4ext", "The game's crash report says: Watchdog timeout! (120 seconds)"), e("GlassFixNative", "Glass Fix 1.x is still installed."),
                              e("ArchiveXL", "Can't patch."), e("ArchiveXL", "[ResourceManager] Resource 1 is missing."), e("ArchiveXL", "[ResourceManager] Resource 2 is missing."),
                              e("ArchiveXL", "...and 2 more in its log.")]);
        let (c, report) = l.crash.as_ref().unwrap();
        assert!(c.contains("crashed") && c.contains("Likely culprit: ArchiveXL.") && report.ends_with("20261104-030610-crash-tid1-01.txt"), "{c} {report}");
        assert!(c.ends_with("Also hooked in on the call path: HookMod."), "{c}");
        let old = r.rows.iter().find(|x| x.name == "Old Plugin").unwrap();
        assert!(old.status == Status::Red && old.nexus_id.is_none() && old.why.contains("last launch (2026-11-04 03:05)") && old.why.contains("an older game"));
        assert!(!r.unlisted.contains(&"Old".to_string()) && r.unlisted.contains(&"Gone".to_string()));

        let o = root.join("red4ext/plugins/Old/o.dll");                    // updated since that launch: no longer reported
        let log = fs::File::options().write(true).open(logs.join("red4ext-2026-11-04-03-05-54.log")).unwrap();
        log.set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
        fs::write(&o, b"MZ new").unwrap();
        assert!(scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap().last.unwrap().refused.is_empty());

        let other = install("last-other", [2, 5, 0, 0], &[]);                  // a log from another game exe is ignored
        fs::create_dir_all(other.join("red4ext/logs")).unwrap();
        fs::write(other.join("red4ext/logs/red4ext-2026-11-04-03-05-54.log"), "[RED4ext] File version: 3.0.80.51928\n").unwrap();
        assert!(scan(&Manifest::parse(LIST).unwrap(), &other, &[], &NO_ENV).unwrap().last.is_none());
    }

    #[test]
    fn redscript_errors() {
        let root = install("redscript", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("r6/scripts/ScriptMod/a.reds", None), ("r6/scripts/Fine/f.reds", None)]);
        fs::create_dir_all(root.join("red4ext/logs")).unwrap();
        fs::write(root.join("red4ext/logs/red4ext-2026-11-04-03-27-39.log"), "[RED4ext] File version: 3.0.88.46235\n").unwrap();
        fs::create_dir_all(root.join("r6/logs")).unwrap();
        let err = |m: &str, msg: &str| format!("[ERROR - Wed, 4 Nov 2026 03:28:29 -0500] [UNRESOLVED_METHOD] At D:\\Games\\Cyberpunk 2077\\r6\\scripts\\{m}\\x.reds:3:1:\n\
            @wrapMethod(X)\n^^^^^^^^^^^^^^\n{msg}\n\n");
        let log = format!("[INFO - Wed, 4 Nov 2026 03:28:26 -0500] Compiling files in D:\\Games\\Cyberpunk 2077\\r6\\scripts:\nScriptMod\\a.reds\n{}{}{}",
                          err("ScriptMod", "no method with this name exists on the target type"), err("ScriptMod", "another"), err("Removed", "gone since"));
        fs::write(root.join("r6/logs/redscript_rCURRENT.log"), &log).unwrap();
        let r = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap();
        assert_eq!(r.last.as_ref().unwrap().errors, [("redscript".to_string(), "Couldn't compile 1 mod, so no script mod ran.".to_string())]);
        let w = r.rows.iter().find(|x| x.name == "ScriptMod").unwrap();
        assert!(w.status == Status::Red && w.nexus_id.is_none() && w.why.contains("2 errors such as \"no method with this name exists on the target type\""), "{}", w.why);
        assert!(!r.rows.iter().any(|x| x.name == "Removed" || x.name == "Fine"));
        fs::write(root.join("r6/logs/redscript_rCURRENT.log"), log.replace("03:28:", "02:28:")).unwrap();   // from before that launch
        assert!(scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap().last.unwrap().errors.is_empty());
        assert_eq!(redscript_stamp("[WARN - Sat, 31 Oct 2026 23:59:01 +0100] x"), Some("20261031235901".into()));
        assert_eq!(redscript_stamp("[ERROR] no date"), None);
    }

    #[test]
    fn patterns() {
        let list = format!("{LIST}\n[[pattern]]\nname = \"Glass Fix 1.x left behind\"\nall = [\"red4ext/plugins/GlassFix\", \"archive/pc/mod/###-Glass*.archive\"]\n\
            none = [\"archive/pc/mod/keep.txt\"]\nbuild = \"2.5\"\ntext = \"Delete the old archive.\"\nsource = \"its log\"\ndate = \"2026-11-04\"\n");
        let both = [("red4ext/plugins/GlassFix/b.dll", None), ("archive/pc/mod/###-GlassFix.archive", None)];
        let hit = |files: &[(&str, Option<[u16; 4]>)], game: [u16; 4]| run(&list, game, files, "pattern", &NO_ENV).0.rows.iter().any(|x| x.name.starts_with("Glass Fix"));
        assert!(hit(&both, [2, 5, 0, 0]));
        assert!(!hit(&both[..1], [2, 5, 0, 0]));
        assert!(!hit(&[&both[..], &[("archive/pc/mod/keep.txt", None)]].concat(), [2, 5, 0, 0]));
        assert!(!hit(&both, [2, 6, 0, 0]), "a pattern for 2.5 doesn't apply to 2.6");
    }

    #[test]
    fn log_parsing() {
        assert_eq!(log_name("red4ext-2026-11-04-03-19-14.log"), Some(("red4ext", "20261104031914".into())));
        assert_eq!(log_name("ArchiveXL-2026-11-04-03-10-45.1.log"), None);
        assert_eq!(log_name("input_loader.log"), None);
        assert_eq!(log_name("é-2026-11-04-03-19-14.log").map(|x| x.0), Some("é"));
        assert_eq!(error_line("[2026-11-04 03:06:10.124] [error   ] [ 60624] [REDGraph] [crash] thread 1"), Some((Some("REDGraph"), "[crash] thread 1")));
        assert_eq!(error_line("[2026-11-04 02:57:37.735] [error   ] [ 75516] [NCA Wardrobe Guard] guard disabled"), Some((Some("NCA Wardrobe Guard"), "guard disabled")));
        assert_eq!(error_line("[2026-11-04 03:10:45.789] [37264] [info] ArchiveXL 1.28.6 is starting..."), None);
        assert_eq!(error_line("no brackets [error] here"), None);
    }
}
