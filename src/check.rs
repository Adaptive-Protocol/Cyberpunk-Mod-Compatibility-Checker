//! The compatibility manifest, the install scan and the verdicts.
//! Read-only: nothing here writes, moves or deletes a file (that's remove.rs, and only when the user asks).
use serde::Deserialize;
use std::{cell::OnceCell, cmp::Ordering, collections::{BTreeMap, HashSet}, fs, io::Read, path::{Path, PathBuf}, time::SystemTime};

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
    #[serde(default)] pub remove: Vec<String>,         // the leftover files Back up and remove offers to take out
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
            if let Some(x) = p.all.iter().chain(&p.none).chain(&p.remove).find(|x| !inside(x)) { return Err(outside(&p.name, x)) }
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

/// Folders that hold one mod per entry (a folder, or a file such as an .archive). Back up and remove only ever moves
/// a whole entry of one of these, so the game's own files can't be touched.
pub const MOD_DIRS: [&str; 6] = ["bin/x64/plugins/cyber_engine_tweaks/mods", "red4ext/plugins", "r6/scripts", "r6/tweaks", "archive/pc/mod", "mods"];

/// The whole mod entry `rel` points into, as written: "r6/scripts/X/a.reds" -> "r6/scripts/X". An .asi in bin/x64/plugins
/// is an entry of its own. None outside the mod folders.
pub fn unit(rel: &str) -> Option<String> {
    let rel = rel.replace('\\', "/");
    let low = rel.to_ascii_lowercase();                        // same byte offsets as `rel`
    if let Some(f) = low.strip_prefix("bin/x64/plugins/") && !f.contains('/') && f.len() > 4 && f.ends_with(".asi") { return Some(rel) }
    MOD_DIRS.iter().find_map(|d| {
        let first = low.strip_prefix(d)?.strip_prefix('/')?.split('/').next().filter(|f| !f.is_empty())?;
        Some(rel[..d.len() + 1 + first.len()].to_string())
    })
}

/// The name a mod entry goes by, for finding its other parts: "archive/pc/mod/X.archive.xl" and "r6/scripts/X" -> "x".
fn key(unit: &str) -> String { unit.rsplit('/').next().unwrap_or("").split('.').next().unwrap_or("").to_lowercase() }

fn files(dir: &Path) -> impl Iterator<Item = PathBuf> + use<> { fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) }
fn name(p: &Path) -> String { p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()) }

/// A redscript file, for finding the add-ons that build on a mod: its root and **Mod entry**, the module it declares,
/// the modules it has to import (not those under `@if(ModuleExists(..))`), the classes and functions it defines, and
/// every name its code uses.
struct Script { root: usize, unit: String, module: Option<String>, imports: Vec<String>, defs: Vec<String>, names: HashSet<String> }

/// A .reds file's (module, imports, defs, names). Comments and strings are blanked first, so an `import` or a name
/// inside them doesn't count.
fn reds(t: &str) -> (Option<String>, Vec<String>, Vec<String>, HashSet<String>) {
    let (mut code, mut it) = (String::with_capacity(t.len()), t.trim_start_matches('\u{feff}').chars().peekable());
    while let Some(c) = it.next() {
        match (c, it.peek()) {
            ('/', Some('/')) => { it.by_ref().find(|&c| c == '\n'); code.push('\n') }
            ('/', Some('*')) => {
                let mut star = false;
                it.next();
                for c in it.by_ref() {
                    if c == '\n' { code.push('\n') }
                    if star && c == '/' { break }
                    star = c == '*';
                }
            }
            ('"', _) => {
                let mut esc = false;
                for c in it.by_ref() {
                    if c == '\n' { code.push('\n'); break }
                    if c == '"' && !esc { break }
                    esc = c == '\\' && !esc;
                }
                code.push(' ');
            }
            _ => code.push(c),
        }
    }
    let path = |s: &str| s.trim_start().split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).next().unwrap_or("").trim_end_matches('.').to_string();
    let (mut module, mut imports, mut guarded) = (None, vec![], false);
    for l in code.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let after = |w: &str| l.strip_prefix(w).filter(|r| r.starts_with(char::is_whitespace)).map(path);
        if module.is_none() { module = after("module") }
        if !guarded && let Some(p) = after("import") { imports.push(p) }
        guarded = l.starts_with("@if(") && !l.contains("import");      // an optional import: compiles without the module
    }
    let (b, mut toks, mut i) = (code.as_bytes(), vec![], 0);
    while i < b.len() {
        let s = i;
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1 }
            toks.push(&code[s..i]);
        } else { if b"{}@".contains(&b[i]) { toks.push(&code[i..=i]) } i += 1 }
    }
    let word = |t: &str| !b"{}@".contains(&t.as_bytes()[0]);
    let (mut defs, mut depth, mut method) = (vec![], 0usize, false);   // method: the next func wraps, replaces or adds a method
    for (k, t) in toks.iter().enumerate() {
        let next = toks.get(k + 1).filter(|n| word(n)).map(|n| n.to_string());
        match *t {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            "@" => method |= next.is_some_and(|a| ["Method", "Global", "Constructor"].into_iter().any(|s| a.ends_with(s))),
            "class" | "struct" | "enum" => defs.extend(next),
            "func" => { if depth == 0 && !method { defs.extend(next) } method = false }
            _ => {}
        }
    }
    (module, imports, defs, toks.into_iter().filter(|t| word(t)).map(str::to_string).collect())
}

/// Calls `f` with every file under `d`, at most 8 folders deep. Links aren't followed.
fn walk(d: &Path, depth: u8, f: &mut dyn FnMut(PathBuf)) {
    for e in fs::read_dir(d).into_iter().flatten().flatten() {
        match e.file_type() { Ok(t) if t.is_dir() && depth < 8 => walk(&e.path(), depth + 1, f), Ok(t) if t.is_file() => f(e.path()), _ => {} }
    }
}

/// Every redscript file in each root: in r6/scripts, and in RED4ext plugin folders, which hold scripts too (Codeware's).
fn scripts(roots: &[&Path]) -> Vec<Script> {
    let mut v = vec![];
    for (root, r) in roots.iter().enumerate() {
        for d in ["r6/scripts", "red4ext/plugins"].into_iter().filter_map(|d| resolve(r, d)) {
            walk(&d, 0, &mut |p: PathBuf| {
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("reds")) { return }
                let Some(entry) = p.strip_prefix(r).ok().and_then(|x| unit(&x.to_string_lossy())) else { return };
                let (module, imports, defs, names) = reds(&String::from_utf8_lossy(&read(&p, 8 << 20).unwrap_or_default()));
                v.push(Script { root, unit: entry, module, imports, defs, names });
            });
        }
    }
    v
}

/// The add-ons that build on the entries in `gone` (root, entry), and then on those, each with the entry it builds on:
/// entries whose scripts import a module only `gone` declares, or use a class or function `gone` defines in a module
/// they import or add to. Left behind, redscript can't compile them, and that stops every script mod.
fn addons(idx: &[Script], mut gone: Vec<(usize, String)>) -> Vec<(usize, String, String)> {
    let mut decl: BTreeMap<&str, Vec<&Script>> = BTreeMap::new();
    for s in idx { if let Some(m) = &s.module { decl.entry(m).or_default().push(s) } }
    fn module<'a>(decl: &BTreeMap<&str, Vec<&Script>>, p: &'a str) -> Option<&'a str> {    // "A.B.Thing" -> "A.B", if A.B is declared
        let mut q = p;
        loop { if decl.contains_key(q) { return Some(q) } q = &q[..q.rfind('.')?] }
    }
    let mut found = vec![];
    loop {
        let out = |s: &Script| gone.iter().any(|(r, u)| *r == s.root && *u == s.unit);
        let mut new: Vec<(usize, String, String)> = vec![];
        for s in idx.iter().filter(|s| !out(s)) {
            if new.iter().any(|(r, u, _)| *r == s.root && *u == s.unit) { continue }
            let base = s.imports.iter().filter_map(|p| module(&decl, p)).map(|m| (m, true)).chain(s.module.as_deref().map(|m| (m, false))).find_map(|(m, imported)| {
                let theirs: Vec<&Script> = decl[m].iter().copied().filter(|o| out(o)).collect();
                let all = imported && theirs.len() == decl[m].len();
                theirs.into_iter().find(|o| all || o.defs.iter().any(|d| s.names.contains(d) && !s.defs.contains(d))).map(|o| o.unit.clone())
            });
            if let Some(b) = base { new.push((s.root, s.unit.clone(), b)) }
        }
        if new.is_empty() { return found }
        gone.extend(new.iter().map(|(r, u, _)| (*r, u.clone())));
        found.extend(new);
    }
}

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

/// A mod the game won't run: a plugin RED4ext refused or one built for another game runtime, a script mod redscript
/// couldn't compile, world edits ArchiveXL skipped, a script RED4ext closed the game over, a file a mod's log says is
/// in its way, or a mod whose code Advanced Crash Reporter caught running when the game crashed. `folder` is its entry, lowercase, such as "red4ext/plugins/archivexl". With `near`, Remove offers only
/// same-named entries beside it (a file left from an older version and its .xl), never the mod's parts elsewhere.
#[derive(Clone, Debug)]
pub struct Refused { pub folder: String, pub name: String, pub version: Option<String>, pub why: String, pub near: bool }

/// What the logs say about the last launch, read only when that launch ran this game exe.
#[derive(Debug, Default)]
pub struct Last {
    pub when: String,                         // local time from the log's name, e.g. "2026-11-04 03:19"
    pub refused: Vec<Refused>,                // plugins RED4ext refused to load
    pub errors: Vec<(String, String)>,        // (who, message) from the mods' own logs
    pub crash: Option<(String, String)>,      // (what Advanced Crash Reporter concluded, its report)
    pub report: Option<PathBuf>,              // that report, for Open report
    pub memory: Option<String>,               // memory was nearly full when the game crashed or closed
    pub lead: Option<String>,                 // the mods named after a character the crash report names, such as a redesign
}

fn older(r: &str, exe: &str) -> &'static str { if cmp_ver(r, exe).is_lt() { "an older" } else { "a newer" } }

fn count(n: usize, noun: &str) -> String { format!("{n} {noun}{}", if n == 1 { "" } else { "s" }) }

/// "a", "a and b", "a, b and c"
fn list(v: &[String]) -> String { match v { [rest @ .., last] if !rest.is_empty() => format!("{} and {last}", rest.join(", ")), _ => v.concat() } }

/// One redscript error: (code, file, message), such as ("UNRESOLVED_IMPORT", "a.reds", "module 'X' has no members or does not exist").
type Fault<'a> = (&'a str, &'a str, &'a str);

