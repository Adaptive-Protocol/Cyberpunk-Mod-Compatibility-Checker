//! The only code that changes files, and it runs only after the user confirms in the app's window.
//! - Back up and remove moves a mod's entries into `_mod_backups` in the game folder, with their folder paths. That's
//!   the same drive, so each move is a rename, and if one fails the earlier ones are moved back.
//! - Delete does the same, then deletes the backup.
//! - Restore moves a backup back, but never over files that are there again.
//!
//! Only whole mod entries inside the game folder qualify (see check::unit). The game's own files never do, and
//! nothing is moved or deleted through a link or junction to another folder.
use crate::check::unit;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::{Path, PathBuf}, time::SystemTime};

pub const DIR: &str = "_mod_backups";
const LIST: &str = "backup.toml";

/// A backed-up mod: its name, why it was removed, the date, and the entries it held, relative to the game folder.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Backup { pub name: String, pub why: String, pub date: String, pub paths: Vec<String>, #[serde(skip)] pub dir: PathBuf }

/// "2026-11-04" for a number of days since 1970-01-01 (H. Hinnant's civil-from-days).
pub fn date(days: i64) -> String {
    let (era, doe) = ((days + 719_468).div_euclid(146_097), (days + 719_468).rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{:02}", yoe + era * 400 + i64::from(m <= 2), doy - (153 * mp + 2) / 5 + 1)
}

fn today() -> String { date(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| (d.as_secs() / 86_400) as i64)) }

/// A mod name fit for a folder name on every OS.
fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || " -_.()[]+&'".contains(c) { c } else { '_' }).take(60).collect();
    let s = s.trim().trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() { "mod".into() } else { s }
}

fn fail(e: &io::Error, p: &str) -> String {
    let hint = match e.kind() {
        io::ErrorKind::PermissionDenied => " Close the game first. If the game is under Program Files, Windows may also need the app run as administrator.",
        _ if e.raw_os_error() == Some(32) => " It's in use: close the game first.",
        _ => "",
    };
    format!("Couldn't move {p}: {e}.{hint}")
}

/// A whole mod entry, written with `/`, that can't point outside the folder it's joined to.
fn plain(p: &str) -> bool { p.split('/').all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains([':', '\\'])) && unit(p).as_deref() == Some(p) }

/// Refuses `p` when a folder on its way down from `root` is a link, a junction or a file: a move or delete through it
/// would happen somewhere else. A folder that isn't there yet is fine.
fn direct(root: &Path, p: &str) -> Result<(), String> {
    let up: Vec<&str> = p.split('/').collect();
    for i in 1..up.len() {
        match root.join(up[..i].join("/")).symlink_metadata() {
            Ok(m) if m.is_dir() => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            _ => return Err(format!("{} in {} isn't a plain folder (it may be a link or junction to elsewhere), so the app won't move or delete anything through it.",
                                    up[..i].join("/"), root.display())),
        }
    }
    Ok(())
}

/// `paths` under `root` after checking each is plain, is reached through plain folders, and exists there (or, with
/// `fresh`, doesn't).
fn checked(root: &Path, paths: &[String], fresh: bool) -> Result<Vec<PathBuf>, String> {
    paths.iter().map(|p| {
        if !plain(p) { return Err(format!("{p} isn't a mod folder or file this app may change.")) }
        direct(root, p)?;
        match (root.join(p).symlink_metadata(), fresh) {
            (Err(e), _) if e.kind() != io::ErrorKind::NotFound => Err(format!("Couldn't check {p}: {e}.")),   // not taken as absent
            (Err(_), false) => Err(format!("{p} wasn't found.")),
            (Ok(_), true) => Err(format!("{p} is installed again. If that's an update, delete this backup instead.")),
            _ => Ok(root.join(p)),
        }
    }).collect()
}

/// Moves each `from[i]` to `to[i]`. If one fails, moves the earlier ones back and says what couldn't go back.
fn shift(from: &[PathBuf], to: &[PathBuf], names: &[String]) -> Result<(), String> {
    for i in 0..from.len() {
        if let Err(e) = to[i].parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| fs::rename(&from[i], &to[i])) {
            let stuck: Vec<&str> = (0..i).rev().filter(|&j| fs::rename(&to[j], &from[j]).is_err()).map(|j| names[j].as_str()).collect();
            let back = if stuck.is_empty() { " Nothing was changed.".to_string() } else { format!(" These couldn't be moved back: {}.", stuck.join(", ")) };
            return Err(fail(&e, &names[i]) + &back);
        }
    }
    Ok(())
}

