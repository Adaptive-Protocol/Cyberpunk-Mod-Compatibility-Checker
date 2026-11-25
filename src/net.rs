//! The **Framework Net** (CONTEXT.md): what each mod needs and changes, read from its own files, and what that means
//! together. Read-only, like check.rs. A row is red only for a certain break; an **Overlap** is a neutral note.
use crate::check::{Hook, Reds, Script, cmp_ver, count, ext, files, key, list, read, resolve, walk, wild};
use serde::Deserialize;
use std::{collections::{BTreeMap, BTreeSet}, fs, io::{self, Read, Seek, SeekFrom}, path::{Path, PathBuf}, sync::LazyLock};

pub const BUILT_IN: &str = include_str!("../net.toml");
static BUILT: LazyLock<List> = LazyLock::new(|| List::parse(BUILT_IN).expect("built-in net.toml is valid"));

/// The Net's own list (net.toml), separate from compat.toml so that v1.x apps, which reject anything new there, keep
/// working (docs/adr/0001-net-list-file.md). It names the **Suite cores** the app can't recognise from scripts alone.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct List {
    pub schema: u32,
    pub updated: String,
    #[serde(default)] pub core: Vec<Core>,
    #[serde(default)] pub reach: Vec<Fix>,
}

/// An archive **Suite core**: how to find it, what it needs, its versions by fingerprint, and its **Suite mods**.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Core {
    pub name: String,
    #[serde(default)] pub nexus_id: Option<u32>,
    pub detect: Vec<String>,
    #[serde(default)] pub needs: Vec<String>,           // framework names, as in compat.toml
    #[serde(default)] pub reach: Option<Reach>,
    #[serde(default)] pub versions: Vec<Version>,
    #[serde(default)] pub suite: Vec<Suite>,
    pub source: String,
    pub date: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Version { pub version: String, pub fingerprint: String }

/// Mods that need the core: a path glob, and the lowest core version they work with, when known.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Suite { pub detect: String, #[serde(default)] pub min: Option<String> }

/// A correction to how far a mod reaches, where its files mislead.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Fix { pub detect: String, pub reach: Reach, pub why: String }

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Reach { Contained, Engine }

impl List {
    pub const EMPTY: Self = Self { schema: 1, updated: String::new(), core: Vec::new(), reach: Vec::new() };

    pub fn parse(text: &str) -> Result<Self, String> {
        let l: Self = toml::from_str(text).map_err(|e| e.message().to_string())?;
        if l.schema != 1 { return Err(format!("unsupported schema {}", l.schema)) }
        let safe = |p: &str| !p.starts_with(['/', '\\']) && p.split(['/', '\\']).all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains(':'));
        for c in &l.core {
            if c.detect.is_empty() || c.nexus_id == Some(0) { return Err(format!("{}: a core needs a detect path and a real nexus_id", c.name)) }
            if let Some(p) = c.detect.iter().chain(c.suite.iter().map(|s| &s.detect)).find(|p| !safe(p)) { return Err(format!("{}: path {p:?} must stay inside the game folder", c.name)) }
            if let Some(v) = c.versions.iter().find(|v| u64::from_str_radix(&v.fingerprint, 16).is_err()) { return Err(format!("{}: fingerprint {:?} isn't hex", c.name, v.fingerprint)) }
        }
        if let Some(f) = l.reach.iter().find(|f| !safe(&f.detect)) { return Err(format!("reach: path {:?} must stay inside the game folder", f.detect)) }
        Ok(l)
    }

    /// The built-in list, or a valid net.toml in `dir` with a later `updated` date, with a warning when one was ignored.
    pub fn load(dir: Option<&Path>) -> (Self, Option<String>) {
        let Some(p) = dir.map(|d| d.join("net.toml")).filter(|p| p.is_file()) else { return (BUILT.clone(), None) };
        match read(&p, 4 << 20).ok_or_else(|| "unreadable".to_string()).and_then(|b| Self::parse(&String::from_utf8_lossy(&b))) {
            Ok(l) if l.updated > BUILT.updated => (l, None),
            Ok(_) => (BUILT.clone(), Some(format!("{} is older than the built-in Net list, so it was ignored.", p.display()))),
            Err(e) => (BUILT.clone(), Some(format!("{} couldn't be read ({e}), so the built-in Net list is used.", p.display()))),
        }
    }
}

/// FNV-1a 64, the game's own hash for paths inside archives, used here for fingerprints too.
fn fnv<'a>(b: impl IntoIterator<Item = &'a u8>) -> u64 { b.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x100_0000_01b3)) }

