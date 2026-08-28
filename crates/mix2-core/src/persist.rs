//! Writing user choices back to `config.toml`: the team picked in the
//! startup picker and the per-turn consultation budget (`/turns`). Edits
//! are surgical — `toml_edit` keeps the user's comments, ordering, and
//! every key we don't touch — and atomic (temp file + rename), so a crash
//! mid-write never leaves a half-written config behind.
//!
//! Only interactive choices are ever written: auto-confirmed teams, CLI
//! `--lead` overrides, and `dev` runs leave the file alone.

use crate::agents::{HarnessKind, SlotId, Team};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut, Item, Table, Value};

/// Persist a picked team (and, when the picker changed it, the budget) in
/// one write. The team lands in the canonical slot-keyed schema: `lead`
/// plus `[slot.one]`/`[slot.two]` `harness` keys — their presence is what
/// makes later runs auto-confirm instead of showing the picker.
///
/// `previous` is the team the config resolved to before the pick. Slot
/// `command`/`model` pins belong to the harness that was on the slot, so
/// when a slot's harness changes its pins *follow the harness* rather than
/// silently pointing a different CLI at them: to the slot the harness
/// moved to, else to its harness-keyed `[claude]`/`[codex]` section (which
/// applies wherever that harness is selected), else — for harnesses with
/// no such section — they are dropped. Returns a human-readable note per
/// pin moved or dropped, for the confirmation shown to the user.
pub fn save_selection(
    path: &Path,
    team: Team,
    previous: Team,
    max_consults: Option<u32>,
) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    edit(path, |doc| {
        set_string(doc, "lead", &team.lead.to_string());
        // Detach every changed slot's pins first (they are keyed by the old
        // harness), then re-home them once the new layout is known.
        let mut orphans: Vec<(SlotId, HarnessKind, Pins)> = Vec::new();
        {
            let slots = ensure_table(doc, "slot", true);
            for slot in SlotId::ALL {
                let entry = ensure_table(slots, &slot.to_string(), false);
                let old = previous.harness(slot);
                if team.harness(slot) != old {
                    let pins = Pins::take(entry);
                    if !pins.is_empty() {
                        orphans.push((slot, old, pins));
                    }
                }
                set_string(entry, "harness", &team.harness(slot).to_string());
            }
        }
        for (from, harness, pins) in orphans {
            // The harness moved onto a slot that also changed: its pins
            // move with it (that slot's own stale pins were detached above).
            let moved_to = SlotId::ALL
                .into_iter()
                .find(|s| team.harness(*s) == harness && previous.harness(*s) != harness);
            if let Some(to) = moved_to {
                let entry = ensure_table(ensure_table(doc, "slot", true), &to.to_string(), false);
                pins.write(entry);
                notes.push(format!(
                    "moved [slot.{from}] {} to [slot.{to}]",
                    pins.names()
                ));
                continue;
            }
            match legacy_section(harness) {
                Some(section) => {
                    let table = ensure_table(doc, section, false);
                    // A harness-level value the user already wrote wins over
                    // a slot pin being demoted to it.
                    let kept = pins.write_if_absent(table);
                    if !kept.is_empty() {
                        notes.push(format!(
                            "moved [slot.{from}] {} to [{section}]",
                            kept.join("/")
                        ));
                    }
                    let shadowed: Vec<&str> = pins
                        .present()
                        .into_iter()
                        .filter(|k| !kept.contains(k))
                        .collect();
                    if !shadowed.is_empty() {
                        notes.push(format!(
                            "dropped [slot.{from}] {} ([{section}] already sets it)",
                            shadowed.join("/")
                        ));
                    }
                }
                None => notes.push(format!(
                    "dropped [slot.{from}] {} ({harness} has no harness section)",
                    pins.names()
                )),
            }
        }
        if let Some(max) = max_consults {
            set_max_consults(doc, max);
        }
    })?;
    Ok(notes)
}

/// Persist the per-turn consultation budget (`[collaboration]
/// max_consults_per_turn`). Validation is the caller's job.
pub fn save_max_consults(path: &Path, max: u32) -> Result<()> {
    edit(path, |doc| set_max_consults(doc, max))
}

fn set_max_consults(doc: &mut Table, max: u32) {
    let table = ensure_table(doc, "collaboration", false);
    table["max_consults_per_turn"] = value(i64::from(max));
}

/// The harness-keyed legacy section that can hold a harness's pins.
fn legacy_section(harness: HarnessKind) -> Option<&'static str> {
    match harness {
        HarnessKind::Claude => Some("claude"),
        HarnessKind::Codex => Some("codex"),
        HarnessKind::Copilot | HarnessKind::Cursor | HarnessKind::Opencode => None,
    }
}