/// `<game>/_mod_backups`, unless it's a link or junction to somewhere else.
pub fn home(game: &Path) -> Result<PathBuf, String> {
    let d = game.join(DIR);
    match d.symlink_metadata() { Ok(m) if !m.is_dir() => Err(format!("{} isn't a plain folder, so the app won't use it.", d.display())), _ => Ok(d) }
}

/// Removes the empty folders under `dir`, then `dir` if only its list is left, then `_mod_backups` if that's empty.
fn prune(dir: &Path) {
    fn empty(d: &Path) {
        for e in fs::read_dir(d).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) { empty(&e.path()); let _ = fs::remove_dir(e.path()); }
        }
    }
    empty(dir);
    if fs::read_dir(dir).into_iter().flatten().flatten().all(|e| e.file_name() == LIST) {
        let _ = fs::remove_file(dir.join(LIST));
        let _ = fs::remove_dir(dir).and_then(|_| dir.parent().map_or(Ok(()), fs::remove_dir));
    }
}

/// Deletes a backup's entries (a link, not what it points to), then the backup folder. The list goes last, so a
/// backup that couldn't be fully deleted still shows in the Backups window.
fn erase(dir: &Path, paths: &[String]) -> Result<(), String> {
    paths.iter().try_for_each(|p| direct(dir, p))?;                                // all checked before anything goes
    for p in paths {
        let f = dir.join(p);
        let r = match f.symlink_metadata() { Ok(m) if m.is_dir() => fs::remove_dir_all(&f), Ok(_) => fs::remove_file(&f).or_else(|_| fs::remove_dir(&f)), Err(_) => Ok(()) };
        r.map_err(|e| format!("Couldn't delete {p}: {e}. It's still in {}.", dir.display()))?;
    }
    fs::remove_dir_all(dir).map_err(|e| format!("Couldn't delete {}: {e}.", dir.display()))?;
    let _ = dir.parent().map(fs::remove_dir);                                       // _mod_backups, once it's empty
    Ok(())
}

/// A backup from list(), not a folder elsewhere.
fn owned(game: &Path, b: &Backup) -> Result<(), String> {
    if b.dir.parent() == Some(home(game)?.as_path()) && b.dir.symlink_metadata().is_ok_and(|m| m.is_dir()) { Ok(()) } else { Err(format!("{} isn't a backup folder.", b.dir.display())) }
}

/// Back up and remove: moves `paths` into a new backup folder. Returns that folder.
pub fn backup(game: &Path, name: &str, why: &str, paths: &[String]) -> Result<PathBuf, String> {
    let from = checked(game, paths, false)?;
    let (root, date) = (home(game)?, today());                // once, so the folder and backup.toml agree across midnight
    let base = format!("{date} {}", safe(name));
    let dir = (1..).map(|n| root.join(if n == 1 { base.clone() } else { format!("{base} ({n})") })).find(|d| d.symlink_metadata().is_err()).unwrap();
    let b = Backup { name: name.into(), why: why.into(), date, paths: paths.to_vec(), dir: PathBuf::new() };
    fs::create_dir_all(&dir).and_then(|_| fs::write(dir.join(LIST), toml::to_string(&b).unwrap_or_default()))   // the list first, so nothing is ever orphaned
        .map_err(|e| format!("Couldn't create {}: {e}.", dir.display()))?;
    shift(&from, &paths.iter().map(|p| dir.join(p)).collect::<Vec<_>>(), paths).inspect_err(|_| prune(&dir))?;
    Ok(dir)
}

/// Delete permanently: backs up first, so a failed move changes nothing, then deletes the backup.
pub fn delete(game: &Path, name: &str, why: &str, paths: &[String]) -> Result<(), String> { erase(&backup(game, name, why, paths)?, paths) }

/// The backups in the game folder, newest first.
pub fn list(game: &Path) -> Vec<Backup> {
    let mut v: Vec<Backup> = home(game).ok().and_then(|d| fs::read_dir(d).ok()).into_iter().flatten().flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))                       // a real folder, not a link to elsewhere
        .filter_map(|e| {
            let f = e.path().join(LIST);
            let t = f.symlink_metadata().ok().filter(|m| m.is_file() && m.len() < 1 << 20).and_then(|_| fs::read(&f).ok())?;
            Some(Backup { dir: e.path(), ..toml::from_str(&String::from_utf8_lossy(&t)).ok()? })
        }).collect();
    v.sort_by(|a, b| b.dir.cmp(&a.dir));                                             // folder names start with the date
    v
}

