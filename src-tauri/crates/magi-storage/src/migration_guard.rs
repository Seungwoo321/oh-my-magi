use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationJournal {
    format: String,
    from_schema: u32,
    target_schema: u32,
    backup_directory: String,
    database_digest: Digest,
    objects: Vec<ContentObjectRef>,
    status: String,
}

pub(super) struct MigrationGuard {
    directory: PathBuf,
    journal: MigrationJournal,
}

impl MigrationGuard {
    pub(super) fn prepare(
        connection: &Connection,
        data_root: &Path,
    ) -> Result<Option<Self>, StorageError> {
        let from_schema: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        reconcile_journals(data_root, from_schema)?;
        if from_schema >= SCHEMA_VERSION {
            return Ok(None);
        }
        let tables: u64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if from_schema == 0 && tables == 0 {
            return Ok(None);
        }
        let directory = data_root
            .join("state")
            .join("migration-backups")
            .join(Uuid::new_v4().to_string());
        let objects_root = data_root.join("objects");
        let mut paths = Vec::new();
        collect_objects(&objects_root, &mut paths)?;
        let pages: u64 = connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let page_size: u64 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let database_bytes = pages.saturating_mul(page_size);
        let object_bytes = paths.iter().try_fold(0u64, |sum, path| {
            fs::metadata(path).map(|metadata| sum.saturating_add(metadata.len()))
        })?;
        let required = database_bytes
            .saturating_mul(3)
            .saturating_add(object_bytes)
            .saturating_add(64 * 1024 * 1024);
        if fs2::available_space(data_root)? < required {
            return Err(StorageError::Integrity(
                "Insufficient free space for a verified pre-migration backup.".into(),
            ));
        }
        create_private_dir(
            directory
                .parent()
                .ok_or_else(|| StorageError::Integrity("Invalid migration backup root.".into()))?,
        )?;
        create_private_dir(&directory)?;
        let snapshot = directory.join("magi.sqlite");
        connection.execute(
            "VACUUM INTO ?1",
            [snapshot
                .to_str()
                .ok_or_else(|| StorageError::Integrity("Invalid migration backup path.".into()))?],
        )?;
        set_private_file_permissions(&snapshot)?;
        File::open(&snapshot)?.sync_all()?;
        let backup =
            Connection::open_with_flags(&snapshot, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let integrity: String = backup.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(StorageError::Integrity(
                "The pre-migration database snapshot failed integrity validation.".into(),
            ));
        }
        let database_digest =
            Digest::from_bytes(&read_bounded_file(&snapshot, 1024 * 1024 * 1024)?);
        let unavailable: bool = backup.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='unavailable_objects')",
            [],
            |row| row.get(0),
        )?;
        let inventory_sql = if unavailable {
            "SELECT digest FROM content_objects WHERE digest NOT IN (SELECT digest FROM unavailable_objects) ORDER BY digest"
        } else {
            "SELECT digest FROM content_objects ORDER BY digest"
        };
        let mut query = backup.prepare(inventory_sql)?;
        let inventory = query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut objects = Vec::new();
        for hex in inventory {
            let digest = Digest::from_hex(hex)?;
            let path = objects_root
                .join(&digest.as_str()[..2])
                .join(&digest.as_str()[2..]);
            reject_symlink_if_present(&path)?;
            let relative = path
                .strip_prefix(&objects_root)
                .map_err(|_| StorageError::Integrity("Invalid migration object path.".into()))?;
            let bytes = read_bounded_file(&path, MAX_OBJECT_BYTES)?;
            let digest = Digest::from_bytes(&bytes);
            let components = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>();
            if components.len() != 2
                || format!("{}{}", components[0], components[1]) != digest.as_str()
            {
                return Err(StorageError::Integrity(
                    "A pre-migration source object failed its digest check.".into(),
                ));
            }
            let target = directory.join("objects").join(relative);
            create_private_dir(target.parent().ok_or_else(|| {
                StorageError::Integrity("Invalid migration object parent.".into())
            })?)?;
            write_new_file(&target, &bytes)?;
            File::open(target.parent().unwrap())?.sync_all()?;
            objects.push(ContentObjectRef {
                digest,
                byte_length: bytes.len() as u64,
            });
        }
        if directory.join("objects").exists() {
            File::open(directory.join("objects"))?.sync_all()?;
        }
        let store_id: String = backup.query_row(
            "SELECT value FROM store_meta WHERE key='store_id'",
            [],
            |row| row.get(0),
        )?;
        let generation: String = backup.query_row(
            "SELECT value FROM store_meta WHERE key='store_generation'",
            [],
            |row| row.get(0),
        )?;
        let event_high_water: u64 = backup.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM run_events",
            [],
            |row| row.get(0),
        )?;
        let manifest = BackupManifest {
            format: "magi-backup".into(),
            schema_version: from_schema,
            store_id,
            generation: generation
                .parse()
                .map_err(|_| StorageError::Integrity("Invalid pre-migration generation.".into()))?,
            event_high_water,
            database_digest: database_digest.clone(),
            objects: objects.clone(),
            contains_private_sources: true,
        };
        let manifest_bytes = serde_json::to_vec(&manifest)?;
        write_new_file(&directory.join("manifest.json"), &manifest_bytes)?;
        let journal = MigrationJournal {
            format: "magi-schema-migration".into(),
            from_schema,
            target_schema: SCHEMA_VERSION,
            backup_directory: directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            database_digest,
            objects,
            status: "prepared".into(),
        };
        let guard = Self { directory, journal };
        guard.persist("prepared")?;
        write_new_file(
            &guard.directory.join("COMPLETE"),
            Digest::from_bytes(&manifest_bytes).as_str().as_bytes(),
        )?;
        File::open(&guard.directory)?.sync_all()?;
        File::open(guard.directory.parent().unwrap())?.sync_all()?;
        Ok(Some(guard))
    }

    pub(super) fn finish(&self, succeeded: bool) -> Result<(), StorageError> {
        self.persist(if succeeded { "completed" } else { "failed" })
    }

    fn persist(&self, status: &str) -> Result<(), StorageError> {
        let mut value = serde_json::to_value(&self.journal)?;
        value["status"] = serde_json::json!(status);
        let bytes = serde_json::to_vec(&value)?;
        let path = self.directory.join(format!("journal-{status}.json"));
        write_new_file(&path, &bytes)?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    set_private_file_permissions(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn reconcile_journals(root: &Path, actual_schema: u32) -> Result<(), StorageError> {
    let backups = root.join("state/migration-backups");
    if !backups.exists() {
        return Ok(());
    }
    reject_symlink_if_present(&backups)?;
    for entry in fs::read_dir(backups)? {
        let directory = entry?.path();
        reject_symlink_if_present(&directory)?;
        let prepared = directory.join("journal-prepared.json");
        if !prepared.exists()
            || !directory.join("COMPLETE").exists()
            || directory.join("journal-completed.json").exists()
            || directory.join("journal-failed.json").exists()
        {
            continue;
        }
        let journal: MigrationJournal =
            serde_json::from_slice(&read_bounded_file(&prepared, 10 * 1024 * 1024)?)?;
        let manifest = read_bounded_file(&directory.join("manifest.json"), 10 * 1024 * 1024)?;
        if read_bounded_file(&directory.join("COMPLETE"), 64)?
            != Digest::from_bytes(&manifest).as_str().as_bytes()
            || Digest::from_bytes(&read_bounded_file(
                &directory.join("magi.sqlite"),
                1024 * 1024 * 1024,
            )?) != journal.database_digest
        {
            return Err(StorageError::Integrity(
                "An interrupted migration backup failed validation.".into(),
            ));
        }
        if journal.target_schema != SCHEMA_VERSION
            || ![journal.from_schema, journal.target_schema].contains(&actual_schema)
        {
            return Err(StorageError::Integrity(
                "An interrupted migration has an incompatible target schema.".into(),
            ));
        }
        MigrationGuard { directory, journal }.finish(actual_schema == SCHEMA_VERSION)?;
    }
    Ok(())
}

fn collect_objects(root: &Path, result: &mut Vec<PathBuf>) -> Result<(), StorageError> {
    reject_symlink_if_present(root)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(StorageError::Integrity(
                "A migration backup object is a symbolic link.".into(),
            ));
        }
        if metadata.is_dir() {
            collect_objects(&path, result)?;
        } else if metadata.is_file() {
            result.push(path);
        } else {
            return Err(StorageError::Integrity(
                "A migration backup object is not a regular file.".into(),
            ));
        }
    }
    result.sort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pre_migration_snapshot_preserves_main_ledger_and_records_actual_target() {
        let root = std::env::temp_dir().join(format!("magi-migration-{}", Uuid::new_v4()));
        create_private_dir(&root).unwrap();
        create_private_dir(&root.join("state")).unwrap();
        create_private_dir(&root.join("objects")).unwrap();
        {
            let connection = Connection::open(root.join("state/magi.sqlite")).unwrap();
            connection.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,checksum TEXT NOT NULL,applied_at TEXT NOT NULL)").unwrap();
            for version in 1..=6 {
                let sql = migration_sql(version).unwrap();
                connection.execute_batch(sql).unwrap();
                connection
                    .execute(
                        "INSERT INTO schema_migrations VALUES (?1,?2,'fixture')",
                        params![version, Digest::from_bytes(sql.as_bytes()).as_str()],
                    )
                    .unwrap();
            }
            connection.execute_batch("PRAGMA user_version=6").unwrap();
            for (key, value) in [
                ("store_id", "fixture"),
                ("store_generation", "1"),
                ("schema_version", "6"),
            ] {
                connection
                    .execute("INSERT INTO store_meta VALUES(?1,?2)", params![key, value])
                    .unwrap();
            }
            let guard = MigrationGuard::prepare(&connection, &root)
                .unwrap()
                .unwrap();
            assert_eq!(guard.journal.from_schema, 6);
            assert_eq!(guard.journal.target_schema, SCHEMA_VERSION);
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                6
            );
            let snapshot = Connection::open_with_flags(
                guard.directory.join("magi.sqlite"),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            assert_eq!(
                snapshot
                    .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r
                        .get::<_, u64>(0))
                    .unwrap(),
                6
            );
            guard.finish(false).unwrap();
            assert!(guard.directory.join("journal-failed.json").exists());
            drop(snapshot);
            drop(connection);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