/// A slot's `command`/`model` pins, detached from their table.
#[derive(Default)]
struct Pins {
    command: Option<Item>,
    model: Option<Item>,
}

impl Pins {
    const KEYS: [&'static str; 2] = ["command", "model"];

    fn take(table: &mut Table) -> Self {
        Self {
            command: table.remove("command"),
            model: table.remove("model"),
        }
    }

    fn is_empty(&self) -> bool {
        self.command.is_none() && self.model.is_none()
    }

    fn get(&self, key: &str) -> Option<&Item> {
        match key {
            "command" => self.command.as_ref(),
            _ => self.model.as_ref(),
        }
    }

    fn present(&self) -> Vec<&'static str> {
        Self::KEYS
            .into_iter()
            .filter(|k| self.get(k).is_some())
            .collect()
    }

    fn names(&self) -> String {
        self.present().join("/")
    }

    fn write(&self, table: &mut Table) {
        for key in self.present() {
            table[key] = self.get(key).expect("present").clone();
        }
    }

    /// Write only the pins the table doesn't already set; returns those.
    fn write_if_absent(&self, table: &mut Table) -> Vec<&'static str> {
        let mut written = Vec::new();
        for key in self.present() {
            if table.get(key).is_none() {
                table[key] = self.get(key).expect("present").clone();
                written.push(key);
            }
        }
        written
    }
}

/// Load-or-create, mutate, write atomically. The file is parsed as a
/// `toml_edit` document so untouched content round-trips byte-for-byte.
/// A symlinked config (dotfile managers) is followed: the file it points
/// at is what gets rewritten, and its permissions are kept.
fn edit(path: &Path, mutate: impl FnOnce(&mut Table)) -> Result<()> {
    let (target, existing) = match std::fs::read_to_string(path) {
        Ok(text) => {
            let resolved = path
                .canonicalize()
                .with_context(|| format!("failed to resolve {}", path.display()))?;
            (resolved, Some(text))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (path.to_path_buf(), None),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    };
    let mut doc: DocumentMut = existing
        .as_deref()
        .unwrap_or("")
        .parse()
        .with_context(|| format!("invalid config {}", path.display()))?;
    mutate(doc.as_table_mut());

    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config.toml".to_owned()),
        std::process::id()
    ));
    let commit = (|| -> Result<()> {
        std::fs::write(&tmp, doc.to_string())
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        if existing.is_some() {
            if let Ok(meta) = std::fs::metadata(&target) {
                let _ = std::fs::set_permissions(&tmp, meta.permissions());
            }
        }
        std::fs::rename(&tmp, &target)
            .with_context(|| format!("failed to replace {}", target.display()))
    })();
    if commit.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    commit
}

/// Get-or-insert a sub-table. `implicit` tables (`[slot]`) never render a
/// header of their own — only their children (`[slot.one]`) do. An inline
/// table (`slot = { one = { ... } }`) is promoted to a regular table so it
/// can be edited in place.
fn ensure_table<'a>(parent: &'a mut Table, key: &str, implicit: bool) -> &'a mut Table {
    if let Some(inline) = parent
        .get(key)
        .and_then(Item::as_value)
        .and_then(Value::as_inline_table)
        .cloned()
    {
        parent[key] = Item::Table(inline.into_table());
    }
    if !parent.get(key).is_some_and(Item::is_table) {
        let mut table = Table::new();
        table.set_implicit(implicit);
        parent[key] = Item::Table(table);
    }
    parent[key].as_table_mut().expect("just ensured a table")
}