/// Why redscript couldn't compile a mod, from its errors. Mods it builds on that aren't installed, part of it installed
/// twice and names nothing defines each have their own fix; anything else points to a game update.
fn cause(errs: &[Fault]) -> String {
    let (mut imports, mut names): (Vec<String>, Vec<(String, usize)>) = (vec![], vec![]);
    for (code, _, m) in errs {
        let Some(n) = m.split('\'').nth(1).filter(|n| !n.is_empty()).map(str::to_string) else { continue };
        match *code {
            "UNRESOLVED_IMPORT" => if !imports.contains(&n) { imports.push(n) },
            "UNRESOLVED_REF" | "UNRESOLVED_TYPE" | "UNRESOLVED_FN" => match names.iter_mut().find(|x| x.0 == n) { Some(x) => x.1 += 1, None => names.push((n, 1)) },
            _ => {}
        }
    }
    let mods: Vec<String> = imports.iter().filter(|m| !imports.iter().any(|o| m.starts_with(&format!("{o}.")))).cloned().collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.1));               // the most used first: the best hint at what's missing
    let twice = errs.iter().find(|e| e.0 == "SYM_REDEFINITION").map(|(_, f, m)|
        (f, m.split("defined at '").nth(1).and_then(|p| p.rsplit(['/', '\\']).next()).and_then(|p| p.split(':').next()).unwrap_or("another file")));
    if !mods.is_empty() { format!("It builds on {}, which no installed mod provides: install or put back the mod that does, or remove this one too.", list(&mods)) }
    else if let Some((f, other)) = twice { format!("Part of it is installed twice ({f} repeats {other}): delete the extra copy.") }
    else if !names.is_empty() {
        let top: Vec<String> = names.iter().take(3).map(|n| n.0.clone()).collect();
        format!("It uses {} that nothing installed defines, such as {}: a mod it needs is missing, another mod replaced its files, or it needs an update for this game version.",
                count(names.len(), "name"), list(&top))
    }
    else { format!("{} such as \"{}\": it likely needs an update for this game version.", count(errs.len(), "error"), errs.first().map_or("", |e| e.2)) }
}

/// Does a mod's log line say a file is in its way, such as an older copy an update left behind?
/// ("Conflicting older X component: <path>", "X 1.x is still installed (<path>). Delete it")
fn in_the_way(msg: &str) -> bool {
    let w: Vec<String> = msg.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).map(str::to_string).collect();
    w.iter().any(|x| x.starts_with("conflict") || ["older", "leftover", "outdated", "obsolete", "duplicate"].contains(&x.as_str()))
        || w.windows(2).any(|p| matches!([p[0].as_str(), p[1].as_str()], ["still", "installed"] | ["left", "over"] | ["delete" | "remove", "it"]))
}

/// The installed file or folder in a mod folder that a log line names, from its root: "(F:\...\Cyberpunk 2077\archive\pc\mod\X.archive).
/// Delete it" -> "archive/pc/mod/X.archive". A path can hold spaces, so trailing words are dropped until one resolves.
fn named(roots: &[&Path], msg: &str) -> Option<String> {
    let (m, low) = (msg.replace('\\', "/"), msg.replace('\\', "/").to_ascii_lowercase());     // same byte offsets
    let starts: Vec<usize> = MOD_DIRS.iter().chain(&["bin/x64/plugins"]).flat_map(|d| low.match_indices(&format!("{d}/")).map(|(i, _)| i).collect::<Vec<_>>())
        .filter(|&i| i == 0 || !low.as_bytes()[i - 1].is_ascii_alphanumeric()).collect();
    starts.into_iter().find_map(|i| {
        let rest = &m[i..];
        let mut p = &rest[..rest.find(['"', '\'', '(', ')', '[', ']', ',', ';', '<', '>', '|', '*']).unwrap_or(rest.len())];
        loop {
            p = p.trim_end_matches(['.', ':', ' ']);
            if unit(p).is_some() && roots.iter().any(|r| resolve(r, p).is_some()) { return Some(p.to_string()) }
            p = &p[..p.rfind(' ')?];
        }
    })
}

/// Files named `file` in RED4ext plugin folders, such as a copy left there by an older version of a mod that now installs to archive/pc/mod.
fn copies(roots: &[&Path], file: &str) -> Vec<String> {
    let mut v = vec![];
    for r in roots {
        let Some(d) = resolve(r, "red4ext/plugins") else { continue };
        walk(&d, 0, &mut |p: PathBuf| if name(&p).eq_ignore_ascii_case(file) && let Ok(x) = p.strip_prefix(r) { v.push(x.to_string_lossy().replace('\\', "/")) });
    }
    v
}

/// Has the mod in `folder` been removed or changed since `at`, when a log reported it?
fn gone(roots: &[&Path], folder: &str, at: SystemTime) -> bool {
    roots.iter().find_map(|r| resolve(r, folder)).and_then(|p| changed(&p)).is_none_or(|t| t > at)
}

