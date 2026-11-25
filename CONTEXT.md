# Cyberpunk mod compatibility checker

Warns a player, before they launch the game, that an installed mod is likely to break on their game build.

Read-only unless the player asks: **Back up and remove** or **Delete** for a red mod, and **Restore** or delete for a **Backup**. Each shows every file first, and none touches the game's own files or a **Framework**. (The maintainer added these file actions on 2026-11-04, before the first release; until then the app was planned as read-only for good.)

App name: Cyberpunk Mod Compatibility Checker. Repo: Adaptive-Protocol/Cyberpunk-Mod-Compatibility-Checker. Own Nexus page, separate from the dedupe app.

## Language

**Game build**:
The Cyberpunk 2077 version installed on the player's machine (e.g. 2.5).
_Avoid_: patch, game version (when ambiguous with a mod's version)

**Framework**:
A core mod that hooks the game at a low level and must be updated after most game builds (RED4ext, CET, redscript, ArchiveXL, TweakXL, Codeware, ...). the maintainer curates the list.
_Avoid_: mod architecture, core mod (fine in prose, not as the term)

**Dependent mod**:
A mod that relies heavily on one or more Frameworks, or touches the engine directly (engine, audio, weather mods are typical), so it is a likely culprit after a new Game build.

**Compatibility manifest**:
the maintainer's curated file mapping Game builds to the Framework versions and Dependent mods known to work with them. Lives in the maintainer's GitHub repo; the utility only ever reads a local copy.
_Avoid_: update status, update manifest

**Last launch**:
The game's most recent run, as its logs record it: RED4ext's log, redscript's log, mods' own logs (ArchiveXL's names the world edits it skipped) and Advanced Crash Reporter's reports. Its findings update after each launch.

**Loader**:
A file in bin/x64 that loads a **Framework**: RED4ext's winmm.dll, or the ASI loader version.dll that loads CET and .asi plugins. Listed as the Framework's `parts`.

**Pattern**:
A known clash between files, kept in the **Compatibility manifest** and checked on every scan: red when all its paths are installed and none of its `none` paths are. Its optional `remove` names what the app may offer to remove. A path through an archive (`archive/pc/mod/*.archive/<game file>`) instead flags each mod archive whose copy of that game file is older than the game's own, such as a sound index from an older patch, read from the archives' file indexes.

**Mod entry**:
A whole folder or file directly inside a mod folder (archive/pc/mod, r6/scripts, r6/tweaks, red4ext/plugins, CET's mods, REDmod's mods, or an .asi in bin/x64/plugins). The only thing the app may move or delete.
_Avoid_: mod file (a mod usually has several entries)

**Leftover**:
A file the **Last launch** shows is in the way: a mod's own log calls it older, conflicting or still installed ("Conflicting older … component: <path>"), or RED4ext closed the game because the script declares native code no loaded plugin provides. Red. Remove offers it, with the same-named files beside it, only when it's a whole **Mod entry**; one inside another mod's folder is left to the player.

**Companion**:
A **Mod entry** with the same name as a red mod's own entry, in another mod folder or beside it (e.g. r6/tweaks/X next to r6/scripts/X, or X.archive next to X.xl). It's offered with the mod and marked "same name". For a **Leftover**, only files beside it count, so the mod's current version is never offered.

**Add-on**:
A mod whose scripts build on a red mod: they import a module only that mod declares, or use a class or function it defines in a module they import or add to (EquipmentEx's add-ons add to its module). Without the red mod, redscript can't compile the add-on, which stops every script mod, so Remove offers the add-on and its **Companions** with the mod, marked "builds on X". Mods that build on an add-on count too. An import under `@if(ModuleExists(..))` is optional and doesn't count.
_Avoid_: dependency (a **Dependent mod** depends on a **Framework**)

**Suite core** (planned, v2.0):
One mod author's shared base that the rest of their mods need: scripts, strings, data or quest content (e.g. DigitalVixen Core, Deceptious Quest Core). Unlike a **Framework**, it's not on the maintainer's low-level list and isn't tied to a Game build. A script core is recognised by its scripts and may carry a version marker, a module a **Suite mod** imports to require that version; an archive core only by its archive, with a fingerprint per version in the **Compatibility manifest**.
_Avoid_: cross core, core mod

**Suite mod** (planned, v2.0):
A mod that needs a **Suite core**. It's red when its core is missing, older than it needs, or installed twice; shown grouped under its core.

**Contained**, **Engine-reaching** (planned, v2.0):
How far a mod reaches. A contained mod only adds its own files and records (Zenitex Core Dependency: its items, stats and store), so it can break only itself and the **Suite mods** that need it. An engine-reaching mod has scripts that change game code, so it can affect other mods and the game. It can be a **Suite core** (DigitalVixen Core, They Will Remember) or an overhaul nothing depends on (Trigger Mode Control, Combat Evolved). The app reads this from the mod's files, and the **Compatibility manifest** can correct it. Every engine-reaching mod is cross-checked against the others for **Overlaps**; a contained mod is checked only within its own suite.
_Avoid_: isolated, engine-level

**Framework Net** (planned, v2.0):
The map of everything other mods stand on. Every **Framework** and every **Suite core** is a point in it, joined by what each one needs and what it clashes with. The app reads it before launch to say whether the installed points work together.
_Avoid_: framework for a **Suite core** (the Net holds both kinds, and they stay separate terms)

**Overlap** (planned, v2.0):
Two installed mods replacing the same game file or the same script method, so only one of them takes effect. The **Framework Net** shows it as a neutral note naming both, and the winner where it can tell (for archives, the one earlier by name). When one mod replaces a method and another wraps it, both still run, so that is only a detail line inside the replacing mod's row. Two mods wrapping the same method chain safely and aren't shown. Never a verdict.
_Avoid_: conflict, clash (a clash is a known break the **Compatibility manifest** names as a **Pattern**)

**Pre-install check** (planned, v2.0):
A separate window where the player drops a mod's archive (or chooses it) before installing it. The app reads the **Candidate** in memory and checks it alone against the **Framework Net**, the **Compatibility manifest** and the installed mods, pointing to likely break points by file and line. It never installs, extracts, writes or runs anything, so a clean result means no certain breaks were found, not that the mod is safe. It takes any mod: an **Engine-reaching** **Candidate** gets the full report (methods, records, hooks), a **Contained** one a short check of what it needs and which game files it replaces.
_Avoid_: sandbox (nothing runs in isolation), test

**Candidate** (planned, v2.0):
The mod archive given to the **Pre-install check**.

**Back up and remove**:
Moving a red mod's ticked **Mod entries** into a **Backup** until the mod is updated.
_Avoid_: disable, uninstall, quarantine

**Backup**:
A folder `_mod_backups/<date> <mod>/` in the game folder that holds the moved **Mod entries** at their original paths, plus backup.toml (name, why, date, paths).

**Restore**:
Moving a **Backup**'s entries back into the game folder. It's refused if any of them has been installed again, and not offered for an **Obsolete backup**.

**Obsolete backup**:
A **Backup** whose mod has a newer version installed: a **Mod entry** in the same mod folder with the same name, or the same name with another version number in it, modified later than the backed-up copy and not identical to it. Each scan looks for them and, under "Updated mods detected", offers to delete them. Dates decide, not the numbers in names.
_Avoid_: old backup, stale backup

**Delete**:
Permanently removing ticked **Mod entries** after a second confirmation, or removing a **Backup**.

**Bridge agent**:
A developer-side Claude agent the maintainer runs with one input: a new Game build, a Framework or mod update, or a pasted **Bug report**. It proposes an updated Compatibility manifest in a PR and sends the **Builder note**. It never ships to players.

**Bug report**:
A problem a player reported (usually in Nexus comments or the bugs tab), pasted to the Bridge agent by the maintainer. One report alone changes nothing players see; it waits in `bridge/reports.md` until it's confirmed.

**Builder note**:
The Bridge agent's summary for whoever builds the app: what changed in the list, what players will now see, and any finding the list can't express, which needs a new check in the app.

## Relationships

- A **Compatibility manifest** has one entry per **Game build**.
- Each entry lists the minimum working version of each **Framework**, and the **Dependent mods** at risk with the **Frameworks** they need.
- Each **Framework** and **Dependent mod** carries its Nexus mod ID, which gives a fixed mod page link (nexusmods.com/cyberpunk2077/mods/<id>).
- The utility scans the player's game install folder (not a downloads folder).
- A mod page link opens in the player's own browser only when they click it; the utility never connects itself. Links are built from the Nexus mod ID, so they always point at nexusmods.com. A security audit runs after the build.
- Verdicts: red (likely to break), green (OK for this Game build), gray (not confirmed: mod not in manifest, or manifest older than the Game build). A banner warns when the Game build is newer than the manifest.
- A **Framework** can be marked build-locked (CET almost never survives a patch): on an unconfirmed newer Game build it shows red, not gray.
- A **Dependent mod** is recognised by a marker file or folder only (no version); its verdict comes from the **Frameworks** it needs, plus any known-broken note (e.g. "broken on 2.6, fixed in 1.3").
- Known-broken notes come from GitHub releases and from bug reports the maintainer collects from Nexus comments; each note records its source and date.
- The utility finds the game folder itself (Steam, GOG, Epic; Windows and Linux), with a folder picker as fallback. If it detects Mod Organizer 2, it also scans the MO2 mods folder (v1).
- Only **Mod entries** inside the game folder can be moved or deleted, never a **Framework**, its **Loader** or the game's own files. A path that leaves the game folder is refused, including one from a drop-in compat.toml.
- Mods installed with Mod Organizer 2 are never moved: the app says to untick them in MO2.
- In the **Framework Net** a row is red only for a certain break: something it needs is missing or older than required, a **Suite core** is installed twice, or the **Compatibility manifest** names the clash. An **Overlap** never turns a row red.
- macOS (planned, v2.0, Apple silicon): the app checks only what the Mac game can load: script imports, mods that need a **Framework** macOS doesn't have, and duplicates. Community ports of **Frameworks** aren't checked, and almost no **Suite core** runs there, so the **Framework Net** is close to empty on a Mac.
- On macOS the app is read-only: it points to the problem and the fix, with no **Back up and remove**, **Delete** or launch mode, and it never changes the game's own archives. File actions come to macOS only if the maintainer decides so later, after a test on a real Mac install.
- **Delete** backs up first and then deletes the **Backup**, so a failed move changes nothing.
- The **Bridge agent** (.claude/agents/compat-bridge.md) writes the **Compatibility manifest**; the utility reads it, offline.

## Flagged ambiguities

- "update status" / "directed update status" resolved to **Compatibility manifest**.
- "read-only" (the 2026-10-22 plan) resolved to read-only unless the player asks for a file action (2026-11-04).
- "cross core" (the 2026-11-08 v2 plan) resolved to **Suite core** and **Suite mod**, since "core mod" isn't a term here and **Framework** means the low-level list.
- "framework" for Zenitex Core Dependency and DigitalVixen Core (the 2026-11-24 v2 plan) resolved: those are **Suite cores**, and **Framework Net** is the map that holds **Frameworks** and **Suite cores** alike.