/// Overwrite a string key while keeping an existing value's decoration
/// (its inline comment and surrounding whitespace).
fn set_string(table: &mut Table, key: &str, text: &str) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = Value::from(text);
            *existing.decor_mut() = decor;
        }
        None => table[key] = value(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_file, Config};

    fn team(one: HarnessKind, two: HarnessKind, lead: SlotId) -> Team {
        Team { one, two, lead }
    }

    const DEFAULT: Team = Team {
        one: HarnessKind::Claude,
        two: HarnessKind::Codex,
        lead: SlotId::One,
    };

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        (dir, path)
    }

    fn resolve(path: &Path) -> Config {
        Config::resolve(None, &load_file(Some(path)).unwrap()).unwrap()
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn writes_a_fresh_file_that_auto_confirms_next_time() {
        let (_dir, path) = tmp();
        let notes = save_selection(
            &path,
            team(HarnessKind::Codex, HarnessKind::Codex, SlotId::Two),
            DEFAULT,
            Some(3),
        )
        .unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            read(&path),
            "lead = \"two\"\n\n[slot.one]\nharness = \"codex\"\n\n[slot.two]\nharness = \"codex\"\n\n[collaboration]\nmax_consults_per_turn = 3\n"
        );
        // Round-trips through the real loader and marks the team explicit.
        let cfg = resolve(&path);
        assert!(cfg.explicit_slots);
        assert_eq!(cfg.team.one, HarnessKind::Codex);
        assert_eq!(cfg.team.two, HarnessKind::Codex);
        assert_eq!(cfg.team.lead, SlotId::Two);
        assert_eq!(cfg.max_consults_per_turn, 3);
    }

    #[test]
    fn an_empty_file_behaves_like_an_absent_one() {
        let (_dir, path) = tmp();
        std::fs::write(&path, "").unwrap();
        save_selection(&path, DEFAULT, DEFAULT, None).unwrap();
        assert_eq!(
            read(&path),
            "lead = \"one\"\n\n[slot.one]\nharness = \"claude\"\n\n[slot.two]\nharness = \"codex\"\n"
        );
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mix2").join("config.toml");
        save_max_consults(&path, 3).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn keeps_comments_legacy_sections_and_unrelated_keys() {
        let (_dir, path) = tmp();
        std::fs::write(
            &path,
            "# my config\nlead = \"codex\"   # who leads\n\n[sandbox]\nmode = \"off\"\n\n[claude]\ncommand = \"/c\"\n\n[codex]\nmodel = \"gpt-5\"\n",
        )
        .unwrap();
        // Legacy `lead = "codex"` resolved to slot two; the pick moves claude to lead.
        let previous = team(HarnessKind::Claude, HarnessKind::Codex, SlotId::Two);
        let notes = save_selection(&path, DEFAULT, previous, None).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            read(&path),
            "# my config\nlead = \"one\"   # who leads\n\n[sandbox]\nmode = \"off\"\n\n[claude]\ncommand = \"/c\"\n\n[codex]\nmodel = \"gpt-5\"\n\n[slot.one]\nharness = \"claude\"\n\n[slot.two]\nharness = \"codex\"\n"
        );
        let cfg = resolve(&path);
        assert_eq!(cfg.team.lead, SlotId::One);
        // The legacy sections still feed their harnesses.
        assert_eq!(cfg.slot(SlotId::One).command, "/c");
        assert_eq!(cfg.slot(SlotId::Two).model.as_deref(), Some("gpt-5"));
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn swapping_the_slots_moves_each_pin_with_its_harness() {
        let (_dir, path) = tmp();
        std::fs::write(
            &path,
            "lead = \"one\"\n\n[slot.one]\nharness = \"claude\"\ncommand = \"/custom/claude\"\nmodel = \"opus\"\n\n[slot.two]\nharness = \"codex\"\nmodel = \"gpt-5\"\n",
        )
        .unwrap();
        let notes = save_selection(
            &path,
            team(HarnessKind::Codex, HarnessKind::Claude, SlotId::Two),
            DEFAULT,
            None,
        )
        .unwrap();
        assert_eq!(
            notes,
            vec![
                "moved [slot.one] command/model to [slot.two]",
                "moved [slot.two] model to [slot.one]"
            ]
        );
        let cfg = resolve(&path);
        assert_eq!(cfg.team.one, HarnessKind::Codex);
        assert_eq!(cfg.team.two, HarnessKind::Claude);
        assert_eq!(cfg.slot(SlotId::One).command, "codex");
        assert_eq!(cfg.slot(SlotId::One).model.as_deref(), Some("gpt-5"));
        assert_eq!(cfg.slot(SlotId::Two).command, "/custom/claude");
        assert_eq!(cfg.slot(SlotId::Two).model.as_deref(), Some("opus"));
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn a_harness_leaving_the_team_keeps_its_pins_in_its_own_section() {
        let (_dir, path) = tmp();
        std::fs::write(
            &path,
            "[slot.one]\nharness = \"claude\"\ncommand = \"/custom/claude\"\nmodel = \"opus\"\n\n[slot.two]\nharness = \"codex\"\nmodel = \"gpt-5\"\n",
        )
        .unwrap();
        // Slot one switches to codex; slot two stays codex.
        let notes = save_selection(
            &path,
            team(HarnessKind::Codex, HarnessKind::Codex, SlotId::One),
            DEFAULT,
            None,
        )
        .unwrap();
        assert_eq!(notes, vec!["moved [slot.one] command/model to [claude]"]);
        let cfg = resolve(&path);
        // Claude's pins must not follow the slot to codex…
        assert_eq!(cfg.slot(SlotId::One).command, "codex");
        assert_eq!(cfg.slot(SlotId::One).model, None);
        // …but they are still there for claude, wherever it's picked next.
        assert_eq!(cfg.fallback_command(HarnessKind::Claude), "/custom/claude");
        assert_eq!(
            cfg.fallback_model(HarnessKind::Claude).as_deref(),
            Some("opus")
        );
        // The untouched slot keeps its pin.
        assert_eq!(cfg.slot(SlotId::Two).model.as_deref(), Some("gpt-5"));
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn an_existing_harness_section_value_is_not_overwritten() {
        let (_dir, path) = tmp();
        std::fs::write(
            &path,
            "[claude]\nmodel = \"sonnet\"\n\n[slot.one]\nharness = \"claude\"\ncommand = \"/c\"\nmodel = \"opus\"\n",
        )
        .unwrap();
        let notes = save_selection(
            &path,
            team(HarnessKind::Codex, HarnessKind::Codex, SlotId::One),
            DEFAULT,
            None,
        )
        .unwrap();
        assert_eq!(
            notes,
            vec![
                "moved [slot.one] command to [claude]",
                "dropped [slot.one] model ([claude] already sets it)"
            ]
        );
        let cfg = resolve(&path);
        assert_eq!(cfg.fallback_command(HarnessKind::Claude), "/c");
        assert_eq!(
            cfg.fallback_model(HarnessKind::Claude).as_deref(),
            Some("sonnet")
        );
    }

    #[test]
    fn pins_of_a_harness_without_a_section_are_dropped_and_reported() {
        let (_dir, path) = tmp();
        std::fs::write(
            &path,
            "[slot.two]\nharness = \"cursor\"\ncommand = \"/cursor-agent\"\n",
        )
        .unwrap();
        let previous = team(HarnessKind::Claude, HarnessKind::Cursor, SlotId::One);
        let notes = save_selection(&path, DEFAULT, previous, None).unwrap();
        assert_eq!(
            notes,
            vec!["dropped [slot.two] command (cursor has no harness section)"]
        );
        let cfg = resolve(&path);
        assert_eq!(cfg.slot(SlotId::Two).command, "codex");
    }

    #[test]
    fn slot_entry_without_harness_key_is_treated_as_its_legacy_pairing() {
        let (_dir, path) = tmp();
        // `[slot.two]` with no harness means codex; keeping codex keeps the model.
        std::fs::write(&path, "[slot.two]\nmodel = \"gpt-5\"\n").unwrap();
        save_selection(&path, DEFAULT, DEFAULT, None).unwrap();
        let cfg = resolve(&path);
        assert_eq!(cfg.slot(SlotId::Two).model.as_deref(), Some("gpt-5"));
        assert_eq!(cfg.team.two, HarnessKind::Codex);
    }

    #[test]
    fn inline_slot_tables_are_edited_not_replaced() {
        let (_dir, path) = tmp();
        std::fs::write(
            &path,
            "slot = { one = { harness = \"claude\", model = \"opus\" }, two = { harness = \"codex\" } }\n",
        )
        .unwrap();
        save_selection(&path, DEFAULT, DEFAULT, None).unwrap();
        let cfg = resolve(&path);
        assert_eq!(cfg.slot(SlotId::One).model.as_deref(), Some("opus"));
        assert_eq!(cfg.team.lead, SlotId::One);

        // The `[slot]` + inline entries spelling, too.
        std::fs::write(
            &path,
            "[slot]\none = { harness = \"claude\", model = \"opus\" }\n",
        )
        .unwrap();
        save_selection(&path, DEFAULT, DEFAULT, None).unwrap();
        let cfg = resolve(&path);
        assert_eq!(cfg.slot(SlotId::One).model.as_deref(), Some("opus"));
        assert_eq!(cfg.team.two, HarnessKind::Codex);
    }

    #[test]
    fn max_consults_is_written_and_updated_in_place() {
        let (_dir, path) = tmp();
        save_max_consults(&path, 3).unwrap();
        assert_eq!(resolve(&path).max_consults_per_turn, 3);

        save_max_consults(&path, 1).unwrap();
        let text = read(&path);
        assert_eq!(text, "[collaboration]\nmax_consults_per_turn = 1\n");
        // Repeated saves never duplicate the key.
        assert_eq!(text.matches("max_consults_per_turn").count(), 1);
    }

    #[test]
    fn invalid_existing_file_is_an_error_not_an_overwrite() {
        let (_dir, path) = tmp();
        std::fs::write(&path, "this is = = not toml").unwrap();
        let err = save_max_consults(&path, 2).unwrap_err();
        assert!(err.to_string().contains("invalid config"), "{err:#}");
        assert_eq!(read(&path), "this is = = not toml");
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let (dir, path) = tmp();
        save_selection(&path, DEFAULT, DEFAULT, None).unwrap();
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["config.toml".to_owned()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_is_rewritten_through_the_link_with_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles").join("mix2.toml");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "[codex]\nmodel = \"gpt-5\"\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        save_max_consults(&link, 4).unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link must survive"
        );
        assert!(read(&real).contains("max_consults_per_turn = 4"));
        assert_eq!(
            std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