/// RED4ext's newest log (and redscript's, the mod logs and the crash report from the same launch), if that launch ran `exe`.
/// `framework` tells a framework's mod entry, which a log can't make a leftover.
fn last_launch(roots: &[&Path], exe: &str, framework: &dyn Fn(&str) -> bool, entries: &[Vec<String>]) -> Option<Last> {
    let logs: Vec<PathBuf> = roots.iter().filter_map(|r| resolve(r, "red4ext/logs")).flat_map(|d| files(&d)).collect();
    let r4 = logs.iter().filter_map(|p| log_name(&name(p)).filter(|(w, _)| w.eq_ignore_ascii_case("red4ext")).map(|(_, d)| (p, d)))
        .max_by(|a, b| a.1.cmp(&b.1)).and_then(|(log, stamp)| {
            let text = String::from_utf8_lossy(&read(log, 16 << 20)?).into_owned();
            text.lines().any(|l| l.split("File version: ").nth(1).is_some_and(|v| v.trim() == exe)).then(|| (log.clone(), stamp, text))
        });
    // without RED4ext, Advanced Crash Reporter's own log tells when the last launch started
    let (log, stamp, text) = r4.or_else(|| Some((PathBuf::new(), acr_launch(roots, exe)?, String::new())))?;
    let at = fs::metadata(&log).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
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
                    why: format!("RED4ext refused to load it at your last launch ({}): it's built for {} game version.", last.when, older(runtime, exe)), near: false });
            }
        }
    }

    let rs = roots.iter().find_map(|r| resolve(r, "r6/logs/redscript_rCURRENT.log"));
    if let Some(rs) = rs && let Some(b) = read(&rs, 16 << 20) && let Ok(at) = fs::metadata(&rs).and_then(|m| m.modified()) {
        let t = String::from_utf8_lossy(&b);
        if t.lines().rev().find_map(redscript_stamp).is_some_and(|d| d >= stamp) {
            let lines: Vec<&str> = t.lines().collect();
            let mut failed: Vec<(String, String, Vec<Fault>)> = vec![];      // (folder, name, errors)
            for (i, l) in lines.iter().enumerate().filter(|(_, l)| l.starts_with("[ERROR")) {
                let Some(path) = l.split(" At ").nth(1) else { continue };
                let bits: Vec<&str> = path.split(['/', '\\']).collect();
                let Some(f) = bits.iter().rposition(|b| b.eq_ignore_ascii_case("scripts")).and_then(|i| bits.get(i + 1)).filter(|f| !f.contains(".reds")) else { continue };
                let code = l.split("] [").nth(1).and_then(|c| c.split(']').next()).unwrap_or("");
                let file = bits.last().and_then(|b| b.split(':').next()).unwrap_or("");
                let msg = lines.get(i + 3).filter(|_| lines.get(i + 2).is_some_and(|c| c.trim_start().starts_with('^'))).map_or("", |m| m.trim());
                match failed.iter_mut().find(|x| x.1 == *f) { Some(x) => x.2.push((code, file, msg)), None => failed.push((format!("r6/scripts/{}", f.to_lowercase()), f.to_string(), vec![(code, file, msg)])) }
            }
            failed.retain(|x| !gone(roots, &x.0, at));
            if !failed.is_empty() {
                last.errors.push(("redscript".into(), format!("Couldn't compile {}, so no script mod ran.", count(failed.len(), "mod"))));
            }
            for (folder, name, errs) in failed {
                last.refused.push(Refused { folder, name, version: None, near: false,
                    why: format!("redscript couldn't compile it at your last launch ({}), which stops every script mod. {}", last.when, cause(&errs)) });
            }
        }
    }

    let plugin_logs = roots.iter().filter_map(|r| resolve(r, "red4ext/plugins")).flat_map(|d| files(&d)).filter(|d| d.is_dir()).flat_map(|d| files(&d));
    let mut sources: Vec<PathBuf> = logs.iter().cloned().chain(plugin_logs).collect();
    sources.sort_by_key(|p| (log_name(&name(p)).is_none_or(|(w, _)| !w.eq_ignore_ascii_case("red4ext")), p.clone()));   // RED4ext's own first
    let mut per: BTreeMap<String, (usize, String)> = BTreeMap::new();   // log owner -> (errors, name as shown)
    let mut stale: Vec<(String, Vec<String>, SystemTime)> = vec![];     // ArchiveXL world edits whose target has changed: (.xl, mismatches, log time)
    let mut leftovers: Vec<(String, String, String, SystemTime)> = vec![];   // files a mod's log says are in its way: (path, who, message, log time)
    let mut invalid: Vec<(String, usize, String, SystemTime)> = vec![];     // scripts RED4ext closed the game over: (path, errors, first, log time)
    let mut oom = false;                                                     // the game's crash report says it ran out of memory
    for p in sources {
        let Some((owner, d)) = log_name(&name(&p)).map(|(w, d)| (w.to_string(), d)) else { continue };
        if d < stamp { continue }
        let Some(b) = read(&p, 16 << 20) else { continue };
        let at = fs::metadata(&p).and_then(|m| m.modified()).unwrap_or_else(|_| SystemTime::now());
        for l in String::from_utf8_lossy(&b).lines() {
            let Some((tag, msg)) = error_line(l) else { continue };
            // "X.xl: The target node #15 has type A, but the mod expects B." (or "The target sector has N node(s), ...")
            if let Some((xl, what)) = msg.split_once(": The target ").filter(|(x, w)| x.to_ascii_lowercase().ends_with(".xl") && w.contains("but the mod expects")) {
                match stale.iter_mut().find(|s| s.0 == xl) { Some(s) => if !s.1.iter().any(|w| w == what) { s.1.push(what.into()) }, None => stale.push((xl.into(), vec![what.into()], at)) }
                continue
            }
            // RED4ext copies the game's own crash report into its log, one error line per field: keep only its message
            if msg.trim_matches(['-', '=', ' ']).is_empty() || ["Crash report", "File:", "Line:", "Condition:"].iter().any(|s| msg.starts_with(s)) { continue }
            let msg = msg.strip_prefix("Message: ").map_or_else(|| msg.to_string(), |m| format!("The game's crash report says: {m}"));
            let (who, msg) = match tag {                                  // a log is its owner's, whatever part of it wrote the line
                Some(t) if t.eq_ignore_ascii_case(&owner) => (t.to_string(), msg),
                Some(t) => (owner.clone(), format!("[{t}] {msg}")),
                None => (owner.clone(), msg),
            };
            oom |= who == "RED4ext" && msg.starts_with("The game's crash report says: ") && msg.to_ascii_lowercase().contains("out of memory");
            // "Script validation error: Missing native global function 'X.Y' at <path>:3": a script needs native code no plugin provides
            if who == "RED4ext" && let Some((what, at_path)) = msg.strip_prefix("Script validation error: ").and_then(|m| m.rsplit_once(" at "))
                && let Some(p) = named(roots, at_path.trim_end_matches(|c: char| c.is_ascii_digit() || c == ':')) {
                match invalid.iter_mut().find(|x| x.0 == p) { Some(x) => x.1 += 1, None => invalid.push((p, 1, what.to_string(), at)) }
                continue
            }
            if in_the_way(&msg) && let Some(p) = named(roots, &msg).filter(|p| !unit(p).is_some_and(|u| framework(&u))) {
                if !leftovers.iter().any(|x| x.0 == p) { leftovers.push((p, who, msg, at)) }
                continue
            }
            let n = per.entry(who.to_lowercase()).or_insert((0, who.clone()));
            n.0 += 1;
            if n.0 <= 3 && last.errors.len() < 30 && !last.errors.iter().any(|(w, m)| *w == who && *m == msg) { last.errors.push((who, msg)) }
        }
    }
    for (n, who) in per.into_values().filter(|(n, _)| *n > 3) { last.errors.push((who, format!("...and {} more in its log.", n - 3))) }
    for (xl, what, at) in stale {
        let rel = format!("archive/pc/mod/{xl}");
        let found = roots.iter().any(|r| resolve(r, &rel).is_some());
        if found && !gone(roots, &rel, at) {
            let others = copies(roots, &xl);
            let also = if others.is_empty() { String::new() } else { format!(
                " Another copy is in {}: if it's left from an older version, move it out by hand first and launch again, as it may be the one that no longer fits.", list(&others)) };
            last.refused.push(Refused { folder: rel.to_lowercase(), name: xl.split('.').next().unwrap_or(&xl).to_string(), version: None, near: false, why: format!(
                "At your last launch ({}), ArchiveXL skipped its world edits in {}: the map has changed since it was made, in a game update or an update of \
                 the mod whose area it edits. Edits that still seemed to fit may change the wrong objects, which can crash the game where they load. It needs an update.{also}",
                last.when, count(what.len(), "place")) });
        } else if !found { last.errors.push(("ArchiveXL".into(), format!("{xl}'s world edits no longer fit the map in {}.", count(what.len(), "place")))) }
    }

    let mut closed = vec![];
    for (p, n, what, _) in invalid.into_iter().filter(|x| !gone(roots, &x.0, x.3)) {
        let (entry, file) = (unit(&p).unwrap_or_default(), p.rsplit('/').next().unwrap_or(&p));
        let name = entry.rsplit('/').next().unwrap_or(&entry).to_string();
        let more = if n > 1 { format!(" and {} more", n - 1) } else { String::new() };
        let how = if entry == p { "No loaded plugin provides it, so it's likely left from an older version of a plugin: back it up and remove it." }
            else { "The plugin it needs is missing, didn't load, or is another version: update or reinstall the mod, or back it up and remove it." };
        closed.push(name.clone());
        last.refused.push(Refused { folder: entry.to_lowercase(), name, version: None, near: entry == p,
            why: format!("At your last launch ({}), RED4ext closed the game over {file}: \"{what}\"{more}. {how}", last.when) });
    }
    if !closed.is_empty() {
        last.errors.insert(0, ("RED4ext".into(), format!("Closed the game at startup: {} need{} native code that no loaded plugin provides.", list(&closed), if closed.len() == 1 { "s" } else { "" })));
    }
    for (p, who, msg, _) in leftovers.into_iter().filter(|x| !gone(roots, &x.0, x.3)) {
        let (entry, file) = (unit(&p).unwrap_or_default(), p.rsplit('/').next().unwrap_or(&p));
        let owner = entry.rsplit('/').next().unwrap_or(&entry);
        let (name, how) = if entry == p { (file.to_string(), "If it's left from an older version, back it up and remove it.".to_string()) } else { (format!("{file} (in {owner})"), format!(
            "It's in {owner}'s folder, and Back up and remove only takes whole mods: if it's left from an older version, move {p} out by hand and keep a copy.")) };
        last.refused.push(Refused { folder: p.to_lowercase(), name, version: None, near: true, why: format!("{who}'s log at your last launch ({}): \"{msg}\" {how}", last.when) });
    }

    let reports = reports_dir(roots);
    let mut found: Vec<(String, PathBuf)> = reports.iter().flat_map(|d| files(d)).filter_map(|p| {
        let n = name(&p);
        let d: String = n.get(..15)?.chars().filter(char::is_ascii_digit).collect();
        (n.ends_with(".txt") && d.len() == 14 && d >= stamp).then_some((d, p))
    }).collect();
    found.sort();
    let (mut mem, mut did) = (None, "");                                    // memory when the game crashed or closed, and which it did
    for (_, p) in found.iter().rev() {
        let Some(b) = read(p, 4 << 20) else { continue };
        let t = String::from_utf8_lossy(&b);
        let hang = name(p).contains("-hang-");
        // a "freeze" while a message box waits for the player, such as redscript's error box, is no freeze
        if hang && t.lines().skip_while(|l| !l.starts_with("# PRIMARY STACK")).take(16).any(|l| l.contains("MessageBox")) { continue }
        if field(&t, "Game version").is_some_and(|v| v.contains(exe)) && let Some(v) = field(&t, "Verdict").or_else(|| field(&t, "Summary")) {
            let rel = roots.iter().find_map(|r| p.strip_prefix(r).ok()).unwrap_or(p).display().to_string();
            let hooks = call_path_hooks(&t);
            let also = if hooks.is_empty() { String::new() } else { format!(" Also hooked in on the call path: {}.", hooks.join(", ")) };
            (did, mem) = (if hang { "froze" } else { "crashed" }, memory(&t));
            let (v, nums) = (v.trim_end_matches('.'), mem.as_ref().map_or(String::new(), |(c, g, _)| format!(" ({c} committed, {g} of it by the game)")));
            // a crash when memory ran out isn't a mod's fault, whatever the verdict says
            last.crash = Some((if oom { format!("Your last launch {did} because the game ran out of memory{nums}: {FREE}. Advanced Crash Reporter's verdict ({v}) likely doesn't apply.") }
                               else { format!("Your last launch {did}. Advanced Crash Reporter's verdict: {v}.{also}") }, rel));
            last.report = Some(p.clone());
            if !oom { last.lead = named_after(&t, roots, entries, framework) }
            // the mod whose own code was running: unless it's a framework, or changed since
            if !oom && let Some((u, version, strength)) = culprit(&t, roots, framework)
                && let Ok(at) = fs::metadata(p).and_then(|m| m.modified()) && !gone(roots, &u, at) {
                last.refused.push(Refused { folder: u.to_lowercase(), name: u.rsplit('/').next().unwrap_or(&u).to_string(), version, near: false, why: format!(
                    "{strength} culprit of your last launch's {} ({}), says Advanced Crash Reporter: its own code was running then. Update it, or back it up and remove it.",
                    if hang { "freeze" } else { "crash" }, last.when) });
            }
        }
        break;
    }
    if oom && last.crash.is_none() {
        let rel = roots.iter().find_map(|r| log.strip_prefix(r).ok()).unwrap_or(&log).display().to_string();
        last.crash = Some((format!("Your last launch crashed because the game ran out of memory: {FREE}."), rel));
    }
    // no crash: the crash reporter's newest routine snapshot of this launch, from a few seconds before the game closed
    if did.is_empty() && let Some(p) = reports.and_then(|d| files(&d.join("watch")).max_by_key(|p| changed(p))) && let Some(b) = read(&p, 4 << 20) {
        let t = String::from_utf8_lossy(&b);
        let at: String = field(&t, "Generated").unwrap_or("").chars().filter(char::is_ascii_digit).take(14).collect();
        if field(&t, "Game version").is_some_and(|v| v.contains(exe)) && at.len() == 14 && at >= stamp { mem = memory(&t) }
    }
    if !oom && let Some((c, g, true)) = &mem {
        let at = if did.is_empty() { "just before the game closed".to_string() } else { format!("when it {did}") };
        last.memory = Some(format!("Memory was nearly full {at} ({c} committed, {g} of it by the game), and running out can crash the game or cut its sound: {FREE}."));
    }
    Some(last)
}

/// Where Advanced Crash Reporter writes its reports: the `output_dir` its config.ini sets (relative to its folder), or
/// the reports folder beside it.
fn reports_dir(roots: &[&Path]) -> Option<PathBuf> {
    let acr = roots.iter().find_map(|r| resolve(r, "bin/x64/plugins/AdvancedCrashReporter"))?;
    let ini = read(&acr.join("config.ini"), 1 << 16).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let set = ini.lines().find_map(|l| l.trim().strip_prefix("output_dir")?.trim_start().strip_prefix('=').map(|v| v.trim().trim_matches('"').to_string()));
    set.filter(|v| !v.is_empty()).map(|v| acr.join(v)).filter(|d| d.is_dir()).or_else(|| resolve(&acr, "reports"))
}

/// When the last launch started ("20261104181648"), from the first line of Advanced Crash Reporter's own log, if that
/// launch ran `exe` ("module #000 loaded: Cyberpunk2077.exe [GAME-EXE] gap=0.000s v3.0.88.46235").
fn acr_launch(roots: &[&Path], exe: &str) -> Option<String> {
    let b = read(&roots.iter().find_map(|r| resolve(r, "bin/x64/plugins/AdvancedCrashReporter/AdvancedCrashReporter.log"))?, 16 << 20)?;
    let t = String::from_utf8_lossy(&b);
    let d: String = t.get(1..20)?.chars().filter(char::is_ascii_digit).collect();
    (d.len() == 14 && t.lines().take(50).any(|l| l.contains("loaded: Cyberpunk2077.exe") && l.trim_end().ends_with(&format!(" v{exe}")))).then_some(d)
}

/// The mod entry Advanced Crash Reporter calls the strong or likely culprit, when its own code was running (MOD
/// INVOLVEMENT's "RUNNING" blocks, each mod followed by `in <its folder>`), with its version and "Strong" or "Likely".
/// Never a framework: crashes often only pass through one, so the verdict line covers those.
fn culprit(report: &str, roots: &[&Path], framework: &dyn Fn(&str) -> bool) -> Option<(String, Option<String>, String)> {
    let (strength, who) = field(report, "Verdict")?.split_once(" culprit: ").filter(|(s, _)| ["Strong", "Likely"].contains(s))?;
    let (mut running, mut module) = (false, "");
    for l in report.lines().skip_while(|l| !l.starts_with("# MOD INVOLVEMENT")).skip(1).take_while(|l| !l.starts_with("# ")) {
        if !l.starts_with(' ') { running = l.contains(" RUNNING "); continue }
        let t = l.trim();
        if !running || t.starts_with("->") { continue }
        // a module's own line, "BlurBegoneNative.dll  [MOD-RED4EXT ] frame #32", then its folder: "RED4ext plugin "BlurBegoneNative" 2.6.0 by CyanideX, in red4ext/plugins/BlurBegone"
        let path = match t.split_once(' ').map(|(m, r)| (m, r.trim_start())).filter(|(_, r)| r.starts_with('[')) {
            Some((m, r)) => { module = m; if r.starts_with("[MOD") && m.to_ascii_lowercase().ends_with(".asi") { format!("bin/x64/plugins/{m}") } else { continue } }
            None => t.to_string(),
        };
        let Some(u) = named(roots, &path).and_then(|p| unit(&p)) else { continue };
        let stem = module.rsplit_once('.').map_or(module, |m| m.0);
        if (key(&u) == key(who) || stem.eq_ignore_ascii_case(who)) && !framework(&u) {
            let version = t.split('"').nth(2).and_then(|v| v.split_whitespace().next()).filter(|v| v.starts_with(|c: char| c.is_ascii_digit()));
            return Some((u, version.map(str::to_string), strength.to_string()));
        }
    }
    None
}