/// An .archive's file index, from its own bytes: the game files it holds (path hashes) and a fingerprint of the whole
/// index, which tells one build of an archive from another. `skip` moves the reader to the index (a seek on disk, a
/// read-and-drop inside a packed Candidate), so nothing is unpacked.
pub fn index<R: Read>(r: &mut R, skip: impl FnOnce(&mut R, u64) -> io::Result<u64>) -> Option<(Vec<u64>, u64)> {
    let (mut h, mut ih) = ([0u8; 16], [0u8; 28]);
    r.read_exact(&mut h).ok()?;
    if &h[..4] != b"RDAR" { return None }
    skip(r, u64::from_le_bytes(h[8..16].try_into().ok()?).checked_sub(16)?).ok()?;
    r.read_exact(&mut ih).ok()?;
    let n = |i: usize| u64::from(u32::from_le_bytes(ih[i..i + 4].try_into().unwrap()));
    let len = n(16) * 56 + n(20) * 16 + n(24) * 8;
    if len > 512 << 20 { return None }
    let mut body = Vec::with_capacity(len as usize);
    r.take(len).read_to_end(&mut body).ok()?;
    (body.len() as u64 == len).then(|| (body[..n(16) as usize * 56].chunks(56).map(|e| u64::from_le_bytes(e[..8].try_into().unwrap())).collect(), fnv(ih.iter().chain(&body))))
}

/// The index of an .archive on disk.
pub fn index_file(p: &Path) -> Option<(Vec<u64>, u64)> {
    index(&mut fs::File::open(p).ok()?, |f, n| f.seek(SeekFrom::Current(n as i64)))
}

/// A place in a mod's files: a path relative to the game folder (or as it would install, for a Candidate) and a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loc { pub file: String, pub line: usize }
impl std::fmt::Display for Loc {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{} line {}", self.file, self.line) }
}

/// A tweak record a mod edits (not one it creates): the field, and whether it sets it outright or only adds to or
/// removes from a list (`!append`, `!remove`), which leaves room for other mods.
#[derive(Clone, Debug)]
pub struct Rec { pub record: String, pub field: String, pub set: bool, pub at: Loc }

/// What a mod's files say about it.
#[derive(Default, Debug)]
pub struct Facts {
    pub modules: Vec<String>,                   // modules its scripts declare
    pub imports: Vec<(String, Loc)>,            // modules they need
    pub defs: Vec<(Option<String>, String, Loc)>,   // classes and functions they define, by module
    pub hooks: Vec<(Hook, String, Loc)>,        // methods they replace, wrap or add
    pub cet: Vec<(String, Loc)>,                // methods its CET scripts Override
    pub records: Vec<Rec>,                      // tweak records it edits
    pub archives: Vec<(String, PathBuf)>,       // its .archive files, read when needed
    pub files: BTreeMap<u64, String>,           // game files in its archives -> the archive
    pub prints: Vec<(String, u64)>,             // each archive's fingerprint
    pub uses: BTreeSet<&'static str>,           // frameworks its file types need
    pub plugins: Vec<String>,                   // native plugins, whose hooks can't be read
    pub probes: usize,                          // ModuleExists(..) probes for other mods
}

impl Facts {
    /// Adds what one file says, by where it installs (`rel`, from the game folder).
    pub fn add(&mut self, rel: &str, b: &[u8]) {
        let low = rel.to_ascii_lowercase();
        let t = || String::from_utf8_lossy(b);
        if low.ends_with(".reds") { self.script(rel, &crate::check::reds(&t())) }
        else if low.ends_with(".xl") { self.uses.insert("ArchiveXL"); }
        else if low.starts_with("r6/tweaks/") && (low.ends_with(".yaml") || low.ends_with(".yml")) { self.uses.insert("TweakXL"); self.records.extend(tweaks(rel, &t())) }
        else if low.starts_with("r6/tweaks/") && low.ends_with(".tweak") { self.uses.insert("TweakXL"); }
        else if low.starts_with("bin/x64/plugins/cyber_engine_tweaks/mods/") && low.ends_with(".lua") { self.uses.insert("Cyber Engine Tweaks"); self.cet.extend(overrides(rel, &t())) }
        else if low.starts_with("red4ext/plugins/") && low.ends_with(".dll") { self.uses.insert("RED4ext"); self.plugins.push(rel.to_string()) }
    }

    pub fn script(&mut self, file: &str, r: &Reds) {
        let at = |line| Loc { file: file.to_string(), line };
        self.modules.extend(r.module.clone());
        self.imports.extend(r.imports.iter().zip(&r.lines).map(|(m, &l)| (m.clone(), at(l))));
        self.defs.extend(r.defs.iter().map(|d| (r.module.clone(), d.clone(), at(0))));
        self.hooks.extend(r.hooks.iter().map(|(h, t, l)| (*h, t.clone(), at(*l))));
        self.probes += r.probes.len();
    }

    /// Engine-reaching when its scripts change game code (replace or wrap a method, or Override one in CET) or it edits
    /// records it didn't create; else contained.
    pub fn reach(&self) -> Reach {
        if self.hooks.iter().any(|h| matches!(h.0, Hook::Replace | Hook::Wrap)) || !self.cet.is_empty() || !self.records.is_empty() { Reach::Engine } else { Reach::Contained }
    }

    /// Reads the archives' indexes, once.
    pub fn read_archives(&mut self) {
        for (rel, p) in std::mem::take(&mut self.archives) {
            if let Some((hs, fp)) = index_file(&p) { self.prints.push((rel.clone(), fp)); for h in hs { self.files.entry(h).or_insert_with(|| rel.clone()); } }
        }
    }
}

