//! Portable Helio backup archive: a private tar.gz containing the database
//! snapshot and the existing Skills archive.

use crate::utils::secure_fs::{ensure_private_dir, secure_export_file};
use crate::utils::skills_backup::{export_skills, SkillsExportResult};
use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;
const MANIFEST_NAME: &str = "manifest.json";
const DATABASE_NAME: &str = "database.sqlite";
const SKILLS_NAME: &str = "skills.tar.gz";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_COMPONENT_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    database: Component,
    skills: Component,
}

#[derive(Debug, Serialize, Deserialize)]
struct Component {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PortableBackupExportResult {
    pub path: String,
    pub skills: SkillsExportResult,
}

/// Holds validated component paths in a private staging directory.
pub struct PortableBackupContents {
    _staging: tempfile::TempDir,
    pub database_path: PathBuf,
    pub skills_path: PathBuf,
}

pub fn export_portable_backup(
    home: &Path,
    database_path: &Path,
    output_path: &Path,
) -> Result<PortableBackupExportResult> {
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create export directory {}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".helio-portable-export-")
        .tempdir_in(parent)
        .context("Failed to create portable backup staging directory")?;
    ensure_private_dir(staging.path())?;

    let database = staging.path().join(DATABASE_NAME);
    crate::db::Database::snapshot_to(database_path, &database)?;
    let skills = staging.path().join(SKILLS_NAME);
    let skills_result = export_skills(home, &skills)?;