/// Moves a backup's entries back into the game folder, then removes the empty backup.
pub fn restore(game: &Path, b: &Backup) -> Result<(), String> {
    owned(game, b)?;
    let to = checked(game, &b.paths, true)?;
    let from = checked(&b.dir, &b.paths, false).map_err(|e| format!("The backup can't be restored: {e}"))?;
    shift(&from, &to, &b.paths)?;
    prune(&b.dir);
    Ok(())
}

/// Deletes a backup for good, even one a failed delete left half done.
pub fn discard(game: &Path, b: &Backup) -> Result<(), String> {
    owned(game, b)?;
    if let Some(p) = b.paths.iter().find(|p| !plain(p)) { return Err(format!("The backup's list names {p}, which isn't a mod folder or file.")) }
    erase(&b.dir, &b.paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ccc-remove-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for f in files { fs::create_dir_all(root.join(f).parent().unwrap()).unwrap(); fs::write(root.join(f), f.as_bytes()).unwrap(); }
        root
    }

    #[test]
    fn dates_and_names() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(20_761), "2026-11-04");
        assert_eq!(date(11_016), "2000-02-29");
        assert_eq!(safe("Mod: \"Best\" <v2>/x"), "Mod_ _Best_ _v2__x");
        assert_eq!(safe(" ... "), "mod");
    }

    #[test]
    fn back_up_restore_and_delete() {
        let g = game("cycle", &["r6/scripts/Old/a.reds", "r6/scripts/Old/sub/b.reds", "archive/pc/mod/Old.archive", "r6/scripts/Keep/k.reds"]);
        let paths = vec!["r6/scripts/Old".to_string(), "archive/pc/mod/Old.archive".to_string()];
        let dir = backup(&g, "Old: mod", "It broke.", &paths).unwrap();
        assert!(!g.join("r6/scripts/Old").exists() && !g.join("archive/pc/mod/Old.archive").exists() && g.join("r6/scripts/Keep/k.reds").exists());
        assert!(dir.join("r6/scripts/Old/sub/b.reds").is_file() && dir.join("archive/pc/mod/Old.archive").is_file());
        assert!(name_of(&dir).ends_with(" Old_ mod"), "{}", dir.display());
        let b = list(&g);
        assert_eq!((b.len(), b[0].name.as_str(), b[0].paths.clone(), b[0].dir.clone()), (1, "Old: mod", paths.clone(), dir.clone()));

        fs::create_dir_all(g.join("r6/scripts/Old")).unwrap();                      // installed again: no overwrite
        assert!(restore(&g, &b[0]).unwrap_err().contains("installed again"));
        fs::remove_dir(g.join("r6/scripts/Old")).unwrap();
        restore(&g, &b[0]).unwrap();
        assert_eq!(fs::read_to_string(g.join("r6/scripts/Old/sub/b.reds")).unwrap(), "r6/scripts/Old/sub/b.reds");
        assert!(!dir.exists() && list(&g).is_empty() && !g.join(DIR).exists());

        let second = backup(&g, "Old", "", &paths).unwrap();
        assert!(backup(&g, "Old", "", &["r6/scripts/Keep".to_string()]).unwrap().ends_with(format!("{} (2)", name_of(&second))));
        discard(&g, &list(&g).into_iter().find(|b| b.dir == second).unwrap()).unwrap();
        assert!(!second.exists() && !g.join("r6/scripts/Old").exists());
        delete(&g, "Gone", "", &["archive/pc/mod/x.archive".to_string()]).unwrap_err();   // not there: nothing happens
        fs::write(g.join("archive/pc/mod/x.archive"), b"x").unwrap();
        delete(&g, "Gone", "", &["archive/pc/mod/x.archive".to_string()]).unwrap();
        assert!(!g.join("archive/pc/mod/x.archive").exists() && list(&g).len() == 1);

        let half = list(&g).remove(0);                                                // a delete that stopped halfway
        fs::remove_dir_all(half.dir.join("r6/scripts/Keep")).unwrap();
        assert!(restore(&g, &half).unwrap_err().contains("can't be restored: r6/scripts/Keep wasn't found"));
        discard(&g, &half).unwrap();
        assert!(!g.join(DIR).exists());
    }

    fn name_of(p: &Path) -> String { p.file_name().unwrap().to_string_lossy().into_owned() }

    #[test]
    fn refuses_anything_but_mod_entries() {
        let g = game("refuse", &["bin/x64/Cyberpunk2077.exe", "archive/pc/content/basegame.archive", "r6/scripts/A/a.reds", "bin/x64/plugins/x.asi"]);
        for bad in ["bin/x64/Cyberpunk2077.exe", "archive/pc/content/basegame.archive", "r6/scripts", "r6/scripts/A/a.reds", "r6/scripts/../../x",
                    "r6\\scripts\\A", "/r6/scripts/A", "r6/scripts/C:A", "red4ext/plugins/.."] {
            assert!(backup(&g, "x", "", &[bad.to_string()]).unwrap_err().contains("isn't a mod folder"), "{bad}");
        }
        assert!(g.join("bin/x64/Cyberpunk2077.exe").exists() && !g.join(DIR).exists());
        backup(&g, "Asi", "", &["bin/x64/plugins/x.asi".to_string()]).unwrap();
        let mut b = list(&g).remove(0);                                                // a tampered list can't reach outside
        b.paths = vec!["archive/pc/content/basegame.archive".into()];
        assert!(restore(&g, &b).is_err() && discard(&g, &b).is_err());
        b.dir = g.join("r6");
        assert!(discard(&g, &b).unwrap_err().contains("isn't a backup folder") && restore(&g, &b).unwrap_err().contains("isn't a backup folder"));
        #[cfg(unix)] {                                                                 // _mod_backups linked to elsewhere
            let away = game("away", &[]);
            fs::create_dir_all(&away).unwrap();
            fs::rename(g.join(DIR), away.join("old")).unwrap();
            std::os::unix::fs::symlink(&away, g.join(DIR)).unwrap();
            assert!(backup(&g, "A", "", &["r6/scripts/A".to_string()]).unwrap_err().contains("isn't a plain folder") && list(&g).is_empty());
            assert!(g.join("r6/scripts/A/a.reds").is_file());
        }
    }

    #[cfg(unix)]
    #[test]
    fn never_through_a_link() {
        use std::os::unix::fs::symlink;
        let (g, away) = (game("link", &["r6/tweaks/A/t.yaml"]), game("link-away", &["B/b.reds", "A/keep.txt"]));
        symlink(&away, g.join("r6/scripts")).unwrap();                              // r6/scripts links to a folder outside the game
        assert!(backup(&g, "B", "", &["r6/scripts/B".into()]).unwrap_err().contains("r6/scripts in "));
        assert!(away.join("B/b.reds").is_file() && !g.join(DIR).exists());

        let dir = backup(&g, "A", "", &["r6/tweaks/A".into()]).unwrap();
        fs::remove_dir(g.join("r6/tweaks")).unwrap();                               // then r6/tweaks is linked too
        symlink(&away, g.join("r6/tweaks")).unwrap();
        let b = list(&g).remove(0);
        assert!(restore(&g, &b).unwrap_err().contains("isn't a plain folder") && dir.join("r6/tweaks/A/t.yaml").is_file());

        fs::remove_dir_all(dir.join("r6/tweaks")).unwrap();                         // a link planted in the backup
        symlink(&away, dir.join("r6/tweaks")).unwrap();
        assert!(discard(&g, &b).unwrap_err().contains("isn't a plain folder") && restore(&g, &b).is_err());
        assert!(away.join("A/keep.txt").is_file() && away.join("B/b.reds").is_file(), "nothing outside was touched");
    }

    #[cfg(windows)]
    #[test]
    fn never_through_a_junction() {
        let (g, away) = (game("junction", &["r6/tweaks/A/t.yaml"]), game("junction-away", &["B/b.reds", "A/keep.txt"]));
        let junction = |at: PathBuf| assert!(std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&at).arg(&away).output().unwrap().status.success());
        junction(g.join("r6").join("scripts"));                                     // r6/scripts is a junction to a folder outside the game
        assert!(backup(&g, "B", "", &["r6/scripts/B".into()]).unwrap_err().contains("r6/scripts in "));
        let dir = backup(&g, "A", "", &["r6/tweaks/A".into()]).unwrap();
        fs::remove_dir_all(dir.join("r6").join("tweaks")).unwrap();                  // a junction planted in the backup
        junction(dir.join("r6").join("tweaks"));
        assert!(discard(&g, &list(&g).remove(0)).unwrap_err().contains("isn't a plain folder"));
        assert!(away.join("A/keep.txt").is_file() && away.join("B/b.reds").is_file(), "nothing outside was touched");
    }

    #[test]
    fn a_failed_move_changes_nothing() {
        let g = game("rollback", &["r6/scripts/A/a.reds", "r6/tweaks/A/t.yaml"]);
        let paths = ["r6/scripts/A", "r6/tweaks/A", "r6/scripts/A"].map(String::from);   // the third is gone by then
        assert!(backup(&g, "A", "", &paths).unwrap_err().contains("Nothing was changed"));
        assert!(g.join("r6/scripts/A/a.reds").is_file() && g.join("r6/tweaks/A/t.yaml").is_file());
        assert!(!g.join(DIR).exists(), "the empty backup is cleaned up");
    }
}