/// The records a TweakXL .yaml edits, by field: top-level keys are records, the keys under them fields. A record with
/// `$type` or `$base` is the mod's own and left out. A flat key ("Record.field: value") sets that one field.
fn tweaks(file: &str, t: &str) -> Vec<Rec> {
    let merge = |l: &str| ["!append", "!remove", "!prepend"].iter().any(|t| l.contains(t));
    // YAML anchors (`&Poses`) whose items merge, so that a field set to `*Poses` merges too
    let lines: Vec<&str> = t.lines().collect();
    let anchors: Vec<&str> = lines.iter().enumerate().filter_map(|(i, l)| {
        let a = l.split('&').nth(1)?.split_whitespace().next()?;
        let d = l.len() - l.trim_start().len();
        lines[i + 1..].iter().take_while(|n| n.trim().is_empty() || n.len() - n.trim_start().len() > d || n.trim_start().starts_with('-')).any(|n| merge(n)).then_some(a)
    }).collect();
    let set = |v: &str| !v.starts_with('!') && !v.strip_prefix(['*', '&']).is_some_and(|a| anchors.contains(&a.split_whitespace().next().unwrap_or("")));
    let (mut out, mut recs, mut cur, mut own, mut indent): (Vec<Rec>, Vec<Rec>, Option<String>, bool, Option<usize>) = (vec![], vec![], None, false, None);
    for (n, raw) in t.trim_start_matches('\u{feff}').lines().enumerate() {
        let l = raw.split(" #").next().unwrap_or("").trim_end();
        if l.trim().is_empty() || l.trim_start().starts_with('#') { continue }
        let depth = l.len() - l.trim_start().len();
        let (k, v) = l.trim().split_once(':').map_or((l.trim(), ""), |(k, v)| (k.trim().trim_matches(['"', '\'']), v.trim()));
        let at = Loc { file: file.to_string(), line: n + 1 };
        if depth == 0 {
            if !own { out.append(&mut recs) }
            (recs, cur, own, indent) = (vec![], None, false, None);
            if v.is_empty() { cur = Some(k.to_string()) }
            else if let Some((r, f)) = k.rsplit_once('.') && !k.starts_with('-') { out.push(Rec { record: r.into(), field: f.into(), set: set(v), at }) }
        } else if let Some(r) = &cur {
            let ind = *indent.get_or_insert(depth);
            // a field with nothing after the colon holds a list or map below: set outright unless an item merges
            if depth == ind && !k.starts_with('-') {
                own |= k.starts_with('$') && ["$type", "$base", "$instances"].contains(&k);
                recs.push(Rec { record: r.clone(), field: k.to_string(), set: set(v), at });
            } else if (depth > ind || k.starts_with('-')) && merge(l) && let Some(f) = recs.last_mut() { f.set = false }
        }
    }
    if !own { out.extend(recs) }
    out
}

/// The methods a CET mod's Lua Overrides without calling the original: `Override("Class", "Method", function(self, wrapped)
/// ... end)` where `wrapped(` never follows. One that calls it chains like a redscript wrap, so it's left out.
fn overrides(file: &str, t: &str) -> Vec<(String, Loc)> {
    let lines: Vec<&str> = t.lines().collect();
    lines.iter().enumerate().filter_map(|(n, l)| {
        let r = l.split("Override(").nth(1)?;
        let q: Vec<&str> = r.split(['"', '\'']).collect();
        if q.len() < 5 || !q[0].trim().is_empty() || q[2].trim() != "," || q[1].contains(' ') || q[3].contains(' ') { return None }
        let params = q[4].split("function").nth(1).and_then(|f| f.split('(').nth(1)).and_then(|f| f.split(')').next()).unwrap_or("");
        let last = params.rsplit(',').next().unwrap_or("").trim();
        let calls = !last.is_empty() && last != "self" && lines[n..].iter().take(400).any(|x| x.replace(' ', "").contains(&format!("{last}(")) && !x.contains("function"));
        (!calls).then(|| (format!("{}.{}", q[1], q[3]), Loc { file: file.into(), line: n + 1 }))
    }).collect()
}

/// "DigitalVixenCore.Api4_9_14" -> ("DigitalVixenCore", "4.9.14"): a version-marker module, which a **Suite mod**
/// imports to require that version of its core (DigitalVixen's convention).
pub fn marker(m: &str) -> Option<(&str, String)> {
    let (parent, last) = m.rsplit_once('.')?;
    let nums = last.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    let parts: Vec<&str> = nums.split('_').collect();
    (parts.len() >= 2 && nums.len() < last.len() && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))).then(|| (parent, parts.join(".")))
}

/// Why modules a script imports are missing, in words: "needs DigitalVixenCore 4.9.14 or newer (the installed copy goes
/// up to 4.9.12)" for a version marker, else "builds on X, which no installed mod provides".
pub fn missing(mods: &[String], declared: &[&str]) -> String {
    let (mut old, mut none) = (vec![], vec![]);
    for m in mods {
        match marker(m) {
            Some((p, v)) => {
                let have = declared.iter().filter_map(|d| marker(d).filter(|(dp, _)| *dp == p).map(|x| x.1)).max_by(|a, b| cmp_ver(a, b));
                old.push(match have { Some(h) => format!("{p} {v} or newer (the installed copy goes up to {h})"), None => format!("{p} {v} or newer, which isn't installed") });
            }
            None => none.push(m.clone()),
        }
    }
    let mut s = vec![];
    if !old.is_empty() { s.push(format!("It needs {}: update or install it, or remove this one too.", list(&old))) }
    if !none.is_empty() { s.push(format!("It builds on {}, which no installed mod provides: install or put back the mod that does, or remove this one too.", list(&none))) }
    s.join(" ")
}

