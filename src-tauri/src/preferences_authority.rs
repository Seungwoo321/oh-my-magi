use magi_domain::{Digest, canonical_json};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Deserializer, Serialize};

use super::ConsolePreferences;

const VERSION: u8 = 1;
const MAX_LEGACY_BYTES: usize = 4096;
const MAX_ID_BYTES: usize = 128;
const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    InvalidInput,
    CorruptAuthority,
    Conflict,
    IdempotencyConflict,
    Unavailable,
    Overflow,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencePatch {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub motion: Option<super::MotionPreference>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub sound: Option<bool>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub theme: Option<super::ThemePreference>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub font_scale: Option<u16>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub language: Option<super::UiLanguage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpectedFieldRevisions {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub motion: Option<u64>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub sound: Option<u64>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub theme: Option<u64>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub font_scale: Option<u64>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub language: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldRevisions {
    pub motion: u64,
    pub sound: u64,
    pub theme: u64,
    pub font_scale: u64,
    pub language: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsSnapshot {
    pub schema_version: u8,
    pub revision: u64,
    pub preferences: ConsolePreferences,
    pub field_revisions: FieldRevisions,
}

impl SettingsSnapshot {
    fn validate(&self) -> Result<(), SettingsError> {
        self.preferences
            .validate()
            .map_err(|_| SettingsError::CorruptAuthority)?;
        if self.schema_version != VERSION
            || self.revision > MAX_SAFE_REVISION
            || [
                self.field_revisions.motion,
                self.field_revisions.sound,
                self.field_revisions.theme,
                self.field_revisions.font_scale,
                self.field_revisions.language,
            ]
            .iter()
            .any(|value| *value > self.revision)
        {
            return Err(SettingsError::CorruptAuthority);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsCommand {
    pub schema_version: u8,
    pub command_id: String,
    pub idempotency_key: String,
    pub target: String,
    pub patch: PreferencePatch,
    pub expected_field_revisions: ExpectedFieldRevisions,
}

fn valid_identity(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_ID_BYTES && !value.chars().any(char::is_control)
}

impl SettingsCommand {
    fn validate(&self) -> Result<(), SettingsError> {
        let pairs = [
            (
                self.patch.motion.is_some(),
                self.expected_field_revisions.motion,
            ),
            (
                self.patch.sound.is_some(),
                self.expected_field_revisions.sound,
            ),
            (
                self.patch.theme.is_some(),
                self.expected_field_revisions.theme,
            ),
            (
                self.patch.font_scale.is_some(),
                self.expected_field_revisions.font_scale,
            ),
            (
                self.patch.language.is_some(),
                self.expected_field_revisions.language,
            ),
        ];
        if self.schema_version != VERSION
            || self.target != "console_preferences"
            || !valid_identity(&self.command_id)
            || !valid_identity(&self.idempotency_key)
            || !pairs.iter().any(|(dirty, _)| *dirty)
            || pairs.iter().any(|(dirty, expected)| {
                *dirty != expected.is_some()
                    || expected.is_some_and(|revision| revision > MAX_SAFE_REVISION)
            })
            || self
                .patch
                .font_scale
                .is_some_and(|value| ![100, 125, 150, 200].contains(&value))
        {
            return Err(SettingsError::InvalidInput);
        }
        Ok(())
    }

    fn digest(&self) -> Result<Digest, SettingsError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Intent<'a> {
            schema_version: u8,
            target: &'a str,
            patch: &'a PreferencePatch,
            expected_field_revisions: &'a ExpectedFieldRevisions,
        }
        canonical_json(&Intent {
            schema_version: self.schema_version,
            target: &self.target,
            patch: &self.patch,
            expected_field_revisions: &self.expected_field_revisions,
        })
        .map(|bytes| Digest::from_bytes(&bytes))
        .map_err(|_| SettingsError::InvalidInput)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsReceipt {
    pub schema_version: u8,
    pub command_id: String,
    pub idempotency_key: String,
    pub intent_digest: Digest,
    pub committed_at: String,
    pub snapshot: SettingsSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsEvent {
    pub schema_version: u8,
    pub revision: u64,
    pub command_id: String,
    pub snapshot: SettingsSnapshot,
}

fn encode<T: Serialize>(value: &T) -> Result<String, SettingsError> {
    serde_json::to_string(value).map_err(|_| SettingsError::InvalidInput)
}

fn decode<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, SettingsError> {
    serde_json::from_str(value).map_err(|_| SettingsError::CorruptAuthority)
}

fn database<T>(result: rusqlite::Result<T>) -> Result<T, SettingsError> {
    result.map_err(|_| SettingsError::Unavailable)
}

const SETTINGS_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS settings_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS settings_commands(command_id TEXT PRIMARY KEY, idempotency_key TEXT NOT NULL UNIQUE, intent_digest TEXT NOT NULL, intent TEXT NOT NULL, receipt TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS settings_aliases(command_id TEXT PRIMARY KEY, original_command_id TEXT NOT NULL REFERENCES settings_commands(command_id));
         CREATE TABLE IF NOT EXISTS settings_events(revision INTEGER PRIMARY KEY CHECK(revision>0), command_id TEXT NOT NULL UNIQUE REFERENCES settings_commands(command_id), payload TEXT NOT NULL, delivered INTEGER NOT NULL DEFAULT 0 CHECK(delivered IN(0,1)));
         CREATE TABLE IF NOT EXISTS settings_import(singleton INTEGER PRIMARY KEY CHECK(singleton=1), legacy_digest TEXT);
         CREATE TRIGGER IF NOT EXISTS settings_command_no_update BEFORE UPDATE ON settings_commands BEGIN SELECT RAISE(ABORT,'immutable settings command'); END;
         CREATE TRIGGER IF NOT EXISTS settings_command_no_delete BEFORE DELETE ON settings_commands BEGIN SELECT RAISE(ABORT,'immutable settings command'); END;
         CREATE TRIGGER IF NOT EXISTS settings_alias_no_update BEFORE UPDATE ON settings_aliases BEGIN SELECT RAISE(ABORT,'immutable settings alias'); END;
         CREATE TRIGGER IF NOT EXISTS settings_alias_no_delete BEFORE DELETE ON settings_aliases BEGIN SELECT RAISE(ABORT,'immutable settings alias'); END;";

fn schema_definition(
    connection: &Connection,
) -> Result<Vec<(String, String, String)>, SettingsError> {
    let mut statement = database(connection.prepare(
        "SELECT type,name,sql FROM sqlite_master WHERE name NOT GLOB 'sqlite_*' ORDER BY type,name",
    ))?;
    database(statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| SettingsError::CorruptAuthority)
}

// The caller owns the safely opened stable settings database, separate from Run restore.
pub(super) fn initialize_with_defaults(
    connection: &mut Connection,
    legacy: Option<&[u8]>,
    defaults: ConsolePreferences,
) -> Result<(), SettingsError> {
    defaults
        .validate()
        .map_err(|_| SettingsError::InvalidInput)?;
    database(connection.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
    ))?;
    let tx = database(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
    let version: u32 = database(tx.query_row("PRAGMA user_version", [], |row| row.get(0)))?;
    let objects: u64 = database(tx.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT GLOB 'sqlite_*'",
        [],
        |row| row.get(0),
    ))?;
    let fresh = objects == 0 && version == 0;
    if fresh {
        database(tx.execute_batch(SETTINGS_SCHEMA))?;
        database(tx.pragma_update(None, "user_version", VERSION))?;
    } else {
        if version != u32::from(VERSION) {
            return Err(SettingsError::CorruptAuthority);
        }
        let expected = database(Connection::open_in_memory())?;
        database(expected.execute_batch(SETTINGS_SCHEMA))?;
        if schema_definition(&tx)? != schema_definition(&expected)? {
            return Err(SettingsError::CorruptAuthority);
        }
    }
    let existing: Option<String> = database(
        tx.query_row(
            "SELECT payload FROM settings_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional(),
    )?;
    if existing.is_some() {
        load(&tx)?;
        let imports: u64 =
            database(tx.query_row("SELECT COUNT(*) FROM settings_import", [], |row| row.get(0)))?;
        if imports != 1 {
            return Err(SettingsError::CorruptAuthority);
        }
    } else {
        if !fresh {
            return Err(SettingsError::CorruptAuthority);
        }
        let preferences = match legacy {
            Some(bytes) => parse_legacy(bytes)?,
            None => defaults,
        };
        let snapshot = SettingsSnapshot {
            schema_version: VERSION,
            revision: 0,
            preferences,
            field_revisions: FieldRevisions::default(),
        };
        database(tx.execute(
            "INSERT INTO settings_state VALUES(1,?1)",
            [encode(&snapshot)?],
        ))?;
        database(tx.execute(
            "INSERT INTO settings_import VALUES(1,?1)",
            [legacy.map(|bytes| Digest::from_bytes(bytes).to_string())],
        ))?;
    }
    database(tx.commit())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyPreferences {
    schema_version: u8,
    preferences: ConsolePreferences,
}

fn parse_legacy(bytes: &[u8]) -> Result<ConsolePreferences, SettingsError> {
    if bytes.len() > MAX_LEGACY_BYTES {
        return Err(SettingsError::InvalidInput);
    }
    let legacy: LegacyPreferences =
        serde_json::from_slice(bytes).map_err(|_| SettingsError::InvalidInput)?;
    if ![1, 2].contains(&legacy.schema_version) {
        return Err(SettingsError::InvalidInput);
    }
    legacy
        .preferences
        .validate()
        .map_err(|_| SettingsError::InvalidInput)?;
    Ok(legacy.preferences)
}

pub(super) fn load(connection: &Connection) -> Result<SettingsSnapshot, SettingsError> {
    let payload: String = database(connection.query_row(
        "SELECT payload FROM settings_state WHERE singleton=1",
        [],
        |row| row.get(0),
    ))?;
    let snapshot: SettingsSnapshot = decode(&payload)?;
    snapshot.validate()?;
    let (commands, events, maximum): (u64, u64, u64) = database(connection.query_row(
        "SELECT (SELECT COUNT(*) FROM settings_commands),COUNT(*),COALESCE(MAX(revision),0) FROM settings_events",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ))?;
    if commands != snapshot.revision || events != snapshot.revision || maximum != snapshot.revision
    {
        return Err(SettingsError::CorruptAuthority);
    }
    if snapshot.revision > 0 {
        let (event, intent, receipt): (String, String, String) = database(connection.query_row(
            "SELECT e.payload,c.intent,c.receipt FROM settings_events e JOIN settings_commands c ON c.command_id=e.command_id WHERE e.revision=?1",
            [snapshot.revision], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ))?;
        let event: SettingsEvent = decode(&event)?;
        let command: SettingsCommand = decode(&intent)?;
        let receipt: SettingsReceipt = decode(&receipt)?;
        validate_receipt(&command, &receipt)?;
        if receipt.snapshot != snapshot
            || event.snapshot != snapshot
            || event.revision != snapshot.revision
            || event.schema_version != VERSION
            || event.command_id != command.command_id
        {
            return Err(SettingsError::CorruptAuthority);
        }
    }
    Ok(snapshot)
}

fn next_revision(value: u64) -> Result<u64, SettingsError> {
    value
        .checked_add(1)
        .filter(|next| *next <= MAX_SAFE_REVISION)
        .ok_or(SettingsError::Overflow)
}

fn merge(snapshot: &mut SettingsSnapshot, command: &SettingsCommand) -> Result<(), SettingsError> {
    macro_rules! field {
        ($name:ident) => {
            if let Some(value) = command.patch.$name {
                if command.expected_field_revisions.$name != Some(snapshot.field_revisions.$name) {
                    return Err(SettingsError::Conflict);
                }
                snapshot.preferences.$name = value;
                snapshot.field_revisions.$name = next_revision(snapshot.field_revisions.$name)?;
            }
        };
    }
    field!(motion);
    field!(sound);
    field!(theme);
    field!(font_scale);
    field!(language);
    snapshot.revision = next_revision(snapshot.revision)?;
    snapshot.validate()
}

fn validate_receipt(
    command: &SettingsCommand,
    receipt: &SettingsReceipt,
) -> Result<(), SettingsError> {
    receipt.snapshot.validate()?;
    if receipt.schema_version != VERSION
        || receipt.command_id != command.command_id
        || receipt.idempotency_key != command.idempotency_key
        || receipt.intent_digest != command.digest()?
        || !valid_identity(&receipt.committed_at)
        || receipt.snapshot.revision == 0
    {
        return Err(SettingsError::CorruptAuthority);
    }
    macro_rules! field {
        ($name:ident) => {
            if let Some(value) = command.patch.$name {
                if receipt.snapshot.preferences.$name != value
                    || receipt.snapshot.field_revisions.$name
                        != next_revision(
                            command
                                .expected_field_revisions
                                .$name
                                .ok_or(SettingsError::CorruptAuthority)?,
                        )
                        .map_err(|_| SettingsError::CorruptAuthority)?
                {
                    return Err(SettingsError::CorruptAuthority);
                }
            }
        };
    }
    field!(motion);
    field!(sound);
    field!(theme);
    field!(font_scale);
    field!(language);
    Ok(())
}

pub(super) fn apply(
    connection: &mut Connection,
    command: &SettingsCommand,
    committed_at: &str,
) -> Result<SettingsReceipt, SettingsError> {
    command.validate()?;
    if !valid_identity(committed_at) {
        return Err(SettingsError::InvalidInput);
    }
    let digest = command.digest()?;
    let tx = database(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
    let mut statement = database(tx.prepare("SELECT DISTINCT c.intent_digest,c.intent,c.receipt FROM settings_commands c LEFT JOIN settings_aliases a ON a.original_command_id=c.command_id WHERE c.command_id=?1 OR a.command_id=?1 OR c.idempotency_key=?2"))?;
    let matches: Vec<(String, String, String)> = database(statement.query_map(
        params![command.command_id, command.idempotency_key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ))?
    .collect::<Result<_, _>>()
    .map_err(|_| SettingsError::Unavailable)?;
    drop(statement);
    if !matches.is_empty() {
        if matches.len() != 1 {
            return Err(SettingsError::IdempotencyConflict);
        }
        let (stored_digest, intent, payload) = &matches[0];
        let original: SettingsCommand = decode(intent)?;
        original
            .validate()
            .map_err(|_| SettingsError::CorruptAuthority)?;
        if original.digest()?.as_str() != stored_digest {
            return Err(SettingsError::CorruptAuthority);
        }
        if original.idempotency_key != command.idempotency_key || stored_digest != digest.as_str() {
            return Err(SettingsError::IdempotencyConflict);
        }
        let receipt: SettingsReceipt = decode(payload)?;
        validate_receipt(&original, &receipt)?;
        database(tx.execute(
            "INSERT INTO settings_aliases VALUES(?1,?2) ON CONFLICT(command_id) DO NOTHING",
            params![command.command_id, original.command_id],
        ))?;
        database(tx.commit())?;
        return Ok(receipt);
    }
    let mut snapshot = load(&tx)?;
    merge(&mut snapshot, command)?;
    let receipt = SettingsReceipt {
        schema_version: VERSION,
        command_id: command.command_id.clone(),
        idempotency_key: command.idempotency_key.clone(),
        intent_digest: digest.clone(),
        committed_at: committed_at.into(),
        snapshot: snapshot.clone(),
    };
    let event = SettingsEvent {
        schema_version: VERSION,
        revision: snapshot.revision,
        command_id: command.command_id.clone(),
        snapshot,
    };
    database(tx.execute(
        "INSERT INTO settings_commands VALUES(?1,?2,?3,?4,?5)",
        params![
            command.command_id,
            command.idempotency_key,
            digest.as_str(),
            encode(command)?,
            encode(&receipt)?
        ],
    ))?;
    database(tx.execute(
        "INSERT INTO settings_aliases VALUES(?1,?1)",
        [&command.command_id],
    ))?;
    database(tx.execute(
        "UPDATE settings_state SET payload=?1 WHERE singleton=1",
        [encode(&receipt.snapshot)?],
    ))?;
    database(tx.execute(
        "INSERT INTO settings_events(revision,command_id,payload) VALUES(?1,?2,?3)",
        params![event.revision, event.command_id, encode(&event)?],
    ))?;
    database(tx.commit())?;
    Ok(receipt)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsEventPage {
    pub schema_version: u8,
    pub events: Vec<SettingsEvent>,
    pub has_more: bool,
}

pub(super) fn pending_events(
    connection: &mut Connection,
    limit: u16,
) -> Result<SettingsEventPage, SettingsError> {
    if !(1..=100).contains(&limit) {
        return Err(SettingsError::InvalidInput);
    }
    let tx = database(connection.transaction_with_behavior(TransactionBehavior::Deferred))?;
    let mut statement = database(tx.prepare("SELECT revision,command_id,payload FROM settings_events WHERE delivered=0 ORDER BY revision LIMIT ?1"))?;
    let rows = database(statement.query_map([u32::from(limit) + 1], |row| {
        Ok((
            row.get::<_, u64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|_| SettingsError::Unavailable)?;
    let has_more = rows.len() > usize::from(limit);
    let mut events = Vec::with_capacity(rows.len());
    for (revision, command_id, payload) in rows.into_iter().take(usize::from(limit)) {
        let event: SettingsEvent = decode(&payload)?;
        event.snapshot.validate()?;
        if event.schema_version != VERSION
            || event.revision != revision
            || event.snapshot.revision != revision
            || event.command_id != command_id
        {
            return Err(SettingsError::CorruptAuthority);
        }
        let (intent, receipt): (String, String) = database(tx.query_row(
            "SELECT intent,receipt FROM settings_commands WHERE command_id=?1",
            [&command_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ))?;
        let command: SettingsCommand = decode(&intent)?;
        let receipt: SettingsReceipt = decode(&receipt)?;
        validate_receipt(&command, &receipt)?;
        if event.snapshot != receipt.snapshot {
            return Err(SettingsError::CorruptAuthority);
        }
        events.push(event);
    }
    Ok(SettingsEventPage {
        schema_version: VERSION,
        events,
        has_more,
    })
}

pub(super) fn acknowledge_event(
    connection: &Connection,
    event: &SettingsEvent,
) -> Result<(), SettingsError> {
    let payload: String = database(connection.query_row(
        "SELECT payload FROM settings_events WHERE revision=?1 AND command_id=?2",
        params![event.revision, event.command_id],
        |row| row.get(0),
    ))?;
    if decode::<SettingsEvent>(&payload)? != *event {
        return Err(SettingsError::CorruptAuthority);
    }
    database(connection.execute(
        "UPDATE settings_events SET delivered=1 WHERE revision=?1 AND command_id=?2",
        params![event.revision, event.command_id],
    ))?;
    Ok(())
}

#[cfg(test)]
fn initialize(connection: &mut Connection, legacy: Option<&[u8]>) -> Result<(), SettingsError> {
    initialize_with_defaults(
        connection,
        legacy,
        ConsolePreferences {
            motion: super::MotionPreference::Full,
            sound: false,
            theme: super::ThemePreference::Command,
            font_scale: 100,
            language: super::UiLanguage::Ko,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialized() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize(&mut connection, None).unwrap();
        connection
    }

    fn command(
        id: &str,
        patch: PreferencePatch,
        expected: ExpectedFieldRevisions,
    ) -> SettingsCommand {
        SettingsCommand {
            schema_version: VERSION,
            command_id: id.into(),
            idempotency_key: format!("key-{id}"),
            target: "console_preferences".into(),
            patch,
            expected_field_revisions: expected,
        }
    }

    fn theme(id: &str) -> SettingsCommand {
        command(
            id,
            PreferencePatch {
                theme: Some(super::super::ThemePreference::Clear),
                ..Default::default()
            },
            ExpectedFieldRevisions {
                theme: Some(0),
                ..Default::default()
            },
        )
    }

    #[test]
    fn distinct_fields_compose_in_both_commit_orders_and_stale_same_field_fails() {
        let font = command(
            "font",
            PreferencePatch {
                font_scale: Some(150),
                ..Default::default()
            },
            ExpectedFieldRevisions {
                font_scale: Some(0),
                ..Default::default()
            },
        );
        for order in [
            [theme("theme"), font.clone()],
            [font.clone(), theme("theme")],
        ] {
            let mut connection = initialized();
            for intent in order {
                apply(&mut connection, &intent, "2026-10-02T10:00:00Z").unwrap();
            }
            let snapshot = load(&connection).unwrap();
            assert_eq!(
                snapshot.preferences.theme,
                super::super::ThemePreference::Clear
            );
            assert_eq!(snapshot.preferences.font_scale, 150);
            assert_eq!(snapshot.revision, 2);
            assert_eq!(
                apply(&mut connection, &theme("stale"), "now"),
                Err(SettingsError::Conflict)
            );
            assert_eq!(load(&connection).unwrap(), snapshot);
            assert_eq!(
                pending_events(&mut connection, 100).unwrap().events.len(),
                2
            );
        }
    }

    #[test]
    fn original_receipt_replays_without_reapplying_state_and_collision_rejects() {
        let mut connection = initialized();
        let first = theme("first");
        let receipt = apply(&mut connection, &first, "original-time").unwrap();
        let next = command(
            "next",
            PreferencePatch {
                theme: Some(super::super::ThemePreference::Command),
                ..Default::default()
            },
            ExpectedFieldRevisions {
                theme: Some(1),
                ..Default::default()
            },
        );
        apply(&mut connection, &next, "later-time").unwrap();
        assert_eq!(
            apply(&mut connection, &first, "replay-time").unwrap(),
            receipt
        );
        assert_eq!(
            load(&connection).unwrap().preferences.theme,
            super::super::ThemePreference::Command
        );
        let mut altered = first.clone();
        altered.patch.theme = Some(super::super::ThemePreference::Command);
        assert_eq!(
            apply(&mut connection, &altered, "time"),
            Err(SettingsError::IdempotencyConflict)
        );
        altered = first.clone();
        altered.command_id = "alias".into();
        assert_eq!(apply(&mut connection, &altered, "time").unwrap(), receipt);
        altered.idempotency_key = "different-key".into();
        assert_eq!(
            apply(&mut connection, &altered, "time"),
            Err(SettingsError::IdempotencyConflict)
        );
    }

    #[test]
    fn event_failure_rolls_back_state_receipt_and_event_and_notification_is_separate() {
        let mut connection = initialized();
        let before = load(&connection).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON settings_events BEGIN SELECT RAISE(ABORT,'controlled'); END;").unwrap();
        assert_eq!(
            apply(&mut connection, &theme("rollback"), "time"),
            Err(SettingsError::Unavailable)
        );
        assert_eq!(load(&connection).unwrap(), before);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM settings_commands", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        connection.execute_batch("DROP TRIGGER fail_event").unwrap();
        let receipt = apply(&mut connection, &theme("commit"), "time").unwrap();
        let events = pending_events(&mut connection, 100).unwrap().events;
        assert_eq!(events[0].snapshot, receipt.snapshot);
        // Failed notification leaves durable delivery work; it cannot undo the receipt.
        assert_eq!(pending_events(&mut connection, 100).unwrap().events, events);
        acknowledge_event(&connection, &events[0]).unwrap();
        assert!(
            pending_events(&mut connection, 100)
                .unwrap()
                .events
                .is_empty()
        );
        assert_eq!(
            apply(&mut connection, &theme("commit"), "again").unwrap(),
            receipt
        );
    }

    #[test]
    fn closed_patch_rejects_null_unknown_empty_and_invalid_fields() {
        for json in [
            r#"{"theme":null}"#,
            r#"{"secret":"canary"}"#,
            r#"{"fontScale":"150"}"#,
            r#"{"motion":"unsupported"}"#,
        ] {
            assert!(serde_json::from_str::<PreferencePatch>(json).is_err());
        }
        let mut connection = initialized();
        for patch in [
            PreferencePatch::default(),
            PreferencePatch {
                font_scale: Some(151),
                ..Default::default()
            },
        ] {
            let intent = command("invalid", patch, ExpectedFieldRevisions::default());
            assert_eq!(
                apply(&mut connection, &intent, "time"),
                Err(SettingsError::InvalidInput)
            );
        }
        let mut intent = theme("mismatch");
        intent.expected_field_revisions.sound = Some(0);
        assert_eq!(
            apply(&mut connection, &intent, "time"),
            Err(SettingsError::InvalidInput)
        );
        assert_eq!(load(&connection).unwrap().revision, 0);
    }

    #[test]
    fn legacy_import_is_once_only_and_corrupt_input_cannot_become_defaults() {
        for version in [1, 2] {
            let mut connection = Connection::open_in_memory().unwrap();
            let bytes = format!(
                r#"{{"schemaVersion":{version},"preferences":{{"motion":"reduced","sound":true,"theme":"clear"}}}}"#
            );
            initialize(&mut connection, Some(bytes.as_bytes())).unwrap();
            let snapshot = load(&connection).unwrap();
            assert_eq!(snapshot.preferences.font_scale, 100);
            initialize(
                &mut connection,
                Some(b"invalid ignored after authoritative import"),
            )
            .unwrap();
            assert_eq!(load(&connection).unwrap(), snapshot);
        }
        for bytes in [b"{}".as_slice(),b"private malformed canary".as_slice(),br#"{"schemaVersion":9,"preferences":{"motion":"full","sound":false,"theme":"command"}}"#.as_slice()] {
            let mut connection=Connection::open_in_memory().unwrap();
            assert_eq!(initialize(&mut connection,Some(bytes)),Err(SettingsError::InvalidInput));
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='settings_state'",[],|row|row.get::<_,u64>(0)).unwrap(),0);
        }
    }

    #[test]
    fn pending_outbox_over_one_page_drains_without_starvation() {
        let mut connection = initialized();
        for revision in 0..101 {
            let intent = command(
                &format!("sound-{revision}"),
                PreferencePatch {
                    sound: Some(revision % 2 == 0),
                    ..Default::default()
                },
                ExpectedFieldRevisions {
                    sound: Some(revision),
                    ..Default::default()
                },
            );
            apply(&mut connection, &intent, "time").unwrap();
        }
        let page = pending_events(&mut connection, 100).unwrap();
        assert!(page.has_more);
        assert_eq!(page.events.len(), 100);
        assert_eq!(page.events.first().unwrap().revision, 1);
        assert_eq!(page.events.last().unwrap().revision, 100);
        for event in &page.events {
            acknowledge_event(&connection, event).unwrap();
        }
        let last = pending_events(&mut connection, 100).unwrap();
        assert!(!last.has_more);
        assert_eq!(last.events.len(), 1);
        assert_eq!(last.events[0].revision, 101);
        acknowledge_event(&connection, &last.events[0]).unwrap();
        assert!(
            pending_events(&mut connection, 100)
                .unwrap()
                .events
                .is_empty()
        );
        assert_eq!(load(&connection).unwrap().revision, 101);
    }

    #[test]
    fn damaged_authority_never_reinitializes_beside_surviving_receipts() {
        for corruption in [
            "DELETE FROM settings_state",
            "DELETE FROM settings_import",
            "PRAGMA user_version=0",
            "DROP TRIGGER settings_alias_no_delete",
        ] {
            let mut connection = initialized();
            apply(&mut connection, &theme("original"), "time").unwrap();
            let receipt: String = connection
                .query_row("SELECT receipt FROM settings_commands", [], |row| {
                    row.get(0)
                })
                .unwrap();
            connection.execute_batch(corruption).unwrap();
            assert_eq!(
                initialize(&mut connection, None),
                Err(SettingsError::CorruptAuthority)
            );
            assert_eq!(
                connection
                    .query_row("SELECT receipt FROM settings_commands", [], |row| row
                        .get::<_, String>(0))
                    .unwrap(),
                receipt
            );
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM settings_events", [], |row| row
                        .get::<_, u64>(0))
                    .unwrap(),
                1
            );
            if corruption == "DELETE FROM settings_state" {
                assert_eq!(
                    connection
                        .query_row("SELECT COUNT(*) FROM settings_state", [], |row| row
                            .get::<_, u64>(0))
                        .unwrap(),
                    0
                );
            }
        }
    }

    struct OwnedDatabase(std::path::PathBuf);
    impl OwnedDatabase {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("magi-settings-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&directory).unwrap();
            Self(directory)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("settings.sqlite")
        }
    }
    impl Drop for OwnedDatabase {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn independent_connections_compose_and_original_alias_receipt_survives_reopen() {
        for font_first in [false, true] {
            let owned = OwnedDatabase::new();
            let mut first = Connection::open(owned.path()).unwrap();
            initialize(&mut first, None).unwrap();
            let mut second = Connection::open(owned.path()).unwrap();
            initialize(&mut second, None).unwrap();
            let theme = theme("theme");
            let font = command(
                "font",
                PreferencePatch {
                    font_scale: Some(150),
                    ..Default::default()
                },
                ExpectedFieldRevisions {
                    font_scale: Some(0),
                    ..Default::default()
                },
            );
            let receipt = if font_first {
                apply(&mut second, &font, "font-time").unwrap();
                apply(&mut first, &theme, "theme-time").unwrap()
            } else {
                let result = apply(&mut first, &theme, "theme-time").unwrap();
                apply(&mut second, &font, "font-time").unwrap();
                result
            };
            drop(first);
            drop(second);
            let mut reopened = Connection::open(owned.path()).unwrap();
            initialize(
                &mut reopened,
                Some(b"cannot override authoritative database"),
            )
            .unwrap();
            let snapshot = load(&reopened).unwrap();
            assert_eq!(snapshot.preferences.font_scale, 150);
            assert_eq!(
                snapshot.preferences.theme,
                super::super::ThemePreference::Clear
            );
            let mut alias = theme.clone();
            alias.command_id = "alias".into();
            assert_eq!(
                apply(&mut reopened, &alias, "replay-time").unwrap(),
                receipt
            );
            assert_eq!(load(&reopened).unwrap(), snapshot);
            assert_eq!(pending_events(&mut reopened, 100).unwrap().events.len(), 2);
            drop(reopened);
        }
    }

    #[test]
    fn concurrent_distinct_field_writers_preserve_both_intents() {
        let owned = OwnedDatabase::new();
        let mut initial = Connection::open(owned.path()).unwrap();
        initialize(&mut initial, None).unwrap();
        drop(initial);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let intents = [
            theme("theme"),
            command(
                "font",
                PreferencePatch {
                    font_scale: Some(150),
                    ..Default::default()
                },
                ExpectedFieldRevisions {
                    font_scale: Some(0),
                    ..Default::default()
                },
            ),
        ];
        let workers: Vec<_> = intents
            .into_iter()
            .map(|intent| {
                let path = owned.path();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let prepared = (|| {
                        let mut connection =
                            Connection::open(path).map_err(|_| SettingsError::Unavailable)?;
                        database(connection.busy_timeout(std::time::Duration::from_secs(5)))?;
                        initialize(&mut connection, None)?;
                        Ok::<_, SettingsError>(connection)
                    })();
                    barrier.wait();
                    let mut connection = prepared.unwrap();
                    apply(&mut connection, &intent, "time").unwrap()
                })
            })
            .collect();
        barrier.wait();
        let joined: Vec<_> = workers.into_iter().map(|worker| worker.join()).collect();
        let mut revisions: Vec<_> = joined
            .into_iter()
            .map(|result| result.unwrap().snapshot.revision)
            .collect();
        revisions.sort();
        assert_eq!(revisions, vec![1, 2]);
        let connection = Connection::open(owned.path()).unwrap();
        let snapshot = load(&connection).unwrap();
        assert_eq!(snapshot.preferences.font_scale, 150);
        assert_eq!(
            snapshot.preferences.theme,
            super::super::ThemePreference::Clear
        );
        drop(connection);
    }

    #[test]
    fn valid_but_rolled_back_snapshot_cannot_replace_latest_committed_authority() {
        let mut connection = initialized();
        let original = load(&connection).unwrap();
        apply(&mut connection, &theme("committed"), "time").unwrap();
        connection
            .execute(
                "UPDATE settings_state SET payload=?1",
                [encode(&original).unwrap()],
            )
            .unwrap();
        assert_eq!(
            initialize(&mut connection, None),
            Err(SettingsError::CorruptAuthority)
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM settings_commands", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM settings_events", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn revisions_remain_exactly_representable_in_public_json() {
        assert_eq!(next_revision(MAX_SAFE_REVISION - 1), Ok(MAX_SAFE_REVISION));
        assert_eq!(
            next_revision(MAX_SAFE_REVISION),
            Err(SettingsError::Overflow)
        );
        let connection = initialized();
        let mut snapshot = load(&connection).unwrap();
        snapshot.revision = MAX_SAFE_REVISION - 1;
        snapshot.field_revisions.theme = MAX_SAFE_REVISION - 1;
        let mut input = theme("limit");
        input.expected_field_revisions.theme = Some(MAX_SAFE_REVISION - 1);
        merge(&mut snapshot, &input).unwrap();
        assert_eq!(snapshot.revision, MAX_SAFE_REVISION);
        input.expected_field_revisions.theme = Some(MAX_SAFE_REVISION);
        assert_eq!(merge(&mut snapshot, &input), Err(SettingsError::Overflow));
        snapshot.revision = MAX_SAFE_REVISION + 1;
        assert_eq!(snapshot.validate(), Err(SettingsError::CorruptAuthority));
        input.expected_field_revisions.theme = Some(MAX_SAFE_REVISION + 1);
        assert_eq!(input.validate(), Err(SettingsError::InvalidInput));
    }

    #[test]
    fn sqlite_engine_identity_is_recorded_from_the_linked_engine() {
        let connection = initialized();
        let (version, source_id): (String, String) = connection
            .query_row("SELECT sqlite_version(), sqlite_source_id()", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let options = connection
            .prepare("PRAGMA compile_options")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!version.is_empty() && !source_id.is_empty());
        assert!(options.iter().any(|option| option == "THREADSAFE=1"));
        println!("SQLite version={version}; source_id={source_id}; compile_options={options:?}");
    }
}