/// The mods named after a character Advanced Crash Reporter names among the objects the crash was handling ("objects: animRig,
/// NPCPuppet misty.ent"): a mod that changes the character, such as a redesign, is the likeliest cause. Mods in other mods' folders aren't searched.
fn named_after(report: &str, roots: &[&Path], entries: &[Vec<String>], framework: &dyn Fn(&str) -> bool) -> Option<String> {
    let objs = field(report, "Technical")?.split("objects: ").nth(1)?.split(';').next()?;
    let who: Vec<String> = objs.split(", ").filter_map(|o| o.rsplit(' ').next()?.strip_suffix(".ent"))     // not passers-by: "citizen__ep1_tenant_ma.ent"
        .filter(|s| s.len() > 2 && s.bytes().all(|b| b.is_ascii_alphabetic())).map(str::to_lowercase).collect();
    let (mut whom, mut v) = (vec![], vec![]);
    for (r, u) in entries.iter().enumerate().flat_map(|(r, l)| l.iter().map(move |u| (r, u))).filter(|(_, u)| !framework(u)) {
        let f = u.rsplit('/').next().unwrap_or(u);
        let Some(w) = who.iter().find(|w| after(f, w)) else { continue };
        if !whom.contains(w) { whom.push(w.clone()) }
        v.push(if r == roots.len() - 1 { f.to_string() } else { format!("{f} (in {})", name(roots[r])) });
    }
    if v.is_empty() { return None }
    v.sort_by_key(|f| f.to_lowercase());
    if v.len() > 6 { let n = v.len() - 5; v.truncate(5); v.push(format!("{n} more")) }
    let ents: Vec<String> = whom.iter().map(|w| format!("{w}.ent")).collect();
    let names: Vec<String> = whom.iter().map(|w| w[..1].to_uppercase() + &w[1..]).collect();
    Some(format!("The crash report names {}, so try without the mods named after {} first: {}.", list(&ents), list(&names), list(&v)))
}

/// Is a file named after `who` ("misty"), as a word of its name? "!!00_Misty_Goth_Redesign.archive", "TheMistyMod", not "Mistyfied".
fn after(file: &str, who: &str) -> bool {
    let b = file.as_bytes();
    file.to_ascii_lowercase().match_indices(who).any(|(i, _)| (i == 0 || !b[i - 1].is_ascii_alphabetic() || b[i - 1].is_ascii_lowercase() && b[i].is_ascii_uppercase())
        && !b.get(i + who.len()).is_some_and(u8::is_ascii_lowercase))
}

/// What to do when memory runs out: need less of it, or let the system commit more.
const FREE: &str = if cfg!(windows) { "close other programs, or set a larger fixed page file (32 GB, say)" } else { "close other programs, or add swap space" };

/// A field from a crash report's header, such as "Verdict        : Likely culprit: X".
fn field<'a>(report: &'a str, k: &str) -> Option<&'a str> { report.lines().take(40).find_map(|l| l.strip_prefix(k)?.trim_start().strip_prefix(':').map(str::trim)) }

/// Memory at the crash, from a crash report's PC CONFIGURATION: ("47.1 GB of 47.4 GB" committed, the game's "16.3 GB", near the limit?).
fn memory(report: &str) -> Option<(String, String, bool)> {
    let pc = report.lines().skip_while(|l| !l.starts_with("# PC CONFIGURATION"));
    let field = |k: &str| pc.clone().find_map(|l| l.trim_start().strip_prefix(k)?.trim_start().strip_prefix(':').map(str::trim));
    let c = field("Commit charge")?;
    Some((c.split(" (").next()?.to_string(), field("Game process")?.split(" committed").next()?.to_string(), c.contains("NEAR THE LIMIT")))
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
                    why: format!("{} is built for {} game version (exe {r}, yours is {e}), so RED4ext won't load it. It needs an update.", name(dll), older(&r, &e)), near: false });
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

/// Which same-named entries Remove offers with a mod's own: none, only files beside them in the same mod folder (an
/// old file's .xl, not a folder holding the mod's current version), or any.
#[derive(Clone, Copy, PartialEq)]
enum Kin { No, Beside, Any }

/// Why Remove offers an entry: the mod's own, a **Companion** (same name), or part of an add-on that builds on the
/// named entry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Part { Own, Kin, Uses(String) }

pub struct Row {
    pub name: String, pub nexus_id: Option<u32>, pub version: Option<String>, pub status: Status, pub why: String, pub notes: Vec<Note>,
    pub remove: Vec<(String, Part)>,    // red mods: its entries in the game folder, then those of the add-ons that build on it
    pub mo2: Vec<String>,               // red mods: the Mod Organizer 2 mods that hold its files or such an add-on
}

pub struct Report {
    pub game: PathBuf,
    pub build: Option<String>,      // e.g. "2.5"
    pub exe: Option<String>,        // the exe's file version, e.g. "3.0.88.46235"
    pub banner: Option<(bool, String)>,   // (is a warning, text); a draft list is only a notice
    pub rows: Vec<Row>,             // sorted: red, gray, green, then by name
    pub unlisted: Vec<String>,      // native plugins the list doesn't know
    pub last: Option<Last>,         // what the logs say about the last launch
    pub crash_reporter: bool,       // Advanced Crash Reporter is installed: without it no crash is ever seen
}