/// One mod as the Net sees it: its name, where it is (root, as in check.rs), its **Mod entries** and its facts.
#[derive(Debug, Default)]
pub struct Mod { pub name: String, pub root: usize, pub entries: Vec<String>, pub facts: Facts, pub reach: Option<Reach> }

/// A row in the Net's group: a title, whether it's red, and the lines under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row { pub title: String, pub lines: Vec<String> }

/// A certain break the Net found, for a red row: the entry to offer, the mod's name, and why.
#[derive(Debug, Clone)]
pub struct Break { pub root: usize, pub entry: String, pub name: String, pub why: String }

#[derive(Debug, Default)]
pub struct Net { pub cores: Vec<Row>, pub overlaps: Vec<Row>, pub details: Vec<Row>, pub breaks: Vec<Break>, pub engine: usize }

/// Groups the install's mod entries into mods: one per Mod Organizer 2 mod, else by the owner Vortex records, else by
/// name (X.archive, X.xl and r6/scripts/X are one mod). `skip` leaves out frameworks.
pub fn mods(roots: &[&Path], entries: &[Vec<String>], scripts: &[Script], owner: &dyn Fn(&str) -> Option<String>, skip: &dyn Fn(&str) -> bool, nl: &List) -> Vec<Mod> {
    let g = roots.len() - 1;
    let mut by: BTreeMap<(usize, String), Mod> = BTreeMap::new();
    for (r, l) in entries.iter().enumerate() {
        for u in l.iter().filter(|u| !skip(u)) {
            let (k, name) = if r != g { (String::new(), crate::check::name(roots[r])) } else if let Some(o) = owner(u) { (o.to_lowercase(), o) } else {
                let n = u.rsplit('/').next().unwrap_or(u);
                (key(u), n.split('.').next().filter(|s| !s.is_empty()).unwrap_or(n).to_string())
            };
            let m = by.entry((r, k)).or_insert_with(|| Mod { name, root: r, ..Default::default() });
            m.entries.push(u.clone());
            let p = roots[r].join(u);
            let mut add = |f: PathBuf| {
                let Ok(rel) = f.strip_prefix(roots[r]).map(|x| x.to_string_lossy().replace('\\', "/")) else { return };
                if ext(&f, &["archive"]) { m.facts.archives.push((rel, f)) }
                else if ext(&f, &["yaml", "yml", "lua", "xl", "tweak"]) { if let Some(b) = read(&f, 8 << 20) { m.facts.add(&rel, &b) } }
                else if ext(&f, &["dll"]) { m.facts.add(&rel, &[]) }
            };
            if p.is_dir() { walk(&p, 0, &mut add) } else { add(p) }
        }
    }
    for s in scripts.iter().filter(|s| !skip(&s.unit)) {
        if let Some(m) = by.values_mut().find(|m| m.root == s.root && m.entries.contains(&s.unit)) { m.facts.script(&s.file, s); m.facts.uses.insert("redscript"); }
    }
    let mut v: Vec<Mod> = by.into_values().collect();
    for m in &mut v {
        m.reach = nl.reach.iter().find(|f| m.entries.iter().any(|u| wild(&f.detect, u))).map(|f| f.reach);
    }
    v
}

impl Mod {
    pub fn reach(&self) -> Reach { self.reach.unwrap_or_else(|| self.facts.reach()) }
}

/// Version from a core's fingerprints, and how many installed archives are copies of it.
fn version<'a>(c: &'a Core, m: &Mod) -> Option<&'a str> {
    m.facts.prints.iter().find_map(|(_, fp)| c.versions.iter().find(|v| u64::from_str_radix(&v.fingerprint, 16).ok() == Some(*fp)).map(|v| v.version.as_str()))
}

