//! The compatibility manifest, the install scan and the verdicts.
//! Read-only: nothing here writes, moves or deletes a file.
use serde::Deserialize;
use std::{cmp::Ordering, collections::BTreeMap, fs, path::{Component, Path, PathBuf}};

pub const BUILT_IN: &str = include_str!("../compat.toml");

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub updated: String,
    #[serde(default)] pub build: Vec<Build>,
    #[serde(default)] pub framework: Vec<Entry>,
    #[serde(default, rename = "mod")] pub mods: Vec<Entry>,
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
    #[serde(default)] pub build_locked: bool,
    #[serde(default)] pub min: BTreeMap<String, String>,
    #[serde(default)] pub needs: Vec<String>,
    #[serde(default)] pub notes: Vec<Note>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Note { pub build: String, pub text: String, pub source: String, pub date: String }

/// The mod's Nexus page. Built from the numeric id, so a manifest can never point a link anywhere else.
pub fn nexus_url(id: u32) -> String { format!("https://www.nexusmods.com/cyberpunk2077/mods/{id}") }

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let m: Self = toml::from_str(text).map_err(|e| e.message().to_string())?;
        if m.schema != 1 { return Err(format!("unsupported schema {}", m.schema)) }
        let mut known: Vec<&str> = vec![];
        for (i, e) in m.framework.iter().chain(&m.mods).enumerate() {
            let fw = i < m.framework.len();
            if e.nexus_id == 0 || e.detect.is_empty() { return Err(format!("{}: needs a nexus_id and a detect path", e.name)) }
            if let Some(p) = e.detect.iter().find(|p| Path::new(p).components().any(|c| !matches!(c, Component::Normal(_)))) {
                return Err(format!("{}: detect path {p:?} must stay inside the game folder", e.name))
            }
            if let Some(n) = e.needs.iter().find(|n| !known.contains(&n.as_str())) {
                return Err(format!("{}: needs {n:?}, which isn't a framework listed above it", e.name))
            }
            if fw { known.push(&e.name) }
        }
        Ok(m)
    }

    /// The built-in list, or a valid `compat.toml` in `dir` with a later `updated` date.
    /// Returns the list, where it came from, and a warning when a copy in `dir` was ignored.
    pub fn load(dir: Option<&Path>) -> (Self, String, Option<String>) {
        let built_in = Self::parse(BUILT_IN).expect("built-in compat.toml is valid");
        let Some(path) = dir.map(|d| d.join("compat.toml")).filter(|p| p.is_file()) else { return (built_in, "built in".into(), None) };
        match fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Self::parse(&t)) {
            Ok(m) if m.updated > built_in.updated => (m, "from the compat.toml next to the app".into(), None),
            Ok(_) => (built_in, "built in".into(), Some(format!("{} is older than the built-in list, so it was ignored.", path.display()))),
            Err(e) => (built_in, "built in".into(), Some(format!("{} couldn't be read ({e}), so the built-in list is used.", path.display()))),
        }
    }
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
    let b = fs::read(path).ok()?;
    let i = b.windows(8).position(|w| w == SIG)?;
    let d = |k: usize| b.get(i + 8 + 4 * k..i + 12 + 4 * k).map(|s| u64::from(u32::from_le_bytes(s.try_into().unwrap())));
    let four = |hi: u64, lo: u64| [hi >> 16, hi & 0xffff, lo >> 16, lo & 0xffff];
    Some((four(d(0)?, d(1)?), four(d(2)?, d(3)?)))
}

/// [2, 5, 0, 0] -> "2.5"; keeps at least `min` parts. Mod versions drop the fourth (build) number.
fn show(v: [u64; 4], min: usize) -> String {
    let n = v.iter().rposition(|&x| x != 0).map_or(0, |i| i + 1).max(min);
    v[..n].iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

/// `rel` under `root`, matching names without regard to case (mods are made on Windows; Linux folders are case-sensitive).
fn resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        let exact = p.join(part);
        p = if exact.exists() { exact } else {
            fs::read_dir(&p).ok()?.flatten().map(|e| e.path()).find(|e| e.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(part)))?
        };
    }
    Some(p)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Status { Red, Gray, Green }

pub struct Row { pub name: String, pub nexus_id: u32, pub version: Option<String>, pub status: Status, pub why: String, pub notes: Vec<Note> }

pub struct Report {
    pub build: Option<String>,      // e.g. "2.5"
    pub exe: Option<String>,        // the exe's file version, e.g. "3.0.88.46235"
    pub banner: Option<(bool, String)>,   // (is a warning, text); a draft list is only a notice
    pub rows: Vec<Row>,             // sorted: red, gray, green, then by name
    pub unlisted: Vec<String>,      // native plugins the list doesn't know
}

