//! The **Pre-install check**: reads a **Candidate** (a mod's .7z or .zip, or the folder of an extracted one) in memory
//! and checks it against the installed mods. It never installs, extracts, writes or runs anything: an archive is read
//! as a stream, and only the text files the checks read and each .archive's index are kept.

use crate::check::{cmp_ver, count, list, name, unit, walk, wild};
use crate::net::{self, Installed, Mod, Reach, Row};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::fs;

const MAX_FILES: usize = 200_000;       // entries in one archive
const MAX_SIZE: u64 = 64 << 30;         // all of them, unpacked
const MAX_TEXT: u64 = 16 << 20;         // one script, tweak or installer file
const MAX_KEPT: u64 = 256 << 20;        // all kept text together
pub const RAR: &str = "A .rar can't be read here. Extract it with 7-Zip or WinRAR, then drop the folder it made.";
const CANCELLED: &str = "Cancelled.";

/// What reading kept of one file: text the checks read, an .archive's index (game-file hashes and fingerprint),
/// or only that it's native code or another archive.
#[derive(Debug, Clone)]
pub enum Got { Text(Vec<u8>), Index(Vec<u64>, u64), Native, Nested }

/// A read's progress, shared with the window: bytes read of the total, and Cancel.
#[derive(Default, Debug)]
pub struct Progress { pub done: AtomicU64, pub total: AtomicU64, pub stop: AtomicBool }

/// A Candidate as read: its name (and, from a Nexus download's file name, its version and Nexus id), its files by
/// path inside the archive, those too large to read, and its FOMOD installer.
#[derive(Debug, Clone, Default)]
pub struct Candidate { pub name: String, pub version: Option<String>, pub nexus_id: Option<u32>, pub files: Vec<(String, Got)>, pub skipped: Vec<String>, pub fomod: Option<Fomod> }

impl Candidate {
    /// Nexus names downloads "Name-12345-1-3-0-1732500000": the mod's id, its version with dashes, and a time.
    fn named(stem: &str) -> Self {
        let bits: Vec<&str> = stem.split('-').collect();
        let num = |s: &&str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        match bits.iter().position(|b| num(b) && b.len() >= 2).filter(|&i| i > 0 && bits.len() >= i + 3 && bits[i..].iter().all(num) && bits[bits.len() - 1].len() >= 9) {
            Some(i) => Self { name: bits[..i].join("-").trim().into(), nexus_id: bits[i].parse().ok(), version: Some(bits[i + 1..bits.len() - 1].join(".")), ..Default::default() },
            None => Self { name: stem.trim().into(), ..Default::default() },
        }
    }
}

/// Reads a Candidate from a .7z, a .zip or a folder. Nothing is written anywhere.
pub fn read(p: &Path, pr: &Progress) -> Result<Candidate, String> {
    let file = name(p);
    let low = file.to_lowercase();
    let mut c = Candidate::named(if p.is_dir() { &file } else { file.rsplit_once('.').map_or(file.as_str(), |x| x.0) });
    let mut k = Keep { c: &mut c, kept: 0 };
    let r = if p.is_dir() { k.folder(p, pr) } else if low.ends_with(".7z") { k.sevenz(p, pr) } else if low.ends_with(".zip") { k.zip(p, pr) }
        else if low.ends_with(".rar") { Err(RAR.into()) } else { Err("Choose a mod's .7z or .zip, or the folder of an extracted one.".into()) };
    if pr.stop.load(Relaxed) { return Err(CANCELLED.into()) }      // a reader may swallow the stop, so ask here
    r.map_err(|e| if e.to_lowercase().contains("password") { "It's password-protected, so it can't be read.".into() } else { e })?;
    c.fomod = c.files.iter().find(|(f, _)| f.to_lowercase().ends_with("fomod/moduleconfig.xml")).and_then(|(f, g)| match g { Got::Text(b) => Fomod::parse(f, b), _ => None });
    Ok(c)
}

enum Want { Text, Index, Native, Nested }

/// What the checks need of a file, by its name.
fn want(low: &str) -> Option<Want> {
    Some(match low.rsplit('.').next().unwrap_or("") {
        "reds" | "yaml" | "yml" | "tweak" | "lua" | "xl" => Want::Text,
        "xml" if low.ends_with("fomod/moduleconfig.xml") => Want::Text,
        "archive" => Want::Index,
        "dll" | "asi" => Want::Native,
        "7z" | "zip" | "rar" => Want::Nested,
        _ => return None,
    })
}

/// A reader that counts into the progress bar and stops when Cancel is pressed.
struct Count<'a> { r: &'a mut dyn Read, pr: &'a Progress }
impl Read for Count<'_> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if self.pr.stop.load(Relaxed) { return Err(io::Error::other(CANCELLED)) }
        let n = self.r.read(b)?;
        self.pr.done.fetch_add(n as u64, Relaxed);
        Ok(n)
    }
}