/// Advanced Crash Reporter's Nexus mod ID, for the note shown when it isn't installed.
pub const CRASH_REPORTER: u32 = 34403;
/// Why the app can't see a crash without Advanced Crash Reporter.
pub const NO_CRASH_REPORTER: &str = "Crashes aren't shown: Advanced Crash Reporter isn't installed. Without it the app can't see a crash or the mod behind it, \
and a crash at one scene or place usually comes from a mod that changes what loads there (an appearance, body or animation mod), not from one this list flags. \
Install it, crash once more, then rescan.";

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

    // For Back up and remove: every mod entry in each root. A red mod's entries are the ones its own paths point into,
    // plus (with `kin`) entries elsewhere that share its name, such as a plugin's scripts. Frameworks are never included.
    let entries: Vec<Vec<String>> = roots.iter().map(|r| MOD_DIRS.iter().chain(&["bin/x64/plugins"]).filter_map(|d| resolve(r, d))
        .flat_map(|d| files(&d)).filter_map(|p| p.strip_prefix(r).ok().map(|x| x.to_string_lossy().replace('\\', "/")))
        .filter(|rel| unit(rel).as_deref() == Some(rel.as_str())).collect()).collect();
    // A framework's entries, and its other entries by name (such as r6/scripts/Audioware), even when a newer list leaves it out
    let built = Manifest::parse(BUILT_IN).ok();
    let protect: Vec<String> = m.framework.iter().chain(built.iter().flat_map(|b| &b.framework)).flat_map(|e| e.detect.iter().chain(&e.parts)).filter_map(|d| unit(d)).collect();
    let framework = |u: &str| protect.iter().any(|p| wild(p, u) || wild(&key(p), &key(u)));
    let mut last = exe_v.as_deref().and_then(|e| last_launch(&roots, e, &framework, &entries));
    let dir = |u: &str| u.rsplit_once('/').map_or(String::new(), |x| x.0.to_lowercase());
    let matching = |own: &[String], kin: Kin| {                     // (root, entry, own?), its own first
        let keys: Vec<(String, String)> = own.iter().map(|u| (key(u), dir(u))).filter(|k| !k.0.is_empty()).collect();
        let mut v: Vec<(usize, String, bool)> = entries.iter().enumerate().flat_map(|(r, list)| list.iter().map(move |u| (r, u))).filter_map(|(r, u)| {
            let mine = own.iter().any(|o| wild(o, u));
            let same = kin != Kin::No && keys.iter().any(|(k, d)| wild(k, &key(u)) && (kin == Kin::Any || *d == dir(u) && roots[r].join(u).is_file()));
            ((mine || same) && !framework(u)).then(|| (r, u.clone(), mine))
        }).collect();
        v.sort_by_key(|(_, u, mine)| (!mine, u.to_lowercase()));
        v
    };
    // ...then the add-ons whose scripts build on what it takes from the game folder (the last root), with their companions
    let (g, index) = (roots.len() - 1, OnceCell::new());
    let pieces = |own: &[String], kin: Kin| {
        let own: Vec<String> = own.iter().filter_map(|p| unit(p)).collect();
        let mut found: Vec<(usize, String, Part)> = matching(&own, kin).into_iter().map(|(r, u, mine)| (r, u, if mine { Part::Own } else { Part::Kin })).collect();
        let gone: Vec<(usize, String)> = found.iter().filter(|f| f.0 == g).map(|f| (g, f.1.clone())).collect();
        if !gone.is_empty() {
            let mut users = addons(index.get_or_init(|| scripts(&roots)), gone);
            users.sort_by_key(|(_, u, _)| u.to_lowercase());
            for (r, u, base) in users.into_iter().filter(|(_, u, _)| !framework(u)) {
                let n = base.rsplit('/').next().unwrap_or(&base);
                let by = Part::Uses(n.strip_suffix(".reds").unwrap_or(n).to_string());
                if r == g { found.extend(matching(std::slice::from_ref(&u), Kin::Any).into_iter().map(|(r, u, _)| (r, u, by.clone()))) }
                else { found.push((r, u, by)) }
            }
        }
        let (mut here, mut mo2): (Vec<(String, Part)>, Vec<String>) = (vec![], vec![]);
        for (r, u, part) in found {
            if r != g { if !mo2.contains(&name(roots[r])) { mo2.push(name(roots[r])) } }
            else if !here.iter().any(|(h, _)| *h == u) { here.push((u, part)) }
        }
        (here, mo2)
    };
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
            // a draft build listed before its frameworks are: build-locked ones stay red, others with a `min` elsewhere stay unconfirmed
            else if e.build_locked && (entry.is_none() || min.is_none()) { (Red, format!("Each release is built for one game build, and this list has none for {b} yet. Expect it to need an update.")) }
            else if entry.is_none() || (min.is_none() && !e.min.is_empty()) { (Gray, format!("Not confirmed for game {b} yet.")) }
            else if let (Some(min), Some(v)) = (min, &version) && cmp_ver(v, min).is_lt() { (Red, format!("Version {v} is older than {min}, the lowest known to work on game {b}.")) }
            else if let (Some(min), None) = (min, &version) { (Gray, format!("Its version couldn't be read. It should be {min} or newer.")) }
            else if !entry.is_some_and(|x| x.confirmed) { (Gray, format!("Matches the list for game {b}, which isn't confirmed by a launch test yet.")) }
            else if let Some(n) = need(Some(Gray)) { (Gray, format!("Needs {n}, which isn't confirmed.")) }
            else { (Green, format!("OK for game {b}.")) };
        if fw { state.insert(&e.name, Some(status)); }
        let (remove, mo2) = if status == Red && !fw { pieces(&[&e.detect[..], &e.parts[..]].concat(), Kin::Any) } else { (vec![], vec![]) };
        rows.push(Row { name: e.name.clone(), nexus_id: Some(e.nexus_id), version, status, why, notes, remove, mo2 });
    }
    // Advanced Crash Reporter blames the framework a crash passed through, which is often a bystander: say so when it's fine
    if let Some((c, _)) = last.as_mut().and_then(|l| l.crash.as_mut()) && entry.is_some() && !c.contains("ran out of memory")
        && let Some(f) = m.framework.iter().find(|f| c.contains(&format!("culprit: {}", f.name)) && state.get(f.name.as_str()).copied().flatten().is_some_and(|s| s != Red)) {
        *c += &format!(" {} itself matches the list for game {b}, so a mod that uses it is the likelier cause.", f.name);
    }
    for f in &m.framework {                                  // missing frameworks matter only when something installed needs them
        let users: Vec<&str> = rows.iter().filter_map(|r| m.framework.iter().chain(&m.mods).find(|e| e.name == r.name))
            .filter(|e| e.needs.contains(&f.name)).map(|e| e.name.as_str()).collect();
        if state.get(f.name.as_str()).is_some_and(Option::is_none) && !users.is_empty() {
            rows.push(Row { name: f.name.clone(), nexus_id: Some(f.nexus_id), version: None, status: Red, notes: vec![], remove: vec![], mo2: vec![],
                            why: format!("Not installed, but {} need{} it.", users.join(", "), if users.len() == 1 { "s" } else { "" }) });
        }
    }
    for r in refused.iter().filter(|r| !claimed.contains(&r.folder)) {   // unlisted mods the last launch shows broken
        // a file inside a mod is left to the player, and so is a name with a `*`, which would match other mods too
        let whole = unit(&r.folder).as_deref() == Some(r.folder.as_str()) && !r.folder.contains('*');
        let (remove, mo2) = if whole { pieces(std::slice::from_ref(&r.folder), if r.near { Kin::Beside } else { Kin::Any }) } else { (vec![], vec![]) };
        rows.push(Row { name: r.name.clone(), nexus_id: None, version: r.version.clone(), status: Red, why: r.why.clone(), notes: vec![], remove, mo2 });
    }
    for p in m.pattern.iter().filter(|p| p.build.as_deref().is_none_or(|pb| build.as_deref().is_some_and(|b| cmp_ver(pb, b).is_eq()))) {
        if p.all.iter().all(|x| find(x).is_some()) && !p.none.iter().any(|x| find(x).is_some()) {
            let (remove, mo2) = pieces(&p.remove, Kin::No);
            rows.push(Row { name: p.name.clone(), nexus_id: p.nexus_id, version: None, status: Red, notes: vec![], remove, mo2,
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
    let crash_reporter = ["bin/x64/plugins/AdvancedCrashReporter.asi", "bin/x64/plugins/AdvancedCrashReporter"].iter().any(|p| find(p).is_some());
    Ok(Report { game: game.to_path_buf(), build, exe: exe_v, banner, rows, unlisted, last, crash_reporter })
}

/// Plain-text report, for `--report` and the tests.
pub fn text(r: &Report, source: &str, updated: &str) -> String {
    let mut s = format!("Game build {} (exe {}). Compatibility list {updated}, {source}.\n",
                        r.build.as_deref().unwrap_or("unknown"), r.exe.as_deref().unwrap_or("unknown"));
    if let Some((_, b)) = &r.banner { s += &format!("! {b}\n") }
    if !r.crash_reporter { s += &format!("{NO_CRASH_REPORTER} {}\n", nexus_url(CRASH_REPORTER)) }
    if let Some(l) = &r.last {
        s += &format!("Last launch: {}\n", l.when);
        if let Some((c, p)) = &l.crash { s += &format!("! {c} ({p})\n") }
        if let Some(m) = &l.memory { s += &format!("! {m}\n") }
        if let Some(m) = &l.lead { s += &format!("! {m}\n") }
        for (w, m) in &l.errors { s += &format!("\terror: {w}: {m}\n") }
    }
    for x in &r.rows {
        s += &format!("{:?}\t{}\t{}\t{}\t{}\n", x.status, x.name, x.version.as_deref().unwrap_or("-"), x.why, x.nexus_id.map_or("-".into(), nexus_url));
        for n in &x.notes { s += &format!("\tnote: {} ({}, {})\n", n.text, n.source, n.date) }
        let take = |(p, k): &(String, Part)| match k { Part::Uses(b) => format!("{p} (builds on {b})"), _ => p.clone() };
        if !x.remove.is_empty() { s += &format!("\tcan remove: {}\n", x.remove.iter().map(take).collect::<Vec<_>>().join(", ")) }
        if !x.mo2.is_empty() { s += &format!("\tin Mod Organizer 2: {}\n", x.mo2.join(", ")) }
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
        min = { "2.5" = "1.38.0" }
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
    fn says_when_crashes_cant_be_seen() {
        let (r, _) = verdicts([2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0]))], "no-acr");
        assert!(!r.crash_reporter && text(&r, "", "").contains("Crashes aren't shown") && text(&r, "", "").contains("/mods/34403"));
        let (r, _) = verdicts([2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("bin/x64/plugins/AdvancedCrashReporter.asi", None)], "acr");
        assert!(r.crash_reporter && !text(&r, "", "").contains("Crashes aren't shown"));
    }

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
    fn draft_build_before_its_releases() {
        let draft = LIST.replace(r#"build = [{ game = "2.5", confirmed = true }]"#, r#"build = [{ game = "2.5", confirmed = true }, { game = "2.6" }]"#);
        let files = [("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("bin/x64/plugins/cyber_engine_tweaks.asi", Some([1, 38, 1, 0]))];
        let why = |r: &Report, n: &str| r.rows.iter().find(|x| x.name == n).map(|x| (x.status, x.why.clone())).unwrap();
        let (r, _) = run(&draft, [2, 6, 0, 0], &files, "draft", &NO_ENV);
        assert!(r.banner.as_ref().unwrap().1.contains("draft"));
        assert_eq!(why(&r, "CET"), (Status::Red, "Each release is built for one game build, and this list has none for 2.6 yet. Expect it to need an update.".into()));
        assert_eq!(why(&r, "RED4ext"), (Status::Gray, "Not confirmed for game 2.6 yet.".into()));
        let listed = draft.replace(r#"min = { "2.5" = "1.31.0" }"#, r#"min = { "2.5" = "1.31.0", "2.6" = "1.31.0" }"#);   // once a release is listed
        let (r, _) = run(&listed, [2, 6, 0, 0], &files, "draft2", &NO_ENV);
        assert!(why(&r, "RED4ext").1.starts_with("Matches the list for game 2.6"));
        let unlisted = LIST.replace(r#"min = { "2.5" = "1.38.0" }"#, "");   // a confirmed build that doesn't say which CET works on it
        let (r, _) = run(&unlisted, [2, 5, 0, 0], &files, "nomin", &NO_ENV);
        assert_eq!(why(&r, "CET").0, Status::Red);
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
                                                  ("red4ext/plugins/Gone/g.dll", None), ("red4ext/plugins/Old/o.dll", None),
                                                  ("archive/pc/mod/Edits.xl", None), ("archive/pc/mod/Edits.archive", None),
                                                  ("red4ext/plugins/EditsRuntime/resources/archives/Edits.xl", None),
                                                  ("archive/pc/mod/Garage_TEST10.archive", None), ("archive/pc/mod/Garage_TEST10.archive.xl", None),
                                                  ("red4ext/plugins/GarageRuntime/resources/archives/Garage.archive", None),
                                                  ("archive/pc/mod/###-Glass Fix.archive", None), ("archive/pc/mod/Old Glass.archive", None)]);
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
            [2026-11-04 03:06:07.036] [info    ] [ 1] [GlassFixNative] fine\n\
            [2026-11-04 03:06:07.036] [error   ] [ 1] [GlassFixNative] Glass Fix 1.x is still installed (D:\\Games\\Cyberpunk 2077\\archive\\pc\\mod\\###-Glass Fix.archive). Delete it: it bakes glass into meshes.\n\
            [2026-11-04 03:06:07.036] [error   ] [ 1] [GlassFixNative] An outdated copy is at archive/pc/mod/Old Glass.archive and must go.\n").unwrap();
        // files a mod's log says are in its way: a whole mod entry, and one inside another mod's folder
        let in_way = |w: &str, f: &str, p: &str| fs::write(logs.join(format!("{}-2026-11-04-03-06-08.log", w.to_lowercase())),
            format!("[2026-11-04 03:06:08.384] [error   ] [ 1] [{w}] {f} disabled: Conflicting older Garage component: {p}\n")).unwrap();
        in_way("AmbienceRuntime", "GL Ambience", "archive/pc/mod/Garage_TEST10.archive");
        in_way("GarageRuntime", "GL Garage", "red4ext/plugins/GarageRuntime/resources/archives/Garage.archive");
        let other = ["Conflicting older ArchiveXL: red4ext/plugins/ArchiveXL/ArchiveXL.dll", "Conflicting older component: archive/pc/mod/Missing.archive",
                     "Can't read the folder archive/pc/mod/Garage_TEST10.archive.xl"];      // a framework, not installed, nothing in the way
        fs::write(logs.join("otherruntime-2026-11-04-03-06-09.log"), other.map(|m| format!("[2026-11-04 03:06:09.000] [error   ] [ 1] [OtherRuntime] {m}\n")).concat()).unwrap();
        fs::write(logs.join("oldmod-2026-11-03-20-00-01.log"), "[x] [error] [OldMod] from an earlier launch\n").unwrap();
        let axl = (1..=4).map(|i| format!("[2026-11-04 03:10:46.000] [37264] [error] [ResourceManager] Resource {i} is missing.\n")).collect::<String>();
        let world = |xl: &str, what: &str| format!("[2026-11-04 03:10:47.000] [37264] [error] [WorldStreaming] {xl}: The target {what}, but the mod expects 1383.\n");
        let edits = [world("Edits.xl", "node #15 has type worldCookedStaticMeshNode"), world("Edits.xl", "node #15 has type worldCookedStaticMeshNode"),
                     world("Edits.xl", "sector has 1386 node(s)"), world("Elsewhere.xl", "sector has 9 node(s)")].concat();
        fs::write(root.join("red4ext/plugins/ArchiveXL/ArchiveXL-2026-11-04-03-10-45.log"), format!("[2026-11-04 03:10:45.789] [37264] [error] Can't patch.\n{axl}{edits}")).unwrap();
        let reports = root.join("bin/x64/plugins/AdvancedCrashReporter/reports");
        fs::create_dir_all(&reports).unwrap();
        fs::write(reports.join("20261104-030610-crash-tid1-01.txt"), "# REPORT\nGame version   : 2.5 (3.0.88.46235, built 2026-10-19)\nVerdict        : Likely culprit: ArchiveXL\n\n\
            Mod hooks on the call path (NOT running):\n  HookMod.dll                   [MOD-RED4EXT ] frame #11\n    in red4ext/plugins/HookMod\n\
              RED4ext.dll                    [LOADER      ] frame #30\n  -> These intercept an engine function [further up]\n\n\
            Mods present on OTHER threads (NOT evidence):\n  audioware.dll                  [MOD-RED4EXT ] 35 threads\n").unwrap();
        fs::write(reports.join("20261103-200000-crash-tid1-01.txt"), "Game version   : 2.5 (3.0.88.46235)\nVerdict        : Likely culprit: Older\n").unwrap();
        fs::write(reports.join("20261104-030700-hang-tid1-01.txt"), "Game version   : 2.5 (3.0.88.46235)\nVerdict        : Unknown culprit\n\n# PRIMARY STACK\n\
            ====\nThread 1  GameThread\n     [05] 0x00007FFADCA1DE5F  USER32.dll  +0x5DE5F  (MessageBoxTimeoutW+0x18F)  [SYSTEM]\n").unwrap();   // only redscript's error box
        let r = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap();
        let l = r.last.as_ref().unwrap();
        assert_eq!(l.when, "2026-11-04 03:05");
        assert_eq!(l.refused.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
                   ["Old Plugin", "Edits", "Garage_TEST10.archive", "Garage.archive (in GarageRuntime)", "###-Glass Fix.archive", "Old Glass.archive"], "a plugin no longer installed is left out");
        let e = |w: &str, m: &str| (w.to_string(), m.to_string());
        assert_eq!(l.errors, [e("RED4ext", "The game's crash report says: Watchdog timeout! (120 seconds)"), e("GlassFixNative", "Glass Fix 1.x is still installed."),
                              e("OtherRuntime", other[0]), e("OtherRuntime", other[1]), e("OtherRuntime", other[2]),
                              e("ArchiveXL", "Can't patch."), e("ArchiveXL", "[ResourceManager] Resource 1 is missing."), e("ArchiveXL", "[ResourceManager] Resource 2 is missing."),
                              e("ArchiveXL", "...and 2 more in its log."), e("ArchiveXL", "Elsewhere.xl's world edits no longer fit the map in 1 place.")]);
        let (c, report) = l.crash.as_ref().unwrap();
        assert!(c.contains("crashed") && c.contains("Likely culprit: ArchiveXL.") && report.ends_with("20261104-030610-crash-tid1-01.txt"), "{c} {report}");
        assert!(c.ends_with("Also hooked in on the call path: HookMod. ArchiveXL itself matches the list for game 2.5, so a mod that uses it is the likelier cause."), "{c}");
        let old = r.rows.iter().find(|x| x.name == "Old Plugin").unwrap();
        assert!(old.status == Status::Red && old.nexus_id.is_none() && old.why.contains("last launch (2026-11-04 03:05)") && old.why.contains("an older game"));
        let ed = r.rows.iter().find(|x| x.name == "Edits").unwrap();
        assert!(ed.status == Status::Red && ed.why.contains("ArchiveXL skipped its world edits in 2 places"), "{}", ed.why);
        assert_eq!(ed.remove, [("archive/pc/mod/Edits.xl".to_string(), Part::Own), ("archive/pc/mod/Edits.archive".into(), Part::Kin)]);
        assert!(ed.why.ends_with("Another copy is in red4ext/plugins/EditsRuntime/resources/archives/Edits.xl: if it's left from an older version, \
                                  move it out by hand first and launch again, as it may be the one that no longer fits."), "{}", ed.why);
        let row = |n: &str| r.rows.iter().find(|x| x.name == n).unwrap();
        let t10 = row("Garage_TEST10.archive");
        assert_eq!(t10.why, "AmbienceRuntime's log at your last launch (2026-11-04 03:05): \"GL Ambience disabled: Conflicting older Garage component: \
                             archive/pc/mod/Garage_TEST10.archive\" If it's left from an older version, back it up and remove it.");
        assert!(t10.status == Status::Red && t10.remove == [("archive/pc/mod/Garage_TEST10.archive".to_string(), Part::Own), ("archive/pc/mod/Garage_TEST10.archive.xl".into(), Part::Kin)]);
        let inner = row("Garage.archive (in GarageRuntime)");
        assert!(inner.status == Status::Red && inner.remove.is_empty() && inner.why.ends_with("It's in GarageRuntime's folder, and Back up and remove only takes whole mods: \
                 if it's left from an older version, move red4ext/plugins/GarageRuntime/resources/archives/Garage.archive out by hand and keep a copy."), "{}", inner.why);
        assert_eq!(row("###-Glass Fix.archive").remove, [("archive/pc/mod/###-Glass Fix.archive".to_string(), Part::Own)]);
        assert!(row("Old Glass.archive").why.starts_with("GlassFixNative's log at your last launch (2026-11-04 03:05): \"An outdated copy is at archive/pc/mod/Old Glass.archive"));
        assert!(!r.unlisted.contains(&"Old".to_string()) && r.unlisted.contains(&"Gone".to_string()));

        let o = root.join("red4ext/plugins/Old/o.dll");                    // updated or removed since that launch: no longer reported
        for l in ["red4ext-2026-11-04-03-05-54.log", "ambienceruntime-2026-11-04-03-06-08.log"] {
            fs::File::options().write(true).open(logs.join(l)).unwrap().set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
        }
        fs::write(&o, b"MZ new").unwrap();
        fs::write(root.join("archive/pc/mod/Garage_TEST10.archive"), b"new").unwrap();
        fs::remove_file(root.join("archive/pc/mod/Old Glass.archive")).unwrap();
        assert_eq!(scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap().last.unwrap().refused.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
                   ["Edits", "Garage.archive (in GarageRuntime)", "###-Glass Fix.archive"]);
        fs::remove_file(reports.join("20261104-030610-crash-tid1-01.txt")).unwrap();   // the error box alone is no freeze
        assert!(scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap().last.unwrap().crash.is_none());

        let other = install("last-other", [2, 5, 0, 0], &[]);                  // a log from another game exe is ignored
        fs::create_dir_all(other.join("red4ext/logs")).unwrap();
        fs::write(other.join("red4ext/logs/red4ext-2026-11-04-03-05-54.log"), "[RED4ext] File version: 3.0.80.51928\n").unwrap();
        assert!(scan(&Manifest::parse(LIST).unwrap(), &other, &[], &NO_ENV).unwrap().last.is_none());
    }

    #[test]
    fn closed_over_a_script() {
        let root = install("closed", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("red4ext/plugins/Scope/Scope.dll", None), ("r6/scripts/Scope.reds", None),
                                                    ("r6/scripts/Scope/new.reds", None), ("archive/pc/mod/Scope.xl", None),
                                                    ("r6/scripts/Gadget/a.reds", None), ("red4ext/plugins/Gadget/g.dll", None),
                                                    ("archive/pc/mod/Blur.archive", None), ("archive/pc/mod/Blur.archive.xl", None), ("red4ext/plugins/Blur/b.dll", None)]);
        let logs = root.join("red4ext/logs");
        fs::create_dir_all(&logs).unwrap();
        let line = |m: &str| format!("[2026-11-04 11:37:24.387] [error   ] [ 1] [RED4ext] Script validation error: {m}\n");
        let g = "F:\\Games\\Cyberpunk 2077\\r6\\scripts";
        fs::write(logs.join("red4ext-2026-11-04-11-37-04.log"), ["[RED4ext] File version: 3.0.88.46235\n".to_string(),
            line(&format!("Missing native global function 'Scope.Crumb' at {g}\\Scope.reds:3")), line(&format!("Missing native global function 'Scope.Trace' at {g}\\Scope.reds:5")),
            line(&format!("Missing native class 'Gadget.Native' at {g}\\Gadget\\a.reds:12")), line("Missing native global function 'Gone.X' at r6/scripts/Gone.reds:1")].concat()).unwrap();
        fs::write(logs.join("blur-2026-11-04-11-37-10.log"), "[2026-11-04 11:37:10.000] [error   ] [ 1] [Blur] Blur 1.x is still installed: archive/pc/mod/Blur.archive\n").unwrap();
        let r = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap();
        let row = |n: &str| r.rows.iter().find(|x| x.name == n).unwrap();
        let scope = row("Scope.reds");
        assert_eq!(scope.why, "At your last launch (2026-11-04 11:37), RED4ext closed the game over Scope.reds: \"Missing native global function 'Scope.Crumb'\" and 1 more. \
                               No loaded plugin provides it, so it's likely left from an older version of a plugin: back it up and remove it.");
        assert_eq!(scope.remove, [("r6/scripts/Scope.reds".to_string(), Part::Own)], "never the plugin or a folder of the same name, nor files elsewhere");
        assert_eq!(row("Gadget").remove, [("r6/scripts/Gadget".to_string(), Part::Own), ("red4ext/plugins/Gadget".into(), Part::Kin)]);
        assert!(row("Gadget").why.contains("over a.reds: \"Missing native class 'Gadget.Native'\". The plugin it needs is missing"), "{}", row("Gadget").why);
        assert_eq!(row("Blur.archive").remove, [("archive/pc/mod/Blur.archive".to_string(), Part::Own), ("archive/pc/mod/Blur.archive.xl".into(), Part::Kin)]);
        let errors = &r.last.as_ref().unwrap().errors;
        assert_eq!(errors[0], ("RED4ext".to_string(), "Closed the game at startup: Scope.reds and Gadget need native code that no loaded plugin provides.".to_string()));
        assert!(errors.iter().any(|(_, m)| m.ends_with("at r6/scripts/Gone.reds:1")), "a script that's gone stays a log line: {errors:?}");
    }

    #[test]
    fn ran_out_of_memory() {
        let root = install("oom", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("red4ext/plugins/ArchiveXL/ArchiveXL.dll", None)]);
        let logs = root.join("red4ext/logs");
        fs::create_dir_all(&logs).unwrap();
        let log = |m: &str| fs::write(logs.join("red4ext-2026-11-04-11-51-43.log"), format!("[2026-11-04 11:51:43.000] [info    ] [ 1] [RED4ext] File version: 3.0.88.46235\n\
            [2026-11-04 11:57:16.724] [error   ] [ 1] [RED4ext] Crash report\n[2026-11-04 11:57:16.724] [error   ] [ 1] [RED4ext] Message: {m}\n")).unwrap();
        log("Out of Memory! Failed to allocate 33554432 bytes with alignment 1 from pool 'PoolResource' using 'DefaultAllocator'");
        let last = || { let l = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap().last.unwrap(); (l.crash.map(|c| c.0), l.memory) };
        let (c, p) = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap().last.unwrap().crash.unwrap();   // no crash reporter: RED4ext's log says it
        assert_eq!(c, format!("Your last launch crashed because the game ran out of memory: {FREE}."));
        assert!(p.ends_with("red4ext-2026-11-04-11-51-43.log"), "{p}");
        let reports = root.join("bin/x64/plugins/AdvancedCrashReporter/reports");
        fs::create_dir_all(reports.join("watch")).unwrap();
        let report = |file: &str, at: &str, commit: &str, game: &str, near: bool| fs::write(reports.join(file), format!("Game version   : 2.5 (3.0.88.46235, built 2026-10-19)\n\
            Generated     : 2026-11-04 {at}.040\nVerdict        : Likely culprit: ArchiveXL\n\n# PC CONFIGURATION\n====\nAt the time of this report:\n  RAM in use     : 27.5 GB of 31.8 GB (86%)\n  \
            Commit charge  : {commit} (RAM + page file){}\n  Game process   : {game} committed, 10.1 GB in RAM (peak 10.8 GB)\n",
            if near { "   <== NEAR THE LIMIT: past it, memory requests fail and the game crashes" } else { "" })).unwrap();
        let crash = |commit, near| report("20261104-115718-crash-tid1-02.txt", "11:57:18", commit, "16.3 GB", near);
        crash("47.1 GB of 47.4 GB", true);
        assert_eq!(last(), (Some(format!("Your last launch crashed because the game ran out of memory (47.1 GB of 47.4 GB committed, 16.3 GB of it by the game): {FREE}. \
                                          Advanced Crash Reporter's verdict (Likely culprit: ArchiveXL) likely doesn't apply.")), None), "and no note that a mod using ArchiveXL is likelier");
        log("Watchdog timeout! (120 seconds)");                  // nearly full, but no allocation failed
        assert_eq!(last(), (Some("Your last launch crashed. Advanced Crash Reporter's verdict: Likely culprit: ArchiveXL. \
                                  ArchiveXL itself matches the list for game 2.5, so a mod that uses it is the likelier cause.".into()),
                            Some(format!("Memory was nearly full when it crashed (47.1 GB of 47.4 GB committed, 16.3 GB of it by the game), \
                                          and running out can crash the game or cut its sound: {FREE}."))));
        crash("31.5 GB of 36.5 GB", false);
        assert!(last().1.is_none());
        fs::remove_file(reports.join("20261104-115718-crash-tid1-02.txt")).unwrap();   // no crash: the newest snapshot of this launch
        report("watch/watch-06.txt", "11:50:00", "52.5 GB of 53.7 GB", "22.4 GB", true);
        assert_eq!(last(), (None, None), "a snapshot from an earlier launch");
        report("watch/watch-07.txt", "12:11:46", "52.5 GB of 53.7 GB", "22.4 GB", true);
        assert_eq!(last().1.unwrap(), format!("Memory was nearly full just before the game closed (52.5 GB of 53.7 GB committed, 22.4 GB of it by the game), \
                                               and running out can crash the game or cut its sound: {FREE}."));
    }

    #[test]
    fn named_after_the_character() {
        for (f, yes) in [("!!00_Misty_Goth_Redesign.archive", true), ("misty_explicit.xl", true), ("TheMistyMod", true), ("MISTY.archive", true), ("Mistyfied.archive", false), ("Hmisty.reds", false)] {
            assert_eq!(after(f, "misty"), yes, "{f}");
        }
        let root = install("misty", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("archive/pc/mod/!!00_Misty_Goth_Redesign.archive", None),
                                                   ("archive/pc/mod/misty_explicit.xl", None), ("archive/pc/mod/Mistyfied.archive", None),
                                                   ("archive/pc/mod/Citizen Variety.archive", None), ("r6/scripts/Allies/Characters/MistyModule.reds", None)]);
        let m = install("misty-MO2 Misty Hair", [0; 4], &[("archive/pc/mod/MistyHair.archive", None)]);
        fs::create_dir_all(root.join("red4ext/logs")).unwrap();
        fs::write(root.join("red4ext/logs/red4ext-2026-11-04-12-20-11.log"), "[RED4ext] File version: 3.0.88.46235\n").unwrap();
        let reports = root.join("bin/x64/plugins/AdvancedCrashReporter/reports");
        fs::create_dir_all(&reports).unwrap();
        let lead = |objs: &str| {
            fs::write(reports.join("20261104-123248-crash-tid1-01.txt"), format!("Game version   : 2.5 (3.0.88.46235, built 2026-10-19)\n\
                Verdict        : Possible culprit: mods not yet updated for game 2.5\nTechnical      : redDispatcher4 faulted in Cyberpunk2077.exe [GAME-EXE]; \
                no mod code executing; objects: {objs}; REDscope-internal logged a warning 0.021s before\n")).unwrap();
            scan(&Manifest::parse(LIST).unwrap(), &root, std::slice::from_ref(&m), &NO_ENV).unwrap().last.unwrap().lead
        };
        assert_eq!(lead("animRig, NPCPuppet misty.ent, NPCPuppet citizen__ep1_tenant_ma.ent, worldAnimationSystem").unwrap(), format!(
            "The crash report names misty.ent, so try without the mods named after Misty first: !!00_Misty_Goth_Redesign.archive, misty_explicit.xl \
             and MistyHair.archive (in {}).", m.file_name().unwrap().to_string_lossy()), "not one in another mod's folder");
        assert!(lead("animRig, NPCPuppet citizen__ep1_tenant_ma.ent").is_none(), "a passer-by");
    }

    #[test]
    fn crash_reporter_culprit() {
        let root = install("culprit", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("red4ext/plugins/ArchiveXL/ArchiveXL.dll", None),
            ("red4ext/plugins/BlurBegone/BlurBegoneNative.dll", None), ("r6/scripts/BlurBegone/b.reds", None), ("red4ext/plugins/Other/o.dll", None),
            ("bin/x64/plugins/Hook.asi", None), ("bin/x64/plugins/AdvancedCrashReporter.asi", None)]);
        let (acr, moved) = (root.join("bin/x64/plugins/AdvancedCrashReporter"), root.join("Crashes"));
        for d in [acr.join("reports"), moved.clone()] { fs::create_dir_all(d).unwrap() }
        // no RED4ext log: Advanced Crash Reporter's own log dates the launch
        fs::write(acr.join("AdvancedCrashReporter.log"), "[2026-11-04 03:25:00.001] [  0.001s] [INFO ] exception handler installed\n\
            [2026-11-04 03:25:02.000] [  2.169s] [INFO ] module #000 loaded: Cyberpunk2077.exe                [GAME-EXE    ] gap=0.000s v3.0.88.46235\n").unwrap();
        let crash = |dir: &Path, verdict: &str, running: &str| fs::write(dir.join("20261104-033004-crash-tid1-02.txt"), format!(
            "Game version   : 2.5 (3.0.88.46235, built 2026-10-19)\nVerdict        : {verdict}\n\n====\n# MOD INVOLVEMENT\n====\n\
             Mod code RUNNING on the analyzed thread:\n{running}  -> The strongest evidence there is: this mod's own code was executing when it\n     happened.\n\n\
             Mod hooks on the call path (NOT running):\n  none\n\nMods present on OTHER threads (NOT evidence):\n  o.dll                          [MOD-RED4EXT ] 2 threads\n\
                 in red4ext/plugins/Other\n\n====\n# PRIMARY STACK\n")).unwrap();
        let blur = "  BlurBegoneNative.dll           [MOD-RED4EXT ] frame #32\n    RED4ext plugin \"BlurBegoneNative\" 2.6.0 by CyanideX, in red4ext/plugins/BlurBegone\n";
        let rows = || { let r = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap(); (r.rows.iter().filter(|x| x.status == Status::Red).map(|x| x.name.clone()).collect::<Vec<_>>(), r) };
        crash(&acr.join("reports"), "Likely culprit: BlurBegone", blur);
        let (red, r) = rows();
        let (l, b) = (r.last.as_ref().unwrap(), r.rows.iter().find(|x| x.name == "BlurBegone").unwrap());
        assert!(l.when == "2026-11-04 03:25" && l.report.as_ref().is_some_and(|p| p.ends_with("reports/20261104-033004-crash-tid1-02.txt")) && red == ["BlurBegone"], "{red:?}");
        assert_eq!(b.why, "Likely culprit of your last launch's crash (2026-11-04 03:25), says Advanced Crash Reporter: its own code was running then. \
                           Update it, or back it up and remove it.");
        assert!(b.version.as_deref() == Some("2.6.0") && b.remove == [("red4ext/plugins/BlurBegone".to_string(), Part::Own), ("r6/scripts/BlurBegone".into(), Part::Kin)]);
        assert!(!r.unlisted.contains(&"BlurBegone".to_string()));
        crash(&acr.join("reports"), "Strong culprit: Hook", "  Hook.asi                       [MOD-ASI     ] frame #3\n");
        let (red, r) = rows();
        assert!(red == ["Hook.asi"] && r.rows[0].why.starts_with("Strong culprit of your last launch's crash") && r.rows[0].remove == [("bin/x64/plugins/Hook.asi".to_string(), Part::Own)], "{red:?}");
        // not a possible culprit, a framework, a mod on another thread, or a mod changed since
        for (v, running) in [("Possible culprit: BlurBegone", blur), ("Likely culprit: ArchiveXL", "  ArchiveXL.dll  [MOD-RED4EXT ] frame #0\n    in red4ext/plugins/ArchiveXL\n"),
                             ("Likely culprit: Other", "  none\n")] {
            crash(&acr.join("reports"), v, running);
            assert!(rows().0.is_empty(), "{v}");
        }
        crash(&acr.join("reports"), "Likely culprit: BlurBegone", blur);
        fs::File::options().write(true).open(acr.join("reports/20261104-033004-crash-tid1-02.txt")).unwrap().set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
        assert!(rows().0.is_empty(), "updated after the crash");

        // reports moved by config.ini's output_dir
        fs::remove_file(acr.join("reports/20261104-033004-crash-tid1-02.txt")).unwrap();
        crash(&moved, "Likely culprit: BlurBegone", blur);
        assert!(rows().1.last.unwrap().crash.is_none());
        fs::write(acr.join("config.ini"), format!("[general]\n; output_dir = elsewhere\noutput_dir = {}\n", moved.display())).unwrap();
        let (red, r) = rows();
        assert!(red == ["BlurBegone"] && r.last.unwrap().report.is_some_and(|p| p.starts_with(&moved)));

        // the game ran out of memory: the verdict likely doesn't apply, and neither does the culprit
        fs::create_dir_all(root.join("red4ext/logs")).unwrap();
        fs::write(root.join("red4ext/logs/red4ext-2026-11-04-03-25-01.log"), "[2026-11-04 03:25:01.000] [info    ] [ 1] [RED4ext] File version: 3.0.88.46235\n\
            [2026-11-04 03:30:03.000] [error   ] [ 1] [RED4ext] Crash report\n[2026-11-04 03:30:03.000] [error   ] [ 1] [RED4ext] Message: Out of Memory!\n").unwrap();
        let (red, r) = rows();
        assert!(red.is_empty() && r.last.unwrap().crash.unwrap().0.contains("ran out of memory"));
    }

    #[test]
    fn never_offers_a_framework_or_a_wildcard() {
        let root = install("never", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("red4ext/plugins/ArchiveXL/ArchiveXL.dll", None),
            ("r6/scripts/ArchiveXL/x.reds", None), ("red4ext/plugins/Codeware/Codeware.dll", None), ("r6/scripts/Codeware/c.reds", None), ("r6/scripts/Other/o.reds", None)]);
        fs::create_dir_all(root.join("red4ext/logs")).unwrap();
        fs::write(root.join("red4ext/logs/red4ext-2026-11-04-03-27-39.log"), "[RED4ext] File version: 3.0.88.46235\n").unwrap();
        fs::create_dir_all(root.join("r6/logs")).unwrap();
        let e = |m: &str| format!("[ERROR - Wed, 4 Nov 2026 03:28:29 -0500] [UNRESOLVED_TYPE] At D:\\Games\\Cyberpunk 2077\\r6\\scripts\\{m}\\a.reds:3:1:\n\
            let x: T;\n^^^^^^\nunresolved type 'T'\n\n");
        fs::write(root.join("r6/logs/redscript_rCURRENT.log"), [e("ArchiveXL"), e("Codeware"), e("*")].concat()).unwrap();
        // LIST leaves Codeware out, as a newer list next to the app might: the built-in list still protects it
        let r = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap();
        for n in ["ArchiveXL", "Codeware", "*"] { assert!(r.rows.iter().any(|x| x.name == n && x.status == Status::Red), "{n}") }
        let offered: Vec<&String> = r.rows.iter().flat_map(|x| &x.remove).map(|(p, _)| p).collect();
        assert!(offered.is_empty(), "{offered:?}");
    }

    #[test]
    fn redscript_errors() {
        let mods = ["ScriptMod", "Fine", "Addon", "Twice", "Ext"].map(|m| format!("r6/scripts/{m}/a.reds"));
        let files: Vec<(&str, Option<[u16; 4]>)> = [("red4ext/RED4ext.dll", Some([1, 31, 0, 0]))].into_iter().chain(mods.iter().map(|m| (m.as_str(), None))).collect();
        let root = install("redscript", [2, 5, 0, 0], &files);
        fs::create_dir_all(root.join("red4ext/logs")).unwrap();
        fs::write(root.join("red4ext/logs/red4ext-2026-11-04-03-27-39.log"), "[RED4ext] File version: 3.0.88.46235\n").unwrap();
        fs::create_dir_all(root.join("r6/logs")).unwrap();
        let e = |code: &str, m: &str, file: &str, msg: &str| format!("[ERROR - Wed, 4 Nov 2026 03:28:29 -0500] [{code}] At D:\\Games\\Cyberpunk 2077\\r6\\scripts\\{m}\\{file}:3:1:\n\
            @wrapMethod(X)\n^^^^^^^^^^^^^^\n{msg}\n\n");
        let err = |m: &str, msg: &str| e("UNRESOLVED_METHOD", m, "x.reds", msg);
        let log = format!("[INFO - Wed, 4 Nov 2026 03:28:26 -0500] Compiling files in D:\\Games\\Cyberpunk 2077\\r6\\scripts:\nScriptMod\\a.reds\n{}",
            [err("ScriptMod", "no method with this name exists on the target type"), err("ScriptMod", "another"), err("Removed", "gone since"),
             e("UNRESOLVED_IMPORT", "Addon", "a.reds", "module 'Lib.Core' has no members or does not exist"), e("UNRESOLVED_IMPORT", "Addon", "a.reds", "module 'Lib.Core.Json' has no members or does not exist"),
             e("UNRESOLVED_REF", "Addon", "a.reds", "unresolved reference 'LibThing'"),
             e("SYM_REDEFINITION", "Twice", "Twice.Json_1.reds", "the name of this type conflicts with a type defined at 'D:\\Games\\Cyberpunk 2077\\r6\\scripts\\Twice\\Twice.Json.reds:7:1'"),
             e("UNRESOLVED_TYPE", "Ext", "a.reds", "unresolved type 'TagEvent'"), e("UNRESOLVED_REF", "Ext", "a.reds", "unresolved reference 'ExtConfig'"),
             e("UNRESOLVED_REF", "Ext", "a.reds", "unresolved reference 'ExtConfig'"), e("UNRESOLVED_MEMBER", "Ext", "a.reds", "member 'changed' not found on 'IScriptable'")].concat());
        fs::write(root.join("r6/logs/redscript_rCURRENT.log"), &log).unwrap();
        let r = scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap();
        assert_eq!(r.last.as_ref().unwrap().errors, [("redscript".to_string(), "Couldn't compile 4 mods, so no script mod ran.".to_string())]);
        let why = |n: &str| r.rows.iter().find(|x| x.name == n).map(|x| { assert!(x.status == Status::Red && x.nexus_id.is_none()); x.why.as_str() }).unwrap();
        assert!(why("ScriptMod").ends_with("2 errors such as \"no method with this name exists on the target type\": it likely needs an update for this game version."), "{}", why("ScriptMod"));
        assert!(why("Addon").contains("It builds on Lib.Core, which no installed mod provides"), "{}", why("Addon"));
        assert!(why("Twice").contains("installed twice (Twice.Json_1.reds repeats Twice.Json.reds)"), "{}", why("Twice"));
        assert!(why("Ext").contains("2 names that nothing installed defines, such as ExtConfig and TagEvent: a mod it needs is missing"), "{}", why("Ext"));
        assert!(!r.rows.iter().any(|x| x.name == "Removed" || x.name == "Fine"));
        fs::write(root.join("r6/logs/redscript_rCURRENT.log"), log.replace("03:28:", "02:28:")).unwrap();   // from before that launch
        assert!(scan(&Manifest::parse(LIST).unwrap(), &root, &[], &NO_ENV).unwrap().last.unwrap().errors.is_empty());
        assert_eq!(redscript_stamp("[WARN - Sat, 31 Oct 2026 23:59:01 +0100] x"), Some("20261031235901".into()));
        assert_eq!(redscript_stamp("[ERROR] no date"), None);
    }

    #[test]
    fn what_remove_takes() {
        let list = format!("{LIST}\n[[pattern]]\nname = \"Old leftover\"\nall = [\"archive/pc/mod/Old-*.archive\"]\nremove = [\"archive/pc/mod/Old-*.archive\"]\n\
            text = \"t\"\nsource = \"s\"\ndate = \"d\"\n[[mod]]\nname = \"Broken\"\nnexus_id = 9\ndetect = [\"r6/scripts/Broken\"]\nneeds = [\"ArchiveXL\"]\n")
            .replacen("needs = [\"RED4ext\"]", "needs = [\"RED4ext\"]\nmin = { \"2.5\" = \"1.28.0\" }", 1);    // ArchiveXL 1.20 is too old, so Broken is red
        let files = [("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("r6/scripts/Broken/b.reds", None), ("r6/tweaks/broken/t.yaml", None),
                     ("archive/pc/mod/Broken.archive", None), ("archive/pc/mod/Broken.archive.xl", None), ("archive/pc/mod/BrokenAddon.archive", None),
                     ("bin/x64/plugins/cyber_engine_tweaks/mods/Broken/init.lua", None), ("archive/pc/mod/Old-1.0.archive", None),
                     ("red4ext/plugins/ArchiveXL/ArchiveXL.dll", Some([1, 20, 0, 0])), ("r6/scripts/ArchiveXL/x.reds", None)];
        let (r, _) = run(&list, [2, 5, 0, 0], &files, "remove", &NO_ENV);
        let of = |n: &str| r.rows.iter().find(|x| x.name == n).unwrap();
        assert_eq!(of("Broken").remove, [("r6/scripts/Broken".to_string(), Part::Own), ("archive/pc/mod/Broken.archive".into(), Part::Kin),
            ("archive/pc/mod/Broken.archive.xl".into(), Part::Kin), ("bin/x64/plugins/cyber_engine_tweaks/mods/Broken".into(), Part::Kin), ("r6/tweaks/broken".into(), Part::Kin)]);
        assert!(of("ArchiveXL").status == Status::Red && of("ArchiveXL").remove.is_empty(), "a framework is never offered");
        assert_eq!(of("Old leftover").remove, [("archive/pc/mod/Old-1.0.archive".to_string(), Part::Own)]);
        assert_eq!(unit("bin/x64/plugins/cyber_engine_tweaks/scripts/x.lua"), None);
        assert_eq!(unit("bin/x64/plugins/Fix.ASI").as_deref(), Some("bin/x64/plugins/Fix.ASI"));
        assert_eq!(unit("Archive/PC/Mod/a.archive").as_deref(), Some("Archive/PC/Mod/a.archive"));

        let game = install("remove-mo2-game", [2, 5, 0, 0], &[("red4ext/RED4ext.dll", Some([1, 31, 0, 0])), ("red4ext/plugins/ArchiveXL/ArchiveXL.dll", Some([1, 20, 0, 0]))]);
        let m = install("remove-mo2-Broken Mod", [0; 4], &[("r6/scripts/Broken/b.reds", None)]);
        let r = scan(&Manifest::parse(&list).unwrap(), &game, std::slice::from_ref(&m), &NO_ENV).unwrap();
        let b = r.rows.iter().find(|x| x.name == "Broken").unwrap();
        assert!(b.remove.is_empty() && b.mo2 == [m.file_name().unwrap().to_string_lossy().into_owned()], "MO2's files are left to MO2");
    }

    #[test]
    fn addons_that_build_on_it() {
        let (m, i, d, n) = reds("\u{feff}module A.B // c\nimport X.Y.*\nimport X.{P, Q}\n@if(ModuleExists(\"Z\"))\n\nimport Z.*\n/* import W.* */\nlet s = \"import V.*\";\n\
            public class K { func Inner() {} }\nfunc Top() {}\n@wrapMethod(K) func Wrapped() {}\nenum E { A = 0 }\n");
        assert_eq!((m.as_deref(), i, d), (Some("A.B"), vec!["X.Y".to_string(), "X".into()], vec!["K".to_string(), "Top".into(), "E".into()]));
        assert!(n.contains("Inner") && !n.contains("V") && !n.contains("W"));

        let list = format!("{LIST}\n[[mod]]\nname = \"Base\"\nnexus_id = 9\ndetect = [\"r6/scripts/Base\"]\nnotes = [{{ build = \"2.5\", text = \"t\", source = \"s\", date = \"d\" }}]\n");
        let code = [("r6/scripts/Base/core.reds", "module Base.Core\npublic class BaseThing {}\npublic func Helper() -> Int32 { return 1; }\n"),
            ("r6/scripts/Base/ui.reds", "module Base.UI\npublic class BaseView {}\n"),
            ("r6/scripts/Addon/a.reds", "module Addon.Stuff\nimport Base.UI.*\npublic class AddonThing {}\n"),        // only Base declares Base.UI
            ("r6/scripts/Second.reds", "import Addon.Stuff.AddonThing\n"),                                                  // builds on Addon
            ("r6/scripts/Patch/p.reds", "module Base.Core\n@wrapMethod(BaseThing)\npublic func Tick() { Helper(); }\n"),   // adds to Base's module
            ("red4ext/plugins/Gadget/g.reds", "import Base.UI.BaseView\n"),
            ("red4ext/plugins/ArchiveXL/x.reds", "import Base.UI.*\n"),                                                    // a framework: never offered
            ("r6/scripts/Quiet/q.reds", "module Base.Core\n// BaseThing\nclass Own { func F() -> String { return \"Helper\"; } }\n/* import Base.UI.* */\n"),
            ("r6/scripts/Optional/o.reds", "@if(ModuleExists(\"Base.UI\"))\nimport Base.UI.*\n\n@if(ModuleExists(\"Base.UI\")) import Base.UI.BaseView\n"),
            ("r6/scripts/Shared/s.reds", "import Base.Core.*\npublic class Helper { func F() -> ref<Helper> { return new Helper(); } }\n")];   // its own Helper
        let files: Vec<(&str, Option<[u16; 4]>)> = code.iter().map(|c| (c.0, None)).chain([("archive/pc/mod/Base.archive", None), ("archive/pc/mod/Addon.archive", None)]).collect();
        let game = install("addons", [2, 5, 0, 0], &files);
        let fan = install("addons-mo2-Fan Mod", [0; 4], &[("r6/scripts/Fan/f.reds", None)]);
        for (rel, c) in code { fs::write(game.join(rel), c).unwrap() }
        fs::write(fan.join("r6/scripts/Fan/f.reds"), "import Base.UI.BaseView\n").unwrap();
        let r = scan(&Manifest::parse(&list).unwrap(), &game, std::slice::from_ref(&fan), &NO_ENV).unwrap();
        let base = r.rows.iter().find(|x| x.name == "Base").unwrap();
        let on = |b: &str| Part::Uses(b.into());
        assert_eq!(base.remove, [("r6/scripts/Base".to_string(), Part::Own), ("archive/pc/mod/Base.archive".into(), Part::Kin),
            ("r6/scripts/Addon".into(), on("Base")), ("archive/pc/mod/Addon.archive".into(), on("Base")), ("r6/scripts/Patch".into(), on("Base")),
            ("r6/scripts/Second.reds".into(), on("Addon")), ("red4ext/plugins/Gadget".into(), on("Base"))]);
        assert_eq!(base.mo2, [fan.file_name().unwrap().to_string_lossy().into_owned()], "an add-on in MO2 is left to MO2");
        assert!(text(&r, "", "").contains("\tcan remove: r6/scripts/Base, archive/pc/mod/Base.archive, r6/scripts/Addon (builds on Base),"));
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
