# 1. The Framework Net's data lives in its own file, not in compat.toml

Date: 2026-11-25
Status: accepted

## Context

v2.0 adds the **Framework Net**, which needs data v1 never had: archive **Suite cores** with a
fingerprint per version, the globs that find their **Suite mods**, the lowest core version a suite mod
works with, and corrections to how far a mod reaches (**Contained** or **Engine-reaching**).

The **Compatibility manifest** (compat.toml) is already published as an optional file on the mod's Nexus
page, so a player can update the list without updating the app. Its parser uses
`#[serde(deny_unknown_fields)]`, which is what keeps a hand-edited or tampered file from being read as
something it isn't. That also means a v1.x app refuses any compat.toml carrying fields it doesn't know:
adding the Net's data to it would break the app for everyone who hasn't updated, exactly when they
download the newest list.

## Decision

The Net's data goes in **net.toml**, a separate file with its own `schema` and `updated` date, built into
the release and overridable by a net.toml next to the app, the same way compat.toml already is.
compat.toml keeps schema 1 and gains nothing new, so v1.x apps keep working with the newest list.

Both files are loaded independently, and the app's header names each one's date and where it came from.

## Consequences

- A player on v1.6 can still take a new compat.toml; they just don't get the Net.
- The maintainer (and the Bridge agent) keeps two files. They cover different things — frameworks and
  known clashes in one, cores and reach in the other — so neither duplicates the other.
- Nexus can carry net.toml as a second optional file.
- A later schema change to the Net's data breaks only Net-aware apps, which all read `schema`.