/// Checks the game folder, plus any Mod Organizer 2 mod folders (each laid over the game folder), against the manifest.
pub fn check(m: &Manifest, game: &Path, mo2: &[PathBuf]) -> Result<Report, String> {
    let exe = resolve(game, "bin/x64/Cyberpunk2077.exe")
        .ok_or_else(|| format!("Cyberpunk2077.exe wasn't found in {}. Choose the folder that holds bin, archive and r6.", game.display()))?;
    let vi = pe_version(&exe);
    let build = vi.map(|v| show(v.1, 2));
    let roots: Vec<&Path> = mo2.iter().map(PathBuf::as_path).chain([game]).collect();
    let find = |rel: &str| roots.iter().find_map(|r| resolve(r, rel));

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
    for (i, e) in m.framework.iter().chain(&m.mods).enumerate() {
        let fw = i < m.framework.len();
        let Some(_) = e.detect.iter().find_map(|d| find(d)) else { if fw { state.insert(&e.name, None); } continue };
        let version = e.detect.first().and_then(|d| find(d)).filter(|p| p.is_file()).and_then(|p| pe_version(&p)).map(|(f, _)| show([f[0], f[1], f[2], 0], 3));
        let need = |s: Option<Status>| e.needs.iter().find(|n| state.get(n.as_str()).copied().flatten() == s);
        let missing = e.needs.iter().find(|n| state.get(n.as_str()).is_some_and(Option::is_none));
        let notes: Vec<Note> = e.notes.iter().filter(|n| build.as_deref().is_some_and(|b| cmp_ver(&n.build, b).is_eq())).cloned().collect();
        let min = build.as_deref().and_then(|b| e.min.iter().find(|(k, _)| cmp_ver(k, b).is_eq())).map(|(_, v)| v);
        use Status::*;
        let (status, why) = if !notes.is_empty() { (Red, format!("Known problem on game {b}.")) }
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
        rows.push(Row { name: e.name.clone(), nexus_id: e.nexus_id, version, status, why, notes });
    }
    for f in &m.framework {                                  // missing frameworks matter only when something installed needs them
        let users: Vec<&str> = rows.iter().filter_map(|r| m.framework.iter().chain(&m.mods).find(|e| e.name == r.name))
            .filter(|e| e.needs.contains(&f.name)).map(|e| e.name.as_str()).collect();
        if state.get(f.name.as_str()).is_some_and(Option::is_none) && !users.is_empty() {
            rows.push(Row { name: f.name.clone(), nexus_id: f.nexus_id, version: None, status: Status::Red, notes: vec![],
                            why: format!("Not installed, but {} need{} it.", users.join(", "), if users.len() == 1 { "s" } else { "" }) });
        }
    }
    rows.sort_by_key(|r| (r.status, r.name.to_lowercase()));

    let listed = |p: &Path| m.framework.iter().chain(&m.mods).flat_map(|e| &e.detect).any(|d| {
        let d = d.replace('\\', "/").to_lowercase();
        roots.iter().any(|r| p.strip_prefix(r).is_ok_and(|rel| { let rel = rel.to_string_lossy().replace('\\', "/").to_lowercase(); d == rel || d.starts_with(&format!("{rel}/")) }))
    });
    let mut unlisted: Vec<String> = roots.iter().flat_map(|r| {
        let dirs = resolve(r, "red4ext/plugins").and_then(|p| fs::read_dir(p).ok()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir());
        let asi = resolve(r, "bin/x64/plugins").and_then(|p| fs::read_dir(p).ok()).into_iter().flatten().flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("asi")));
        dirs.chain(asi).collect::<Vec<_>>()
    }).filter(|p| !listed(p)).filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
    unlisted.sort_by_key(|n| n.to_lowercase());
    unlisted.dedup();
    Ok(Report { build, exe: vi.map(|v| show(v.0, 4)), banner, rows, unlisted })
}

/// Plain-text report, for `--report` and the tests.
pub fn text(r: &Report, source: &str, updated: &str) -> String {
    let mut s = format!("Game build {} (exe {}). Compatibility list {updated}, {source}.\n",
                        r.build.as_deref().unwrap_or("unknown"), r.exe.as_deref().unwrap_or("unknown"));
    if let Some((_, b)) = &r.banner { s += &format!("! {b}\n") }
    for x in &r.rows {
        s += &format!("{:?}\t{}\t{}\t{}\t{}\n", x.status, x.name, x.version.as_deref().unwrap_or("-"), x.why, nexus_url(x.nexus_id));
        for n in &x.notes { s += &format!("\tnote: {} ({}, {})\n", n.text, n.source, n.date) }
    }
    if !r.unlisted.is_empty() { s += &format!("Unlisted native plugins: {}\n", r.unlisted.join(", ")) }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn verdicts(game: [u16; 4], files: &[(&str, Option<[u16; 4]>)], name: &str) -> (Report, Vec<(String, Status)>) {
        let r = check(&Manifest::parse(LIST).unwrap(), &install(name, game, files), &[]).unwrap();
        let v = r.rows.iter().map(|x| (x.name.clone(), x.status)).collect();
        (r, v)
    }

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
        assert!(r.banner.is_none());
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
        let r = check(&Manifest::parse(LIST).unwrap(), &game, &[m]).unwrap();
        assert!(r.rows.iter().any(|x| x.name == "ArchiveXL" && x.status == Status::Green));
    }

    #[test]
    fn rejects_unsafe_or_broken_lists() {
        let bad = LIST.replace("r6/scripts/EquipmentEx", "../../outside");
        assert!(Manifest::parse(&bad).unwrap_err().contains("inside the game folder"));
        assert!(Manifest::parse(&LIST.replace(r#"needs = ["RED4ext"]"#, r#"needs = ["Nope"]"#)).is_err());
        assert!(Manifest::parse(&LIST.replace("schema = 1", "schema = 1\nurl = \"https://example.com\"")).is_err());
    }
}