/// The Net over the installed mods. `has` says whether a framework (by its compat.toml name) is installed.
/// `fw` holds the modules frameworks declare (Codeware's), which make no mod a core.
pub fn net(mods: &mut [Mod], nl: &List, has: &dyn Fn(&str) -> bool, fw: &BTreeSet<String>, mac: bool) -> Net {
    let mut n = Net::default();
    // archive cores from the list, with the mods that match its suite globs
    let mut suite: Vec<Option<usize>> = vec![None; mods.len()];     // mod -> the core mod it belongs to
    let mut cores: Vec<(usize, &Core)> = vec![];
    let mut patch = vec![false; mods.len()];                        // an add-on of an archive core, which patches its whole suite
    for c in &nl.core {
        let found: Vec<usize> = (0..mods.len()).filter(|&i| mods[i].entries.iter().any(|u| c.detect.iter().any(|d| wild(d, u)))).collect();
        let users: Vec<usize> = (0..mods.len()).filter(|i| !found.contains(i) && mods[*i].entries.iter().any(|u| c.suite.iter().any(|s| wild(&s.detect, u)))).collect();
        if found.is_empty() {
            for &i in &users {
                n.breaks.push(Break { root: mods[i].root, entry: mods[i].entries[0].clone(), name: mods[i].name.clone(), why: format!(
                    "It's part of {}'s suite and needs {}, which isn't installed. Install it{}, or remove this one too.", c.name, c.name, c.nexus_id.map_or(String::new(), |id| format!(" (nexusmods.com/cyberpunk2077/mods/{id})"))) });
            }
            continue
        }
        for &i in found.iter().chain(&users) { suite[i] = Some(found[0]) }
        for &i in &users { patch[i] = mods[i].entries.iter().any(|u| c.detect.iter().any(|d| addon(u, d))) }
        cores.push((found[0], c));
        for &i in &found { mods[i].facts.read_archives() }
        let all: Vec<&String> = found.iter().flat_map(|&i| &mods[i].entries).collect();
        let copies = c.detect.iter().map(|d| all.iter().filter(|u| wild(d, u)).map(|s| s.to_string()).collect::<Vec<_>>()).find(|v| v.len() > 1);
        if let Some(e) = copies {
            n.breaks.push(Break { root: mods[found[0]].root, entry: e[1].clone(), name: c.name.clone(), why: format!("{} is installed twice ({}). Keep one copy: remove the older one.", c.name, list(&e)) });
        }
        let have = version(c, &mods[found[0]]);
        for &i in &users {
            let Some(min) = c.suite.iter().find(|s| mods[i].entries.iter().any(|u| wild(&s.detect, u))).and_then(|s| s.min.as_deref()) else { continue };
            if let Some(h) = have && cmp_ver(h, min).is_lt() {
                n.breaks.push(Break { root: mods[i].root, entry: mods[i].entries[0].clone(), name: mods[i].name.clone(), why: format!("It needs {} {min} or newer, and {h} is installed: update {}.", c.name, c.name) });
            }
        }
        for f in c.needs.iter().filter(|f| !has(f) || mac && MAC_LACKS.contains(&f.as_str())) {
            let why = if mac { format!("It needs {f}, which macOS doesn't have, so it can't work on a Mac.") } else { format!("It needs {f}, which isn't installed.") };
            n.breaks.push(Break { root: mods[found[0]].root, entry: mods[found[0]].entries[0].clone(), name: c.name.clone(), why });
        }
    }
    // script cores: mods whose modules other mods import
    let mut decl: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, m) in mods.iter().enumerate() { for d in &m.facts.modules { let v = decl.entry(d.as_str()).or_default(); if v.last() != Some(&i) { v.push(i) } } }
    let mut users: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    // a module several mods declare belongs to the one defining most in it; the others add to it (EquipmentEx's add-ons)
    let defs = |i: usize, d: &str| mods[i].facts.defs.iter().filter(|x| x.0.as_deref() == Some(d)).count();
    for (d, v) in decl.iter_mut().filter(|(d, v)| v.len() > 1 && !fw.contains(**d)) {
        v.sort_by_key(|&i| std::cmp::Reverse(defs(i, d)));
        let c = v[0];
        let dup = |i: usize| mods[i].facts.defs.iter().any(|x| x.0.as_deref() == Some(*d) && mods[c].facts.defs.iter().any(|y| y.0 == x.0 && y.1 == x.1));
        users.entry(c).or_default().extend(v[1..].iter().filter(|&&i| i != c && !dup(i)));
    }
    let owner = |p: &str| { let mut q = p; loop { if fw.contains(q) { return None } if let Some(v) = decl.get(q) { return Some(v) } q = &q[..q.rfind('.')?] } };
    for (i, m) in mods.iter().enumerate() {
        for (p, _) in &m.facts.imports {
            let p = marker(p).map_or(p.as_str(), |x| x.0);
            if let Some(o) = owner(p) && !o.contains(&i) { users.entry(o[0]).or_default().insert(i); }
        }
    }
    let mut twice: Vec<(usize, usize, String)> = vec![];           // the same class or function in one module, from two mods
    let mut seen: BTreeMap<(Option<&str>, &str), usize> = BTreeMap::new();
    for (i, m) in mods.iter().enumerate() {
        for (module, d, _) in &m.facts.defs {
            match seen.get(&(module.as_deref(), d.as_str())) {
                Some(&j) if j != i && !twice.iter().any(|t| t.0 == j && t.1 == i) => twice.push((j, i, module.as_ref().map_or_else(|| d.clone(), |m| format!("{m}.{d}")))),
                Some(_) => {}
                None => { seen.insert((module.as_deref(), d.as_str()), i); }
            }
        }
    }
    for (a, b, what) in twice {
        n.breaks.push(Break { root: mods[b].root, entry: mods[b].entries[0].clone(), name: mods[b].name.clone(), why: format!(
            "It and {} both define {what}, so redscript can't compile them together, which stops every script mod. If one is an older copy of the other, remove it.", mods[a].name) });
    }
    // mods that need a framework that isn't there (or, on a Mac, can't be)
    for m in mods.iter() {
        let lacks: Vec<String> = m.facts.uses.iter().filter(|f| mac && MAC_LACKS.contains(f)).map(|s| s.to_string()).collect();
        if lacks.is_empty() { continue }              // off a Mac, check.rs gives a missing framework one row of its own
        n.breaks.push(Break { root: m.root, entry: m.entries[0].clone(), name: m.name.clone(), why: format!("It needs {}, which macOS doesn't have, so it can't work on a Mac.", list(&lacks)) });
    }
    for (&c, us) in &users {
        suite[c] = suite[c].or(Some(c));
        for &u in us { suite[u] = suite[u].or(Some(c)) }
    }
    // rows for the cores
    for (&c, us) in &users {
        let m = &mods[c];
        let v = m.facts.modules.iter().filter_map(|d| marker(d).map(|x| x.1)).max_by(|a, b| cmp_ver(a, b));
        let mut lines: Vec<String> = us.iter().map(|&u| format!("Needed by {}", mods[u].name)).collect();
        lines.sort();
        n.cores.push(Row { title: format!("{}: script core, {}{}, {}", m.name, reach(m.reach()), v.map_or(String::new(), |v| format!(", version {v} (from its version markers)")), count(us.len(), "suite mod")), lines });
    }
    for &(c, core) in &cores {
        let us: Vec<String> = (0..mods.len()).filter(|&i| i != c && suite[i] == Some(c) && !mods[c].entries.iter().all(|e| mods[i].entries.contains(e))).map(|i| format!("Needed by {}", mods[i].name)).collect();
        let r = core.reach.unwrap_or_else(|| mods[c].reach());
        n.cores.push(Row { title: format!("{}: archive core, {}{}, {}", core.name, reach(r), version(core, &mods[c]).map_or(String::new(), |v| format!(", version {v}")), count(us.len(), "suite mod")), lines: us });
    }
    n.cores.sort_by_key(|r| r.title.to_lowercase());
    // overlaps: every engine-reaching mod against the others, and contained mods within their own suite
    let engine: Vec<bool> = mods.iter().map(|m| m.reach() == Reach::Engine).collect();
    n.engine = engine.iter().filter(|e| **e).count();
    let pair = |i: usize, j: usize| engine[i] && engine[j] || suite[i].is_some() && suite[i] == suite[j];
    for i in (0..mods.len()).filter(|&i| engine[i] || suite[i].is_some()) { mods[i].facts.read_archives() }
    overlaps(mods, &pair, &|i, j| patch[i] != patch[j], &mut n);
    n
}