struct Keep<'a> { c: &'a mut Candidate, kept: u64 }
impl Keep<'_> {
    /// Keeps what the checks need of one file and drops the rest.
    fn take(&mut self, name: &str, size: u64, mut r: &mut dyn Read) -> io::Result<()> {
        let name = name.replace('\\', "/").trim_start_matches('/').to_string();
        let got = match want(&name.to_ascii_lowercase()) {
            Some(Want::Text) if size <= MAX_TEXT && self.kept + size <= MAX_KEPT => {
                let mut b = Vec::new();
                r.take(MAX_TEXT).read_to_end(&mut b)?;
                self.kept += b.len() as u64;
                Some(Got::Text(b))
            }
            Some(Want::Index) => net::index(&mut r, |r, n| io::copy(&mut r.take(n), &mut io::sink())).map(|(h, f)| Got::Index(h, f)),
            Some(Want::Native) => Some(Got::Native),
            Some(Want::Nested) => Some(Got::Nested),
            Some(Want::Text) => None,
            None => return Ok(()),
        };
        match got { Some(g) => self.c.files.push((name, g)), None => self.c.skipped.push(name) }
        Ok(())
    }

    fn sevenz(&mut self, p: &Path, pr: &Progress) -> Result<(), String> {
        use sevenz_rust2::{ArchiveReader, EntryDecision, Password};
        let mut z = ArchiveReader::new(fs::File::open(p).map_err(|e| e.to_string())?, Password::empty()).map_err(|e| e.to_string())?;
        let files = &z.archive().files;
        limits(files.len(), files.iter().map(sevenz_rust2::ArchiveEntry::size).sum(), pr)?;
        let mut failed = None;
        z.for_each_entries_with_decision(|e, r| {
            if e.is_directory() { return Ok(EntryDecision::Continue) }
            let mut r = Count { r, pr };
            match self.take(e.name(), e.size(), &mut r).and_then(|_| io::copy(&mut r, &mut io::sink())) {
                Ok(_) => Ok(EntryDecision::Continue),
                Err(x) => { failed = Some(x.to_string()); Ok(EntryDecision::Stop) }
            }
        }).map_err(|e| e.to_string())?;
        failed.map_or(Ok(()), Err)
    }

    fn zip(&mut self, p: &Path, pr: &Progress) -> Result<(), String> {
        let mut z = zip::ZipArchive::new(fs::File::open(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let pick: Vec<usize> = (0..z.len()).filter(|&i| z.name_for_index(i).and_then(Result::ok).is_some_and(|n| want(&n.to_ascii_lowercase()).is_some())).collect();
        limits(z.len(), pick.iter().filter_map(|&i| z.by_index_raw(i).ok().map(|f| f.size())).sum(), pr)?;
        for i in pick {
            let mut f = z.by_index(i).map_err(|e| e.to_string())?;
            if f.is_dir() { continue }
            let (n, s) = (f.name().map_err(|e| e.to_string())?.into_owned(), f.size());
            self.take(&n, s, &mut Count { r: &mut f, pr }).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn folder(&mut self, p: &Path, pr: &Progress) -> Result<(), String> {
        let mut all = vec![];
        walk(p, 0, &mut |f| if all.len() <= MAX_FILES { all.push(f) });     // never follows links
        all.sort();                                                         // the same order on every OS
        let pick: Vec<(String, std::path::PathBuf, u64)> = all.iter().filter_map(|f| {
            let rel = f.strip_prefix(p).ok()?.to_string_lossy().replace('\\', "/");
            want(&rel.to_ascii_lowercase()).map(|_| (rel, f.clone(), f.metadata().map_or(0, |m| m.len())))
        }).collect();
        limits(all.len(), pick.iter().map(|x| x.2).sum(), pr)?;
        for (rel, f, size) in pick {
            if rel.to_ascii_lowercase().ends_with(".archive") {      // on disk the index is a seek away
                pr.done.fetch_add(size, Relaxed);
                match net::index_file(&f) { Some((h, fp)) => self.c.files.push((rel, Got::Index(h, fp))), None => self.c.skipped.push(rel) }
                continue
            }
            let mut file = fs::File::open(&f).map_err(|e| e.to_string())?;
            self.take(&rel, size, &mut Count { r: &mut file, pr }).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn limits(files: usize, size: u64, pr: &Progress) -> Result<(), String> {
    if files > MAX_FILES { return Err(format!("It holds more than {MAX_FILES} files, more than a mod should, so it wasn't read.")) }
    if size > MAX_SIZE { return Err(format!("It unpacks to over {} GB, more than this check reads.", MAX_SIZE >> 30)) }
    pr.total.store(size, Relaxed);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// FOMOD: the installer many mods ship (fomod/ModuleConfig.xml). Its options are shown with the installer's defaults
// ticked, and only the files the ticked options install are checked.

/// A FOMOD installer: where its paths start inside the archive, files it always installs, steps of option groups, and
/// files installed when flags (set by ticked options) or installed files match.
#[derive(Debug, Clone, Default)]
pub struct Fomod { pub base: String, pub required: Vec<Copy>, pub steps: Vec<Step>, pub conditional: Vec<(Cond, Vec<Copy>)> }
/// A file or folder the installer copies: its source inside the archive and where it goes in the game folder.
#[derive(Debug, Clone)]
pub struct Copy { pub src: String, pub dst: String, pub folder: bool }
#[derive(Debug, Clone)]
pub struct Step { pub name: String, pub visible: Option<Cond>, pub groups: Vec<Group> }
#[derive(Debug, Clone)]
pub struct Group { pub name: String, pub kind: Kind, pub options: Vec<Opt> }
/// How many of a group's options can be ticked: exactly one, at most one, at least one, all, or any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { One, AtMostOne, AtLeastOne, All, Any }
#[derive(Debug, Clone)]
pub struct Opt { pub name: String, pub about: String, pub files: Vec<Copy>, pub flags: Vec<(String, String)>, pub dflt: Ty, pub rules: Vec<(Cond, Ty)>, pub ty: Ty, pub on: bool }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty { Required, Recommended, Optional, NotUsable, CouldBeUsable }
/// Flags and installed files an installer tests, all of them or any one, with nested tests.
#[derive(Debug, Clone, Default)]
pub struct Cond { pub any: bool, pub flags: Vec<(String, String)>, pub files: Vec<(String, String)>, pub nested: Vec<Cond> }

#[derive(Debug, Default)]
struct Node { tag: String, attrs: Vec<(String, String)>, kids: Vec<Node>, text: String }
impl Node {
    fn kid(&self, t: &str) -> Option<&Node> { self.kids.iter().find(|k| k.tag == t) }
    fn all<'a>(&'a self, t: &'a str) -> impl Iterator<Item = &'a Node> { self.kids.iter().filter(move |k| k.tag == t) }
    fn at(&self, a: &str) -> String { self.attrs.iter().find(|x| x.0 == a).map(|x| x.1.clone()).unwrap_or_default() }
}

/// The XML as a tree (tags and attribute names lowercase), or None when it's broken. Capped against huge files.
fn dom(xml: &str) -> Option<Node> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml);
    let mut stack = vec![Node::default()];
    let node = |s: &quick_xml::events::BytesStart| Node { tag: s.local_name().as_ref().to_ascii_lowercase(),
        attrs: s.attributes().flatten().map(|a| (a.key.local_name().as_ref().to_ascii_lowercase(), a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(std::borrow::Cow::into_owned).unwrap_or_default())).collect(), ..Default::default() };
    for _ in 0..1_000_000 {
        match r.read_event().ok()? {
            Event::Start(s) => { if stack.len() > 64 { return None } stack.push(node(&s)) }
            Event::Empty(s) => stack.last_mut()?.kids.push(node(&s)),
            Event::End(_) => { let x = stack.pop()?; stack.last_mut()?.kids.push(x) }
            Event::Text(t) => stack.last_mut()?.text += &t.xml10_content(),
            Event::CData(t) => stack.last_mut()?.text += &t,
            Event::GeneralRef(g) => stack.last_mut()?.text += &g.resolve_char_ref().ok().flatten().map(String::from)
                .or_else(|| quick_xml::escape::unescape(&format!("&{};", g.xml10_content())).ok().map(std::borrow::Cow::into_owned)).unwrap_or_default(),
            Event::Eof => return (stack.len() == 1).then(|| stack.pop()).flatten(),
            _ => {}
        }
    }
    None
}

/// Text from bytes that may be UTF-16 (as installers often are) or UTF-8.
fn utf(b: &[u8]) -> String {
    let wide = |be: bool| char::decode_utf16(b[2..].chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }))
        .map(|c| c.unwrap_or('\u{fffd}')).collect();
    match b { [0xFF, 0xFE, ..] => wide(false), [0xFE, 0xFF, ..] => wide(true), _ => String::from_utf8_lossy(b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b)).into_owned() }
}

fn slash(p: &str) -> String { p.replace('\\', "/").trim_matches('/').to_string() }

impl Fomod {
    /// Parses fomod/ModuleConfig.xml at `path` inside the archive. Paths in it start at the folder holding fomod/.
    pub fn parse(path: &str, b: &[u8]) -> Option<Self> {
        let root = dom(&utf(b))?;
        let cfg = root.kid("config")?;
        let base = slash(&path[..path.to_ascii_lowercase().rfind("fomod/")?]);
        let copies = |n: Option<&Node>| -> Vec<Copy> { n.map(|n| n.kids.iter().filter(|k| k.tag == "file" || k.tag == "folder").map(|k| {
            let src = slash(&k.at("source"));
            Copy { dst: if k.attrs.iter().any(|a| a.0 == "destination") { slash(&k.at("destination")) } else { src.clone() }, src, folder: k.tag == "folder" }
        }).collect()).unwrap_or_default() };
        let ty = |n: Option<&Node>| match n.map(|n| n.at("name").to_ascii_lowercase()).as_deref() {
            Some("required") => Ty::Required, Some("recommended") => Ty::Recommended, Some("notusable") => Ty::NotUsable, Some("couldbeusable") => Ty::CouldBeUsable, _ => Ty::Optional };
        let steps = cfg.kid("installsteps").map(|s| s.all("installstep").map(|st| Step {
            name: st.at("name"),
            visible: st.kid("visible").map(|v| v.kid("dependencies").map_or_else(|| cond(v), cond)),
            groups: st.kid("optionalfilegroups").map(|g| g.all("group").map(|g| Group {
                name: g.at("name"),
                kind: match g.at("type").to_ascii_lowercase().as_str() { "selectexactlyone" => Kind::One, "selectatmostone" => Kind::AtMostOne,
                    "selectatleastone" => Kind::AtLeastOne, "selectall" => Kind::All, _ => Kind::Any },
                options: g.kid("plugins").map(|p| p.all("plugin").map(|p| {
                    let td = p.kid("typedescriptor");
                    let dep = td.and_then(|t| t.kid("dependencytype"));
                    let dflt = td.and_then(|t| t.kid("type")).map_or_else(|| ty(dep.and_then(|d| d.kid("defaulttype"))), |t| ty(Some(t)));
                    let rules = dep.and_then(|d| d.kid("patterns")).map(|ps| ps.all("pattern").map(|pt| (pt.kid("dependencies").map(cond).unwrap_or_default(), ty(pt.kid("type")))).collect()).unwrap_or_default();
                    Opt { name: p.at("name"), about: p.kid("description").map(|d| d.text.trim().to_string()).unwrap_or_default(), files: copies(p.kid("files")),
                          flags: p.kid("conditionflags").map(|f| f.all("flag").map(|f| (f.at("name"), f.text.trim().to_string())).collect()).unwrap_or_default(),
                          dflt, rules, ty: dflt, on: false }
                }).collect()).unwrap_or_default(),
            }).collect()).unwrap_or_default(),
        }).collect()).unwrap_or_default();
        let conditional = cfg.kid("conditionalfileinstalls").and_then(|c| c.kid("patterns")).map(|ps| ps.all("pattern")
            .map(|p| (p.kid("dependencies").map(cond).unwrap_or_default(), copies(p.kid("files")))).collect()).unwrap_or_default();
        Some(Self { base, required: copies(cfg.kid("requiredinstallfiles")), steps, conditional })
    }

    /// Ticks the installer's defaults: required and recommended options, and the first usable one where a group needs one.
    /// `has` says whether a file is in the game folder, for options whose type depends on it.
    pub fn defaults(&mut self, has: &dyn Fn(&str) -> bool) {
        let mut flags = BTreeMap::new();
        for s in &mut self.steps {
            if s.visible.as_ref().is_some_and(|v| !holds(v, &flags, has)) { continue }
            for g in &mut s.groups {
                for o in &mut g.options {
                    o.ty = o.rules.iter().find(|(c, _)| holds(c, &flags, has)).map_or(o.dflt, |r| r.1);
                    o.on = o.ty != Ty::NotUsable && (g.kind == Kind::All || matches!(o.ty, Ty::Required | Ty::Recommended));
                }
                let first = |pick: &dyn Fn(&Opt) -> bool| g.options.iter().position(|o| o.on).or_else(|| g.options.iter().position(pick));
                let one = match g.kind { Kind::One => first(&|o| o.ty != Ty::NotUsable), Kind::AtMostOne => first(&|_| false), _ => None };
                if matches!(g.kind, Kind::One | Kind::AtMostOne) { for (i, o) in g.options.iter_mut().enumerate() { o.on = Some(i) == one } }
                if g.kind == Kind::AtLeastOne && !g.options.iter().any(|o| o.on) && let Some(o) = g.options.iter_mut().find(|o| o.ty != Ty::NotUsable) { o.on = true }
                for o in g.options.iter().filter(|o| o.on) { flags.extend(o.flags.iter().cloned()) }
            }
        }
    }

    /// Ticks or unticks an option, keeping its group's rule. Returns whether anything changed.
    pub fn set(&mut self, s: usize, g: usize, o: usize, on: bool) -> bool {
        let g = &mut self.steps[s].groups[g];
        let t = g.options[o].ty;
        if g.kind == Kind::All || (t == Ty::Required && !on) || (t == Ty::NotUsable && on) || (g.kind == Kind::One && !on) || g.options[o].on == on { return false }
        if on && matches!(g.kind, Kind::One | Kind::AtMostOne) { for x in &mut g.options { x.on = false } }
        if !on && g.kind == Kind::AtLeastOne && g.options.iter().filter(|x| x.on).count() == 1 { return false }
        g.options[o].on = on;
        true
    }

    /// The files the ticked options install, with the flags they set and the conditional installs that match.
    fn copies(&self, has: &dyn Fn(&str) -> bool) -> Vec<Copy> {
        let (mut v, mut flags) = (self.required.clone(), BTreeMap::new());
        for s in &self.steps {
            if s.visible.as_ref().is_some_and(|c| !holds(c, &flags, has)) { continue }
            for o in s.groups.iter().flat_map(|g| &g.options).filter(|o| o.on) { v.extend(o.files.iter().cloned()); flags.extend(o.flags.iter().cloned()) }
        }
        v.extend(self.conditional.iter().filter(|(c, _)| holds(c, &flags, has)).flat_map(|(_, f)| f.iter().cloned()));
        v
    }
}

fn cond(n: &Node) -> Cond {
    Cond { any: n.at("operator").eq_ignore_ascii_case("or"), flags: n.all("flagdependency").map(|k| (k.at("flag"), k.at("value"))).collect(),
           files: n.all("filedependency").map(|k| (slash(&k.at("file")), k.at("state"))).collect(), nested: n.all("dependencies").map(cond).collect() }
}

/// Does a test hold? A flag never set counts as empty; a file is there unless the test asks for it to be missing.
fn holds(c: &Cond, flags: &BTreeMap<String, String>, has: &dyn Fn(&str) -> bool) -> bool {
    let mut t = c.flags.iter().map(|(f, v)| flags.get(f).map_or(v.is_empty(), |x| x.eq_ignore_ascii_case(v)))
        .chain(c.files.iter().map(|(f, s)| has(f) != s.eq_ignore_ascii_case("missing")))
        .chain(c.nested.iter().map(|n| holds(n, flags, has)));
    if c.any { t.any(|x| x) } else { t.all(|x| x) }
}

// ---------------------------------------------------------------------------------------------------------------

/// The game's own top folders, which tell where a mod's files start inside its archive.
const TOP: [&str; 6] = ["archive", "bin", "r6", "red4ext", "engine", "mods"];

/// Where each of the Candidate's files installs, from the game folder: by its FOMOD choices, else by finding the
/// game's folders inside it, with loose .archive and .xl files going to archive/pc/mod. Also the files that couldn't
/// be placed, and the alternative folders an archive without an installer holds.
pub fn place<'a>(c: &'a Candidate, has: &dyn Fn(&str) -> bool) -> (Vec<(String, &'a Got)>, Vec<String>, Vec<String>) {
    let (mut out, mut lost, mut roots): (Vec<(String, &Got)>, Vec<String>, BTreeSet<String>) = (vec![], vec![], BTreeSet::new());
    let mut put = |dst: String, g| if !out.iter().any(|(d, _): &(String, &Got)| d.eq_ignore_ascii_case(&dst)) { out.push((dst, g)) };
    if let Some(f) = &c.fomod {
        let base = f.base.to_ascii_lowercase();                     // ASCII only: the same byte offsets as the path
        let lows: Vec<String> = c.files.iter().map(|f| f.0.to_ascii_lowercase()).collect();
        for cp in f.copies(has) {
            let src = [base.as_str(), &cp.src.to_ascii_lowercase()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("/");
            for ((path, g), low) in c.files.iter().zip(&lows) {
                let rest = if src.is_empty() { Some(0) } else { low.strip_prefix(&src).filter(|r| r.is_empty() || r.starts_with('/')).map(|r| path.len() - r.len()) };
                match rest {
                    Some(i) if cp.folder => put([cp.dst.as_str(), path[i..].trim_start_matches('/')].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("/"), g),
                    Some(i) if i == path.len() => put(cp.dst.clone(), g),
                    _ => {}
                }
            }
        }
        return (out, lost, vec![])
    }
    for (path, g) in &c.files {
        let segs: Vec<&str> = path.split('/').collect();
        if let Some(i) = segs.iter().position(|s| TOP.contains(&s.to_ascii_lowercase().as_str())) {
            roots.insert(segs[..i].join("/"));
            put(segs[i..].join("/"), g);
        } else if wild("*.archive", path) || wild("*.xl", path) { put(format!("archive/pc/mod/{}", segs[segs.len() - 1]), g) }
        else if !matches!(g, Got::Nested) { lost.push(path.clone()) }
    }
    (out, lost, if roots.len() > 1 { roots.into_iter().map(|r| if r.is_empty() { "the top folder".into() } else { r }).collect() } else { vec![] })
}

/// A group's rule in words, for the option list.
pub fn rule(k: Kind) -> &'static str {
    match k { Kind::One => "pick one", Kind::AtMostOne => "at most one", Kind::AtLeastOne => "at least one", Kind::All => "all of them", Kind::Any => "any" }
}

/// Can the player change this option? The installer fixes required, unusable and select-all ones.
pub fn changeable(k: Kind, o: &Opt) -> bool { k != Kind::All && o.ty != Ty::Required && o.ty != Ty::NotUsable }

/// A **Pre-install check**'s result: the Candidate's reach in words, what will break (each naming the Candidate's file,
/// and its line where there is one), notes, its place in the Net, and the **Mod entries** it would install.
#[derive(Debug, Clone, Default)]
pub struct Pre { pub reach: String, pub breaks: Vec<String>, pub notes: Vec<String>, pub net: Vec<Row>, pub entries: Vec<String> }

impl Pre {
    /// "Will break: 2 · 3 notes", or "No certain breaks found · 1 note". Never a safe or green verdict.
    pub fn summary(&self) -> String {
        let b = if self.breaks.is_empty() { "No certain breaks found".to_string() } else { format!("Will break: {}", self.breaks.len()) };
        format!("{b} · {}", count(self.notes.len(), "note"))
    }
}

/// Checks a Candidate against the installed mods, with its FOMOD choices as they stand.
pub fn precheck(inst: &Installed, nl: &net::List, c: &Candidate) -> Pre {
    let has_file = |f: &str| crate::check::resolve(&inst.game, f).is_some();
    let (placed, lost, alts) = place(c, &has_file);
    let mut m = Mod { name: c.name.clone(), root: usize::MAX, ..Default::default() };
    for (dst, g) in &placed {
        if let Some(u) = unit(dst) && !m.entries.iter().any(|e| e.eq_ignore_ascii_case(&u)) { m.entries.push(u) }
        match g {
            Got::Text(b) => m.facts.add(dst, b),
            Got::Index(hs, fp) => m.facts.archive(dst, hs, *fp),
            Got::Native => m.facts.add(dst, &[]),
            Got::Nested => {}
        }
    }
    m.reach = nl.reach_of(&m.entries);
    // installed mods with the same entries are older copies it replaces: it's checked in their place
    let low: HashSet<String> = m.entries.iter().map(|e| e.to_lowercase()).collect();
    let all = inst.read.get_or_init(|| { let mut v = inst.mods.clone(); for m in &mut v { m.facts.read_archives() } v });
    let (old, keep): (Vec<&Mod>, Vec<&Mod>) = all.iter().partition(|x| x.entries.iter().any(|e| low.contains(&e.to_lowercase())));
    let (mut mods, ci) = (vec![m], 0);      // the Candidate goes first, so its own file leads every line about a pair
    mods.extend(keep.into_iter().cloned());
    let n = net::net(&mut mods, nl, &|f| inst.has.contains(f), &inst.fw, inst.mac, Some(ci));
    let cm = &mods[ci];
    let mut p = Pre { entries: cm.entries.clone(), net: n.cores.iter().filter(|r| r.mods.contains(&ci)).cloned().collect(), ..Default::default() };
    p.reach = format!("{} It was checked against {} you have installed.", match cm.reach() {
        Reach::Engine => "Engine-reaching: it changes game code.",
        Reach::Contained => "Contained: it doesn't change game code, so its needs and the game files it would replace were checked.",
    }, count(mods.len() - 1, "mod"));
    // will break: imports nothing provides, frameworks that aren't installed, and the Net's certain breaks
    let decl: HashSet<&str> = mods.iter().flat_map(|x| x.facts.modules.iter().map(String::as_str)).chain(inst.fw.iter().map(String::as_str)).collect();
    let mut seen = BTreeSet::new();
    for (i, at) in &cm.facts.imports {
        let Some(u) = crate::check::unmet(&decl, i).filter(|u| seen.insert(u.clone())) else { continue };
        p.breaks.push(match net::marker(&u) {
            Some((core, v)) => match decl.iter().filter_map(|d| net::marker(d).filter(|x| x.0 == core).map(|x| x.1)).max_by(|a, b| cmp_ver(a, b)) {
                Some(h) => format!("It needs {core} {v} or newer ({at}), and yours goes up to {h}: update {core} first."),
                None => format!("It needs {core} {v} or newer ({at}), which isn't installed: install it first."),
            },
            None => format!("It builds on {u} ({at}), which no installed mod provides: install the mod that does first."),
        });
    }
    let uses = if inst.mac { None } else { Some(&cm.facts.uses) };      // on a Mac the Net says what can't work there
    for f in uses.into_iter().flatten().filter(|f| !inst.has.contains(**f)) {
        let file = placed.iter().find(|(d, _)| net::needs(d) == Some(*f)).map_or(String::new(), |x| format!(" ({})", x.0));
        p.breaks.push(format!("It needs {f}{file}, which isn't installed: install {f} first."));
    }
    for b in n.breaks.iter().filter(|b| b.at == ci && !cm.facts.uses.iter().any(|f| b.why == format!("It needs {f}, which isn't installed."))) {
        p.breaks.push(b.why.replace(", or remove this one too.", " first."));
    }
    // notes
    if !old.is_empty() { p.notes.push(format!("It replaces your installed {}, so it was checked in place of {}.", list(&old.iter().map(|m| m.name.clone()).collect::<Vec<_>>()), if old.len() == 1 { "it" } else { "them" })) }
    for r in &n.overlaps { if let Some(&o) = r.mods.iter().find(|&&i| i != ci) { p.notes.extend(r.lines.iter().map(|l| format!("With {}: {l}", mods[o].name))) } }
    for r in n.details.iter().filter(|r| r.mods.contains(&ci)) { p.notes.extend(r.lines.iter().cloned()) }
    let ours = inst.game_files.get_or_init(|| net::game_files(&inst.game));
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for (h, a) in &cm.facts.files { if ours.contains(h) { *by.entry(a.as_str()).or_default() += 1 } }
    p.notes.extend(by.iter().map(|(a, k)| format!("{} replaces {k} of the game's own files.", a.rsplit('/').next().unwrap_or(a))));
    for (f, paths) in &inst.frameworks {
        let units: Vec<String> = paths.iter().filter_map(|p| unit(p)).collect();
        let is_own = |d: &&String| paths.iter().any(|p| p.eq_ignore_ascii_case(d));
        let own: Vec<String> = placed.iter().map(|x| &x.0).filter(is_own).cloned().collect();
        let into: Vec<String> = placed.iter().map(|x| &x.0).filter(|d| !is_own(d) && unit(d).is_some_and(|u| units.iter().any(|q| q.eq_ignore_ascii_case(&u)))).cloned().collect();
        if !own.is_empty() { p.notes.push(format!("It ships its own copy of {f} ({}), which replaces the one in your game folder.", list(&own))) }
        if !into.is_empty() { p.notes.push(format!("It adds {} to {f}'s folder; {} to this mod, not {f}.", list(&into), if into.len() == 1 { "that file belongs" } else { "those files belong" })) }
    }
    let native: Vec<String> = placed.iter().filter(|(_, g)| matches!(g, Got::Native)).map(|x| x.0.clone()).collect();
    if !native.is_empty() { p.notes.push(format!("It runs native code ({}), which can only be judged once the game loads it.", list(&native))) }
    let nested: Vec<String> = c.files.iter().filter(|(_, g)| matches!(g, Got::Nested)).map(|x| x.0.clone()).collect();
    if !nested.is_empty() { p.notes.push(format!("It holds other archives ({}), which weren't opened: check each on its own.", list(&nested))) }
    if !c.skipped.is_empty() { p.notes.push(format!("{} couldn't be read (too large, or not what its name says), so {} checked.", list(&c.skipped), if c.skipped.len() == 1 { "it wasn't" } else { "they weren't" })) }
    let one = lost.len() == 1;
    if !lost.is_empty() { p.notes.push(format!("{} {} in the game's folders, so where {} install couldn't be told: {}.", count(lost.len(), "file"), if one { "isn't" } else { "aren't" }, if one { "it'd" } else { "they'd" }, list(&lost.iter().take(3).cloned().collect::<Vec<_>>()))) }
    if !alts.is_empty() { p.notes.push(format!("It holds {} alternatives without an installer ({}): they were checked together, so install only one.", alts.len(), list(&alts))) }
    if placed.is_empty() { p.notes.push("Nothing in it installs into the game's folders, so there was nothing to check.".into()) }
    p
}

/// Plain text for `--precheck` and the tests.
pub fn text(c: &Candidate, p: &Pre) -> String {
    let mut s = format!("{}{}\n{}\n{}\n", c.name, c.version.as_deref().map_or(String::new(), |v| format!(" {v}")), p.summary(), p.reach);
    for (head, lines) in [("will break", &p.breaks), ("note", &p.notes)] { for l in lines { s += &format!("\t{head}: {l}\n") } }
    for r in &p.net { s += &format!("\tnet: {}\n", r.title); for l in &r.lines { s += &format!("\t\t{l}\n") } }
    if let Some(f) = &c.fomod {
        for st in &f.steps { for g in &st.groups { for o in &g.options {
            s += &format!("\toption: [{}] {} / {} ({})\n", if o.on { "x" } else { " " }, st.name, o.name, rule(g.kind));
        } } }
    }
    if !p.entries.is_empty() { s += &format!("\tinstalls: {}\n", p.entries.join(", ")) }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nexus_names() {
        let c = Candidate::named("BT 7274-35041-1-3-0-1732500000");
        assert_eq!((c.name.as_str(), c.version.as_deref(), c.nexus_id), ("BT 7274", Some("1.3.0"), Some(35041)));
        assert_eq!(Candidate::named("My-Mod v2").name, "My-Mod v2");
    }

    #[test]
    fn fomod_defaults_and_placing() {
        let xml = r#"<?xml version="1.0" encoding="utf-16"?>
<config xmlns:xsi="x"><moduleName>Demo</moduleName>
  <requiredInstallFiles><folder source="core" destination="" /></requiredInstallFiles>
  <installSteps order="Explicit"><installStep name="Body">
    <optionalFileGroups order="Explicit">
      <group name="Pick one" type="SelectExactlyOne"><plugins order="Explicit">
        <plugin name="A"><description>Plain &amp; simple</description><files><file source="opt\a.archive" destination="archive\pc\mod\a.archive"/></files>
          <conditionFlags><flag name="body">A</flag></conditionFlags><typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
        <plugin name="B"><files><folder source="opt\b" destination="r6\scripts\B"/></files>
          <typeDescriptor><type name="Recommended"/></typeDescriptor></plugin>
      </plugins></group>
      <group name="Extras" type="SelectAny"><plugins order="Explicit">
        <plugin name="Needs X"><files><file source="opt\x.xl"/></files>
          <typeDescriptor><dependencyType><defaultType name="Optional"/><patterns><pattern><dependencies operator="And">
            <fileDependency file="red4ext\plugins\ArchiveXL\ArchiveXL.dll" state="Active"/></dependencies><type name="Recommended"/></pattern></patterns></dependencyType></typeDescriptor></plugin>
      </plugins></group>
    </optionalFileGroups></installStep></installSteps>
  <conditionalFileInstalls><patterns><pattern><dependencies><flagDependency flag="body" value="A"/></dependencies>
    <files><file source="patch\a_patch.archive" destination="archive\pc\mod\a_patch.archive"/></files></pattern></patterns></conditionalFileInstalls>
</config>"#;
        let mut b = vec![0xFF, 0xFE];
        b.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        let mut f = Fomod::parse("Demo 1.0/fomod/ModuleConfig.xml", &b).unwrap();
        assert_eq!(f.base, "Demo 1.0");
        assert_eq!(f.steps[0].groups[0].options[0].about, "Plain & simple");
        f.defaults(&|p| p == "red4ext/plugins/ArchiveXL/ArchiveXL.dll");
        let on = |f: &Fomod| f.steps[0].groups.iter().flat_map(|g| &g.options).filter(|o| o.on).map(|o| o.name.clone()).collect::<Vec<_>>();
        assert_eq!(on(&f), ["B", "Needs X"], "recommended ones, and one whose type depends on an installed file");
        let idx = |p: &str| Got::Index(vec![crate::check::hash(p)], 1);
        let c = Candidate { name: "Demo".into(), fomod: Some(f), files: vec![
            ("Demo 1.0/core/r6/tweaks/Demo/t.yaml".into(), Got::Text(b"Items.X.price: 1\n".to_vec())), ("Demo 1.0/opt/a.archive".into(), idx("a")),
            ("Demo 1.0/opt/b/b.reds".into(), Got::Text(b"module B\n".to_vec())), ("Demo 1.0/opt/x.xl".into(), Got::Text(vec![])),
            ("Demo 1.0/patch/a_patch.archive".into(), idx("p")), ("Demo 1.0/fomod/ModuleConfig.xml".into(), Got::Text(b.clone()))], ..Default::default() };
        let dst = |c: &Candidate| place(c, &|_| false).0.into_iter().map(|x| x.0).collect::<Vec<_>>();
        assert_eq!(dst(&c), ["r6/tweaks/Demo/t.yaml", "r6/scripts/B/b.reds", "opt/x.xl"]);
        let mut c2 = c.clone();
        let fm = c2.fomod.as_mut().unwrap();
        assert!(fm.set(0, 0, 0, true) && !fm.set(0, 0, 0, false), "exactly one: tick another instead");
        assert_eq!(dst(&c2), ["r6/tweaks/Demo/t.yaml", "archive/pc/mod/a.archive", "opt/x.xl", "archive/pc/mod/a_patch.archive"], "the flag A sets installs the patch");
    }

    #[test]
    fn layout_without_an_installer() {
        let t = |p: &str| (p.to_string(), Got::Text(vec![]));
        let c = Candidate { files: vec![t("Mod/r6/scripts/M/a.reds"), t("Mod/archive/pc/mod/m.xl"), t("loose.xl"), t("readme/notes.lua")], ..Default::default() };
        let (placed, lost, alts) = place(&c, &|_| false);
        assert_eq!(placed.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["r6/scripts/M/a.reds", "archive/pc/mod/m.xl", "archive/pc/mod/loose.xl"]);
        assert_eq!((lost, alts), (vec!["readme/notes.lua".to_string()], vec![]));
        let c = Candidate { files: vec![t("Option A/archive/pc/mod/a.xl"), t("Option B/archive/pc/mod/b.xl")], ..Default::default() };
        assert_eq!(place(&c, &|_| false).2, ["Option A", "Option B"]);
    }

    #[test]
    fn checks_a_candidate_against_the_install() {
        let s = |code: &str| crate::check::reds(code);
        let mk = |name: &str, f: &dyn Fn(&mut net::Facts)| { let mut m = Mod { name: name.into(), entries: vec![format!("r6/scripts/{name}")], ..Default::default() }; f(&mut m.facts); m };
        let inst = Installed { mods: vec![
            mk("Core", &|f| { f.script("r6/scripts/Core/a.reds", &s("module Core\npublic class Hub {}\n")); f.script("r6/scripts/Core/m.reds", &s("module Core.Api1_2_0\n")) }),
            mk("Sorter", &|f| f.script("r6/scripts/Sorter/s.reds", &s("@replaceMethod(Builder)\npublic func Sort() {}\n"))),
            mk("Old", &|f| f.script("r6/scripts/Old/o.reds", &s("@wrapMethod(Other)\nfunc Y() {}\n"))),
        ], has: ["redscript".to_string()].into(), game: std::env::temp_dir().join("ccc-no-game"), ..Default::default() };
        let c = Candidate { name: "New".into(), files: vec![
            ("New/r6/scripts/New/n.reds".into(), Got::Text(b"import Core.Api1_3_0.*\nimport Gone.*\n@replaceMethod(Builder)\npublic func Sort() {}\n".to_vec())),
            ("New/archive/pc/mod/new.archive.xl".into(), Got::Text(vec![])),
            ("New/red4ext/plugins/New/New.dll".into(), Got::Native)], ..Default::default() };
        let p = precheck(&inst, &net::List::parse("schema = 1\nupdated = \"x\"").unwrap(), &c);
        assert_eq!(p.breaks, [
            "It needs Core 1.3.0 or newer (r6/scripts/New/n.reds line 1), and yours goes up to 1.2.0: update Core first.",
            "It builds on Gone (r6/scripts/New/n.reds line 2), which no installed mod provides: install the mod that does first.",
            "It needs ArchiveXL (archive/pc/mod/new.archive.xl), which isn't installed: install ArchiveXL first.",
            "It needs RED4ext (red4ext/plugins/New/New.dll), which isn't installed: install RED4ext first."]);
        assert!(p.notes.iter().any(|n| n.starts_with("With Sorter: Both replace Builder.Sort (r6/scripts/New/n.reds line 3")), "{:?}", p.notes);
        assert!(p.notes.iter().any(|n| n.starts_with("It runs native code (red4ext/plugins/New/New.dll)")));
        assert!(p.reach.starts_with("Engine-reaching") && p.summary() == format!("Will break: 4 · {}", count(p.notes.len(), "note")));
        // a newer copy of an installed mod is checked in its place, not against it
        let c = Candidate { name: "Sorter 2".into(), files: vec![("r6/scripts/Sorter/s.reds".into(), Got::Text(b"@replaceMethod(Builder)\npublic func Sort() {}\n".to_vec()))], ..Default::default() };
        let p = precheck(&inst, &net::List::parse("schema = 1\nupdated = \"x\"").unwrap(), &c);
        assert_eq!((p.breaks.len(), p.notes[0].as_str()), (0, "It replaces your installed Sorter, so it was checked in place of it."));
        assert_eq!(p.summary(), "No certain breaks found · 1 note");
    }

    #[test]
    fn odd_paths_cancel_and_nested_archives() {
        // a path whose lowercase form has another byte length (the Kelvin sign) keeps its offsets
        assert_eq!(Fomod::parse("a\u{212A}/fomod/ModuleConfig.xml", b"<config/>").map(|f| f.base), Some("a\u{212A}".into()));
        let dir = std::env::temp_dir().join(format!("ccc-cancel-{}", std::process::id()));
        fs::create_dir_all(dir.join("archive/pc/mod")).unwrap();
        fs::write(dir.join("archive/pc/mod/x.archive"), b"not an archive").unwrap();
        fs::write(dir.join("Optional.zip"), b"").unwrap();
        // Cancel wins even when the reader got to the end
        let pr = Progress::default();
        pr.stop.store(true, Relaxed);
        assert_eq!(read(&dir, &pr).unwrap_err(), CANCELLED);
        // an archive inside it is named as one, not as a file that couldn't be placed
        let c = read(&dir, &Progress::default()).unwrap();
        let p = precheck(&Installed { game: dir.join("no-game"), ..Default::default() }, &net::List::EMPTY, &c);
        assert!(p.notes.iter().any(|n| n.starts_with("It holds other archives (Optional.zip)")) && !p.notes.iter().any(|n| n.contains("in the game's folders")), "{:?}", p.notes);
        fs::remove_dir_all(&dir).unwrap();
    }
}