    let manifest = Manifest {
        version: VERSION,
        database: Component {
            path: DATABASE_NAME.to_string(),
            sha256: sha256_file(&database)?,
        },
        skills: Component {
            path: SKILLS_NAME.to_string(),
            sha256: sha256_file(&skills)?,
        },
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;

    let tmp = output_path.with_file_name(format!(
        ".{}.tmp-{}",
        output_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("helio-backup"),
        uuid::Uuid::new_v4()
    ));
    let result: Result<()> = (|| {
        let file = fs::File::create(&tmp)
            .with_context(|| format!("Failed to create {}", tmp.display()))?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        append_file(&mut archive, DATABASE_NAME, &database)?;
        append_file(&mut archive, SKILLS_NAME, &skills)?;
        append_bytes(&mut archive, MANIFEST_NAME, &manifest_bytes)?;
        archive
            .finish()
            .context("Failed to finalize portable archive")?;
        archive
            .into_inner()
            .context("Failed to unwrap portable archive encoder")?
            .finish()
            .context("Failed to finalize portable archive gzip stream")?;
        secure_export_file(&tmp)?;
        fs::rename(&tmp, output_path).with_context(|| {
            format!(
                "Failed to move portable archive to {}",
                output_path.display()
            )
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;

    Ok(PortableBackupExportResult {
        path: output_path.to_string_lossy().to_string(),
        skills: skills_result,
    })
}

/// Extract and validate a portable archive into a private temporary directory.
/// The caller owns the returned contents and may validate/import the database.
pub fn extract_portable_backup(archive_path: &Path) -> Result<PortableBackupContents> {
    let staging = tempfile::Builder::new()
        .prefix(".helio-portable-import-")
        .tempdir()
        .context("Failed to create portable import staging directory")?;
    ensure_private_dir(staging.path())?;

    let file = fs::File::open(archive_path)
        .with_context(|| format!("Failed to open {}", archive_path.display()))?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    let mut manifest: Option<Manifest> = None;
    let mut database_seen = false;
    let mut skills_seen = false;

    for entry in archive
        .entries()
        .context("Backup is not a valid tar.gz archive")?
    {
        let mut entry = entry.context("Failed to read portable archive entry")?;
        let header = entry.header();
        if header.entry_type().is_symlink() || header.entry_type().is_hard_link() {
            anyhow::bail!("Portable archive contains a link entry");
        }
        if !header.entry_type().is_file() {
            anyhow::bail!("Portable archive contains a non-file entry");
        }
        let path = entry
            .path()
            .context("Portable archive contains an invalid path")?
            .into_owned();
        let name = path
            .to_str()
            .ok_or_else(|| anyhow!("Portable archive path is not UTF-8"))?;
        let size = header
            .size()
            .context("Portable archive entry has invalid size")?;

        match name {
            MANIFEST_NAME => {
                if manifest.is_some() || size > MAX_MANIFEST_BYTES {
                    anyhow::bail!("Portable archive manifest is invalid");
                }
                let mut bytes = Vec::with_capacity(size as usize);
                entry
                    .take(MAX_MANIFEST_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .context("Failed to read portable archive manifest")?;
                manifest = Some(
                    serde_json::from_slice(&bytes)
                        .context("Portable archive manifest is invalid JSON")?,
                );
            }
            DATABASE_NAME | SKILLS_NAME => {
                if size > MAX_COMPONENT_BYTES {
                    anyhow::bail!("Portable archive component is too large: {name}");
                }
                let destination = staging.path().join(name);
                match name {
                    DATABASE_NAME if database_seen => {
                        anyhow::bail!("Portable archive has duplicate database")
                    }
                    SKILLS_NAME if skills_seen => {
                        anyhow::bail!("Portable archive has duplicate Skills archive")
                    }
                    DATABASE_NAME => database_seen = true,
                    SKILLS_NAME => skills_seen = true,
                    _ => unreachable!(),
                }
                copy_entry(&mut entry, &destination)?;
            }
            _ => anyhow::bail!("Portable archive contains an unexpected entry: {name}"),
        }
    }

    let manifest = manifest.ok_or_else(|| anyhow!("Portable archive is missing manifest.json"))?;
    if manifest.version != VERSION
        || manifest.database.path != DATABASE_NAME
        || manifest.skills.path != SKILLS_NAME
        || !database_seen
        || !skills_seen
    {
        anyhow::bail!("Portable archive manifest does not match required components");
    }

    let database_path = staging.path().join(DATABASE_NAME);
    let skills_path = staging.path().join(SKILLS_NAME);
    if manifest.database.sha256 != sha256_file(&database_path)?
        || manifest.skills.sha256 != sha256_file(&skills_path)?
    {
        anyhow::bail!("Portable archive component integrity check failed");
    }

    Ok(PortableBackupContents {
        _staging: staging,
        database_path,
        skills_path,
    })
}

fn append_file<W: Write>(archive: &mut tar::Builder<W>, name: &str, path: &Path) -> Result<()> {
    let mut file =
        fs::File::open(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut header = tar::Header::new_gnu();
    header.set_size(file.metadata()?.len());
    header.set_mode(0o600);
    header.set_mtime(chrono::Utc::now().timestamp() as u64);
    header.set_cksum();
    archive
        .append_data(&mut header, name, &mut file)
        .with_context(|| format!("Failed to add {name} to portable archive"))?;
    Ok(())
}

fn append_bytes<W: Write>(archive: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(chrono::Utc::now().timestamp() as u64);
    header.set_cksum();
    archive
        .append_data(&mut header, name, bytes)
        .with_context(|| format!("Failed to add {name} to portable archive"))?;
    Ok(())
}

fn copy_entry<R: Read>(entry: &mut R, destination: &Path) -> Result<()> {
    let mut output = fs::File::create(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;
    io::copy(entry, &mut output)
        .with_context(|| format!("Failed to extract {}", destination.display()))?;
    output.sync_all()?;
    crate::utils::secure_fs::ensure_private_file(destination)?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("Failed to hash {}", path.display()))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::models::{ApiProfile, TargetApp};

    #[test]
    fn export_and_extract_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let home = dir.path().join("home");
        let skill = home.join(".codex/skills/demo/SKILL.md");
        fs::create_dir_all(skill.parent().unwrap())?;
        fs::write(&skill, "# demo")?;
        let db_path = dir.path().join("db.sqlite");
        let db = Database::open(&db_path)?;
        db.add_profile(&ApiProfile {
            name: "codex".into(),
            target_app: Some(TargetApp::Codex),
            api_url: "https://example.test/v1".into(),
            api_key: "secret".into(),
            ..Default::default()
        })?;
        let output = dir.path().join("portable.tar.gz");

        let exported = export_portable_backup(&home, &db_path, &output)?;
        assert_eq!(exported.skills.total, 1);
        let contents = extract_portable_backup(&output)?;
        Database::validate_import_candidate(&contents.database_path)?;
        assert!(contents.skills_path.exists());
        Ok(())
    }

    #[test]
    fn extract_rejects_hash_mismatch() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let archive_path = dir.path().join("invalid.tar.gz");
        let file = fs::File::create(&archive_path)?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        append_bytes(&mut archive, DATABASE_NAME, b"database")?;
        append_bytes(&mut archive, SKILLS_NAME, b"skills")?;
        let manifest = serde_json::to_vec(&Manifest {
            version: VERSION,
            database: Component {
                path: DATABASE_NAME.into(),
                sha256: "00".repeat(32),
            },
            skills: Component {
                path: SKILLS_NAME.into(),
                sha256: "00".repeat(32),
            },
        })?;
        append_bytes(&mut archive, MANIFEST_NAME, &manifest)?;
        archive.finish()?;
        archive.into_inner()?.finish()?;

        assert!(extract_portable_backup(&archive_path).is_err());
        Ok(())
    }
}