/// Frameworks the Mac game can't load (the RED Modding wiki's "Modding on macOS"): only redscript runs there.
pub const MAC_LACKS: [&str; 5] = ["RED4ext", "Cyber Engine Tweaks", "ArchiveXL", "TweakXL", "Codeware"];

fn reach(r: Reach) -> &'static str { if r == Reach::Engine { "engine-reaching" } else { "contained" } }

/// "A file line 3" for a place, without the line when it isn't known.
fn at(l: &Loc) -> String { if l.line == 0 { l.file.clone() } else { l.to_string() } }

/// The **Overlaps** between mods that `pair` says to compare: the same method replaced twice, the same CET Override,
/// the same record field set twice, the same game file in two archives. A method one mod replaces and another wraps is
/// a detail line under the replacing mod; two wraps chain safely and aren't shown.
/// `meant` tells pairs whose shared archive files are by design, such as a core's camo add-on and the items it recolours.
pub fn overlaps(mods: &[Mod], pair: &dyn Fn(usize, usize) -> bool, meant: &dyn Fn(usize, usize) -> bool, n: &mut Net) {
    let mut by: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
    let mut detail: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut targets: BTreeMap<&str, Vec<(usize, Hook, &Loc)>> = BTreeMap::new();
    for (i, m) in mods.iter().enumerate() { for (h, t, l) in &m.facts.hooks { if matches!(h, Hook::Replace | Hook::Wrap) { targets.entry(t).or_default().push((i, *h, l)) } } }
    for (t, v) in &targets {
        for (k, &(i, hi, li)) in v.iter().enumerate() {
            for &(j, hj, lj) in &v[k + 1..] {
                if i == j || !pair(i, j) { continue }
                match (hi, hj) {
                    (Hook::Replace, Hook::Replace) => by.entry((i.min(j), i.max(j))).or_default().push(format!("Both replace {t} ({}; {}): only one of them takes effect.", at(li), at(lj))),
                    (Hook::Replace, Hook::Wrap) | (Hook::Wrap, Hook::Replace) => {
                        let (r, w, lw) = if hi == Hook::Replace { (i, j, lj) } else { (j, i, li) };
                        detail.entry(r).or_default().push(format!("{t}: {} wraps it ({}), so its code runs around {}'s version, not the game's.", mods[w].name, at(lw), mods[r].name));
                    }
                    _ => {}
                }
            }
        }
    }
    let mut cet: BTreeMap<&str, Vec<(usize, &Loc)>> = BTreeMap::new();
    for (i, m) in mods.iter().enumerate() { for (t, l) in &m.facts.cet { cet.entry(t).or_default().push((i, l)) } }
    for (t, v) in &cet {
        for (k, &(i, li)) in v.iter().enumerate() { for &(j, lj) in &v[k + 1..] { if i != j && pair(i, j) {
            by.entry((i.min(j), i.max(j))).or_default().push(format!("Both Override {t} in Cyber Engine Tweaks ({}; {}): if either skips the original, the other's change is lost.", at(li), at(lj)));
        } } }
    }
    let mut recs: BTreeMap<(&str, &str), Vec<(usize, &Loc)>> = BTreeMap::new();
    for (i, m) in mods.iter().enumerate() { for r in m.facts.records.iter().filter(|r| r.set) { recs.entry((&r.record, &r.field)).or_default().push((i, &r.at)) } }
    for ((r, f), v) in &recs {
        for (k, &(i, li)) in v.iter().enumerate() { for &(j, lj) in &v[k + 1..] { if i != j && pair(i, j) {
            by.entry((i.min(j), i.max(j))).or_default().push(format!("Both set {r}.{f} ({}; {}): only one value takes effect.", at(li), at(lj)));
        } } }
    }
    let mut shared: BTreeMap<(usize, usize), BTreeMap<(&str, &str), usize>> = BTreeMap::new();
    let mut owners: BTreeMap<u64, Vec<(usize, &str)>> = BTreeMap::new();
    for (i, m) in mods.iter().enumerate() { for (h, a) in &m.facts.files { owners.entry(*h).or_default().push((i, a)) } }
    for v in owners.values() {
        for (k, &(i, a)) in v.iter().enumerate() { for &(j, b) in &v[k + 1..] { if i != j && pair(i, j) && !meant(i, j) {
            let key = if i < j { ((i, j), (a, b)) } else { ((j, i), (b, a)) };
            *shared.entry(key.0).or_default().entry(key.1).or_default() += 1;
        } } }
    }
    for ((i, j), files) in shared {
        for ((a, b), c) in files.into_iter().filter(|((a, b), _)| !addon(a, b)) {
            let (fa, fb) = (a.rsplit('/').next().unwrap_or(a), b.rsplit('/').next().unwrap_or(b));
            let win = if fa.as_bytes() <= fb.as_bytes() { fa } else { fb };
            by.entry((i, j)).or_default().push(format!("{fa} and {fb} hold {c} of the same game {}: {win} loads first and wins.", if c == 1 { "file" } else { "files" }));
        }
    }
    for ((i, j), lines) in by { n.overlaps.push(Row { title: format!("{} and {}", mods[i].name, mods[j].name), lines }) }
    for (r, mut lines) in detail {
        lines.sort();
        lines.dedup();
        n.details.push(Row { title: format!("{} replaces {} that other mods wrap", mods[r].name, count(lines.len(), "method")), lines });
    }
}

