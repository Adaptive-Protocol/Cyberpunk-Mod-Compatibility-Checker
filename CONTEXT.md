# Cyberpunk mod compatibility checker

Warns a player, before they launch the game, that an installed mod is likely to break on their game build.

Strictly read-only, now and in all future versions: a notification app that never changes, moves or deletes the player's files.

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

**Bridge agent**:
A developer-side Claude agent the maintainer runs after a new Game build. It researches Framework support and proposes an updated Compatibility manifest. It never ships to players.

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
- The **Bridge agent** (.claude/agents/compat-bridge.md) writes the **Compatibility manifest**; the utility reads it, offline.

## Flagged ambiguities

- "update status" / "directed update status" resolved to **Compatibility manifest**.