/// Is one archive an add-on or patch of the other, by name? "x_armor_pads_addon_elbow" builds on "x_armor_pads", so their
/// shared files are meant. The same words with only a load-order prefix ("zz_X" and "X") are two copies, not an add-on.
fn addon(a: &str, b: &str) -> bool {
    let words = |p: &str| -> Vec<String> {
        let f = p.rsplit('/').next().unwrap_or(p).to_lowercase();
        f.split('.').next().unwrap_or("").split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !w.bytes().all(|b| b.is_ascii_digit() || b == b'z')).map(str::to_string).collect()
    };
    let (a, b) = (words(a), words(b));
    let (short, long) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    short.len() < long.len() && !short.is_empty() && long.windows(short.len()).any(|w| w == short.as_slice())
}

/// Every installed mod folder's archives under `dir`, for the game's own: hash -> the archive's name.
pub fn game_files(game: &Path) -> BTreeSet<u64> {
    ["archive/pc/content", "archive/pc/ep1"].iter().filter_map(|d| resolve(game, d)).flat_map(|d| files(&d)).filter(|p| ext(p, &["archive"]))
        .filter_map(|p| index_file(&p)).flat_map(|x| x.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_list_is_valid() { List::parse(BUILT_IN).unwrap(); }

    #[test]
    fn markers() {
        assert_eq!(marker("DigitalVixenCore.Api4_9_14"), Some(("DigitalVixenCore", "4.9.14".into())));
        assert_eq!(marker("RedscriptConfigFramework.Api3_2_1"), Some(("RedscriptConfigFramework", "3.2.1".into())));
        assert_eq!(marker("Foo.Bar"), None);
        assert_eq!(marker("Foo.V2"), None);
        let decl = ["DigitalVixenCore", "DigitalVixenCore.Api4_9_9", "DigitalVixenCore.Api4_9_12"];
        assert_eq!(missing(&["DigitalVixenCore.Api4_9_14".into(), "RedFunctions".into()], &decl),
            "It needs DigitalVixenCore 4.9.14 or newer (the installed copy goes up to 4.9.12): update or install it, or remove this one too. \
             It builds on RedFunctions, which no installed mod provides: install or put back the mod that does, or remove this one too.");
    }

    #[test]
    fn tweak_records() {
        let t = "Character.Player_Primary_Stats_Base:\n  statModifiers:\n    - !append Character.X\n  level: 5 # comment\n\
                 MyMod.Thing:\n  $type: gamedataItem_Record\n  quality: Quality.Rare\n\
                 Items.Flat.price: 100\n";
        let r = tweaks("r6/tweaks/a.yaml", t);
        let v: Vec<(&str, &str, bool, usize)> = r.iter().map(|r| (r.record.as_str(), r.field.as_str(), r.set, r.at.line)).collect();
        assert_eq!(v, [("Character.Player_Primary_Stats_Base", "statModifiers", false, 2), ("Character.Player_Primary_Stats_Base", "level", true, 4), ("Items.Flat", "price", true, 8)]);
    }

    #[test]
    fn cet_overrides() {
        let v = overrides("m/init.lua", "Observe('A', 'B', f)\n  Override(\"MinimapContainerController\", \"CreateMappinUIProfile\", function(self, w) end)\n\
            Override('InteractionUIBase', 'OnDialogsData', function(self, value, wrappedMethod)\n  wrappedMethod(value)\nend)\n");
        assert_eq!(v, [("MinimapContainerController.CreateMappinUIProfile".to_string(), Loc { file: "m/init.lua".into(), line: 2 })]);
    }

    #[test]
    fn archive_index() {
        let mut b = b"RDAR\x0c\0\0\0".to_vec();
        b.extend(40u64.to_le_bytes());
        b.extend([0; 24]);
        let mut ih = [0u8; 28];
        ih[16..20].copy_from_slice(&2u32.to_le_bytes());
        b.extend(ih);
        for h in [7u64, 9] { b.extend(h.to_le_bytes()); b.extend([0; 48]) }
        let (hs, fp) = index(&mut &b[..], |r, n| io::copy(&mut r.take(n), &mut io::sink())).unwrap();
        assert_eq!(hs, [7, 9]);
        assert_ne!(fp, 0);
    }

    fn m(name: &str, f: impl FnOnce(&mut Facts)) -> Mod { let mut m = Mod { name: name.into(), entries: vec![format!("r6/scripts/{name}")], ..Default::default() }; f(&mut m.facts); m }

    #[test]
    fn overlaps_and_details() {
        let s = |code: &str| crate::check::reds(code);
        let mut mods = vec![
            m("SortA", |f| f.script("r6/scripts/SortA/a.reds", &s("@replaceMethod(NewItemCompareBuilder)\npublic func QualityAsc() {}\n"))),
            m("SortB", |f| f.script("r6/scripts/SortB/b.reds", &s("@replaceMethod(NewItemCompareBuilder)\n\npublic func QualityAsc() {}\n"))),
            m("Wrapper", |f| f.script("r6/scripts/Wrapper/w.reds", &s("@wrapMethod(NewItemCompareBuilder)\npublic func QualityAsc() {}\n@wrapMethod(X)\nfunc Y() {}\n"))),
            m("Wrapper2", |f| f.script("r6/scripts/Wrapper2/w.reds", &s("@wrapMethod(X)\nfunc Y() {}\n"))),
            m("Plain", |f| f.script("r6/scripts/Plain/p.reds", &s("@addMethod(X)\nfunc Z() {}\n"))),
        ];
        let n = net(&mut mods, &List::parse("schema = 1\nupdated = \"x\"").unwrap(), &|_| true, &BTreeSet::new(), false);
        assert_eq!(n.engine, 4);
        assert_eq!(n.overlaps, [Row { title: "SortA and SortB".into(), lines: vec![
            "Both replace NewItemCompareBuilder.QualityAsc (r6/scripts/SortA/a.reds line 1; r6/scripts/SortB/b.reds line 1): only one of them takes effect.".into()] }]);
        assert_eq!(n.details.len(), 2, "each replacing mod gets the wrapper as a detail; two wraps aren't shown");
        assert!(n.details[0].lines[0].starts_with("NewItemCompareBuilder.QualityAsc: Wrapper wraps it (r6/scripts/Wrapper/w.reds line 1)"));
    }

    #[test]
    fn script_cores_and_twice() {
        let s = |code: &str| crate::check::reds(code);
        let mut mods = vec![
            m("DVCore", |f| f.script("r6/scripts/DVCore/a.reds", &s("module DigitalVixenCore.Api4_9_17\nmodule X\n"))),
            m("DVCore2", |f| { f.script("r6/scripts/DVCore/b.reds", &s("module DigitalVixenCore\npublic class Hub {}\n")) }),
            m("Combat", |f| f.script("r6/scripts/Combat/c.reds", &s("import DigitalVixenCore.Api4_9_14\nimport DigitalVixenCore.*\n"))),
            m("OldCopy", |f| f.script("r6/scripts/OldCopy/b.reds", &s("module DigitalVixenCore\npublic class Hub {}\n"))),
        ];
        let n = net(&mut mods, &List::parse("schema = 1\nupdated = \"x\"").unwrap(), &|_| true, &BTreeSet::new(), false);
        assert!(n.cores.iter().any(|c| c.title == "DVCore2: script core, contained, 1 suite mod" && c.lines == ["Needed by Combat"]), "{:?}", n.cores);
        assert_eq!(n.breaks.len(), 1);
        assert!(n.breaks[0].name == "OldCopy" && n.breaks[0].why.starts_with("It and DVCore2 both define DigitalVixenCore.Hub"));
    }
}
