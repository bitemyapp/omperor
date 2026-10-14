//! Project archives. Archiving bundles a project's sessions (their journals
//! and sidecars, their sub-agents' journals, the content blobs those
//! journals refer to, and their metrics), its registry rows, and the saved
//! reading state of each session into one zstd-compressed tarball in
//! Omperor's private `archives` directory, which nothing loads from. The
//! tarball is read back and checked before anything it holds is removed.
//! Folders, worktrees, and code are never archived or removed.
//!
//! Restoring puts everything back and removes the archive. Everything here
//! reads or writes the disk, so it runs on a worker.

use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use omp_journal::{blob::BlobStore, gc};
use serde::{Deserialize, Serialize};

use crate::{housekeeping, journal, metrics, perf, preferences};

const FORMAT: &str = "omperor-project-archive";
const VERSION: u32 = 1;
const ARCHIVE_EXTENSION: &str = "tar.zst";
const MANIFEST: &str = "manifest.json";
const METRICS: &str = "metrics.jsonl";
const SESSIONS: &str = "sessions";
const BLOBS: &str = "blobs";
/// Archives are written once and read rarely: favour size.
const ZSTD_LEVEL: i32 = 12;

/// What an archive holds, stored as its first entry and beside it as
/// `<id>.json` so the archive list never decompresses anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) format: String,
    pub(crate) version: u32,
    pub(crate) id: String,
    pub(crate) archived_at_ms: i64,
    pub(crate) project: preferences::ProjectRows,
    pub(crate) sessions: Vec<SessionEntry>,
    /// Sub-agent journals copied in that sessions outside the project also
    /// name, so they were left in place.
    #[serde(default)]
    pub(crate) shared_journals: Vec<String>,
    /// Worktrees the project's sessions ran in. They hold code and were left
    /// on disk.
    #[serde(default)]
    pub(crate) worktrees: Vec<PathBuf>,
    /// Bytes of session data before compression.
    pub(crate) bytes: u64,
    pub(crate) blobs: usize,
    /// The tarball's size; only the sidecar knows it.
    #[serde(default)]
    pub(crate) compressed_bytes: u64,
}

impl Manifest {
    pub(crate) fn session_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|session| session.journal.is_some())
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionEntry {
    pub(crate) id: String,
    pub(crate) title: String,
    /// Its journal's stem, when it had one.
    pub(crate) journal: Option<String>,
    /// The stems of the sub-agent journals archived with it.
    #[serde(default)]
    pub(crate) children: Vec<String>,
    /// Its saved reading state: scroll, pins, unsent draft.
    #[serde(default)]
    pub(crate) view: Option<serde_json::Value>,
    /// A draft's key, so it comes back as the same draft.
    #[serde(default)]
    pub(crate) draft_key: Option<u64>,
}

/// One session of the project being archived, as the main thread knows it.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) journal: bool,
    pub(crate) view: Option<serde_json::Value>,
    pub(crate) draft_key: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub(crate) archives_dir: PathBuf,
    pub(crate) sessions_dir: PathBuf,
    pub(crate) database: PathBuf,
    pub(crate) project_id: String,
    pub(crate) members: Vec<Member>,
    pub(crate) worktrees: Vec<PathBuf>,
    /// The folder to remember as active when the archived project held it.
    pub(crate) active_fallback: Option<PathBuf>,
    pub(crate) metrics: Option<metrics::Editor>,
}

/// An archive on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listing {
    pub(crate) manifest: Manifest,
    pub(crate) path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Archived {
    pub(crate) listing: Listing,
    /// What went wrong after the archive was safely written.
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct Restored {
    /// False when incomplete restoration or a cleanup error kept the backup.
    pub(crate) archive_removed: bool,
    pub(crate) manifest: Manifest,
    /// Folders another project took since; they stay with it.
    pub(crate) taken_directories: Vec<PathBuf>,
    pub(crate) warnings: Vec<String>,
}

fn sidecar_path(archives_dir: &Path, id: &str) -> PathBuf {
    archives_dir.join(format!("{id}.json"))
}

fn archive_path(archives_dir: &Path, id: &str) -> PathBuf {
    archives_dir.join(format!("{id}.{ARCHIVE_EXTENSION}"))
}

/// An archive id names files in the archives directory, so it must be one
/// plain file-name component.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

/// A file-name-safe version of a project's name.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            if dash && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(ch);
            dash = false;
        } else {
            dash = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    if slug.is_empty() {
        "project".into()
    } else {
        slug
    }
}

/// `YYYYMMDD-HHMMSS` in UTC, for archive names that sort by date.
fn stamp(unix_ms: i64) -> String {
    let seconds = unix_ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

fn private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// Writes `bytes` to `path` through a staged file and a rename.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = path.with_extension("partial");
    {
        let mut file = File::create(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600))?;
    fs::rename(&staged, path)
}

/// Records each regular file appended, by its name in the archive, so the
/// written tarball can be checked against it.
struct Writer<W: Write> {
    builder: tar::Builder<W>,
    expected: BTreeMap<PathBuf, u64>,
}

impl<W: Write> Writer<W> {
    fn bytes(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        self.builder.append_data(&mut header, name, bytes)?;
        self.expected
            .insert(PathBuf::from(name), bytes.len() as u64);
        Ok(())
    }

    fn file(&mut self, source: &Path, name: &Path) -> io::Result<u64> {
        let mut file = File::open(source)?;
        let size = file.metadata()?.len();
        let mut header = tar::Header::new_gnu();
        header.set_size(size);
        header.set_mode(0o600);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        self.builder.append_data(&mut header, name, &mut file)?;
        self.expected.insert(name.to_path_buf(), size);
        Ok(size)
    }

    /// Appends every regular file under `source` beneath `name`.
    fn tree(&mut self, source: &Path, name: &Path) -> io::Result<u64> {
        let metadata = fs::symlink_metadata(source)?;
        if metadata.is_file() {
            return self.file(source, name);
        }
        if !metadata.is_dir() {
            // A link or socket is not session data.
            return Ok(0);
        }
        let mut total = 0;
        let mut entries: Vec<_> = fs::read_dir(source)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            total += self.tree(&entry.path(), &name.join(entry.file_name()))?;
        }
        Ok(total)
    }
}

/// Checks that the tarball at `path` decompresses and holds exactly the
/// files expected, at their sizes.
fn verify(path: &Path, expected: &BTreeMap<PathBuf, u64>) -> io::Result<()> {
    let mut archive = tar::Archive::new(zstd::Decoder::new(File::open(path)?)?);
    let mut found = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_file() {
            let name = entry.path()?.into_owned();
            // Reading the body is what proves the compressed stream whole.
            let size = io::copy(&mut entry, &mut io::sink())?;
            found.insert(name, size);
        }
    }
    if &found != expected {
        let missing: Vec<_> = expected
            .iter()
            .filter(|(name, size)| found.get(*name) != Some(size))
            .map(|(name, _)| name.display().to_string())
            .take(5)
            .collect();
        return Err(io::Error::other(format!(
            "the written archive does not match what was put in it ({})",
            missing.join(", ")
        )));
    }
    Ok(())
}

fn count_files(path: &Path) -> usize {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return 1;
    }
    if !metadata.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| count_files(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// Every referenced blob must be copied before its source can be removed.
fn copy_blobs(
    source: &BlobStore,
    destination: &BlobStore,
    journals: &[PathBuf],
) -> Result<(), String> {
    gc::copy_journal_blobs(source, destination, journals)
        .map(|_| ())
        .map_err(|error| format!("could not copy journal content: {error}"))
}

/// Archives a project. Nothing is removed unless the archive was written
/// and read back whole.
pub(crate) fn archive(request: Request) -> Result<Archived, String> {
    let _span = perf::span_with(
        "gui.archive.create",
        || serde_json::json!({ "project": request.project_id, "sessions": request.members.len() }),
    );
    let Request {
        archives_dir,
        sessions_dir,
        database,
        project_id,
        members,
        worktrees,
        active_fallback,
        metrics,
    } = request;
    private_directory(&archives_dir)
        .map_err(|error| format!("could not create {}: {error}", archives_dir.display()))?;
    let member_ids: Vec<String> = members.iter().map(|member| member.id.clone()).collect();
    let rows = preferences::project_rows_in(&database, &project_id, &member_ids)
        .map_err(|error| format!("could not read the project: {error}"))?;
    let archived_at_ms = preferences::now_ms();
    let base = format!("{}-{}", slug(&rows.name), stamp(archived_at_ms));
    let mut id = base.clone();
    let mut suffix = 2;
    while archive_path(&archives_dir, &id).exists() || sidecar_path(&archives_dir, &id).exists() {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }

    // Hold every journal so no OMP child opens one while it is copied and
    // removed.
    let mut leases = Vec::new();
    let mut roots = HashSet::new();
    for member in members.iter().filter(|member| member.journal) {
        let path = housekeeping::journal_path(&sessions_dir, &member.id);
        if !path.is_file() {
            continue;
        }
        leases.push(
            housekeeping::lease(&path)
                .map_err(|reason| format!("“{}” is still open: {reason}", member.title))?,
        );
        roots.insert(housekeeping::stem(&member.id));
    }
    let any_children = roots.iter().any(|root| {
        housekeeping::spawned(&sessions_dir.join(format!("{root}.{}", journal::JOURNAL_EXTENSION)))
            .is_ok_and(|children| !children.is_empty())
    });
    let graph = if any_children {
        housekeeping::graph(&sessions_dir)?
    } else {
        Default::default()
    };
    let (mut owned, _) = housekeeping::exclusive_descendants(&graph, &roots);
    let mut warnings = Vec::new();
    let mut entries = Vec::new();
    let mut claimed = HashSet::new();
    for member in &members {
        let root = (member.journal && roots.contains(&housekeeping::stem(&member.id)))
            .then(|| housekeeping::stem(&member.id));
        let mut children = Vec::new();
        if let Some(root) = &root {
            let mut queue = std::collections::VecDeque::from([root.clone()]);
            while let Some(parent) = queue.pop_front() {
                for child in graph.get(&parent).into_iter().flatten() {
                    let journal =
                        sessions_dir.join(format!("{child}.{}", journal::JOURNAL_EXTENSION));
                    if roots.contains(child) || !journal.is_file() || !claimed.insert(child.clone())
                    {
                        continue;
                    }
                    children.push(child.clone());
                    queue.push_back(child.clone());
                }
            }
        }
        entries.push(SessionEntry {
            id: member.id.clone(),
            title: member.title.clone(),
            journal: root,
            children,
            view: member.view.clone(),
            draft_key: member.draft_key,
        });
    }
    owned.retain(|child| {
        if !claimed.contains(child) {
            return false;
        }
        match housekeeping::lease(&housekeeping::journal_path(&sessions_dir, child)) {
            Ok(lease) => {
                leases.push(lease);
                true
            }
            Err(_) => false,
        }
    });
    housekeeping::retain_exclusive(&graph, &roots, &mut owned);
    let mut shared: Vec<String> = claimed.difference(&owned).cloned().collect();
    shared.sort();
    let stems: Vec<String> = entries
        .iter()
        .flat_map(|entry| entry.journal.iter().chain(&entry.children).cloned())
        .collect();
    let journals: Vec<PathBuf> = stems
        .iter()
        .map(|stem| sessions_dir.join(format!("{stem}.{}", journal::JOURNAL_EXTENSION)))
        .collect();

    let staging = archives_dir.join(format!(".staging-{id}"));
    let partial = archives_dir.join(format!(".{id}.{ARCHIVE_EXTENSION}.partial"));
    let cleanup = |paths: &[&Path]| {
        for path in paths {
            let _ = fs::remove_dir_all(path).or_else(|_| fs::remove_file(path));
        }
    };
    let written = (|| -> Result<(Manifest, BTreeMap<PathBuf, u64>), String> {
        let source = BlobStore::open(&sessions_dir).map_err(|error| error.to_string())?;
        let destination = BlobStore::open(&staging).map_err(|error| error.to_string())?;
        if !journals.is_empty() {
            copy_blobs(&source, &destination, &journals)?;
        }
        let metrics_lines = metrics::lines_for(
            &sessions_dir.join(metrics::FILE_NAME),
            &member_ids
                .iter()
                .cloned()
                .chain(stems.iter().cloned())
                .collect::<HashSet<_>>(),
        );
        let file = File::create(&partial).map_err(|error| error.to_string())?;
        let encoder = zstd::Encoder::new(file, ZSTD_LEVEL).map_err(|error| error.to_string())?;
        let mut writer = Writer {
            builder: tar::Builder::new(encoder),
            expected: BTreeMap::new(),
        };
        let mut manifest = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            id: id.clone(),
            archived_at_ms,
            project: rows.clone(),
            sessions: entries.clone(),
            shared_journals: shared.clone(),
            worktrees: worktrees.clone(),
            bytes: 0,
            blobs: count_files(&staging.join(BLOBS)),
            compressed_bytes: 0,
        };
        // The manifest goes first so a listing can stop after one entry;
        // its byte count is filled into the sidecar once the rest is in.
        let body = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
        writer
            .bytes(MANIFEST, &body)
            .map_err(|error| error.to_string())?;
        let mut bytes = 0;
        if !metrics_lines.is_empty() {
            let text = metrics_lines.join("\n") + "\n";
            writer
                .bytes(METRICS, text.as_bytes())
                .map_err(|error| error.to_string())?;
        }
        for stem in &stems {
            for path in housekeeping::files_for(&sessions_dir, stem) {
                // Locks are recreated when a session is opened.
                if path
                    .extension()
                    .is_some_and(|extension| extension == "lock")
                    || !path.exists()
                {
                    continue;
                }
                let name = Path::new(SESSIONS).join(path.file_name().unwrap_or_default());
                bytes += writer
                    .tree(&path, &name)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
            }
        }
        if staging.join(BLOBS).is_dir() {
            bytes += writer
                .tree(&staging.join(BLOBS), Path::new(BLOBS))
                .map_err(|error| error.to_string())?;
        }
        let encoder = writer
            .builder
            .into_inner()
            .map_err(|error| error.to_string())?;
        let file = encoder.finish().map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        manifest.bytes = bytes;
        manifest.compressed_bytes = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        Ok((manifest, writer.expected))
    })();
    let (manifest, expected) = match written {
        Ok(written) => written,
        Err(error) => {
            cleanup(&[&staging, &partial]);
            return Err(format!("could not write the archive: {error}"));
        }
    };
    let path = archive_path(&archives_dir, &id);
    let finished = verify(&partial, &expected)
        .and_then(|()| fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)))
        .and_then(|()| fs::rename(&partial, &path))
        .and_then(|()| {
            let sidecar = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
            write_atomically(&sidecar_path(&archives_dir, &id), &sidecar)
        });
    if let Err(error) = finished {
        cleanup(&[&staging, &partial, &path, &sidecar_path(&archives_dir, &id)]);
        return Err(format!("could not finish the archive: {error}"));
    }
    cleanup(&[&staging]);

    // The archive is whole: now the project leaves what Omperor loads.
    let mut removed_roots = HashSet::new();
    for stem in manifest.sessions.iter().flat_map(|entry| &entry.journal) {
        match housekeeping::remove_stem(&sessions_dir, stem) {
            Ok(_) => {
                removed_roots.insert(stem.clone());
            }
            Err(error) => {
                warnings.push(format!(
                    "Could not remove {stem} after archiving it: {error}"
                ));
            }
        }
    }
    let (removed_children, cleanup_warnings) =
        housekeeping::remove_descendants(&sessions_dir, &graph, removed_roots.clone(), owned);
    warnings.extend(cleanup_warnings);
    drop(leases);
    if let Err(error) = preferences::remove_project_rows_in(
        &database,
        &rows,
        &member_ids,
        active_fallback.as_deref(),
    ) {
        warnings.push(format!(
            "Could not remove the project from the registry: {error}"
        ));
    }
    if let Some(editor) = &metrics {
        let mut sessions = removed_roots;
        sessions.extend(removed_children.into_keys());
        sessions.extend(
            members
                .iter()
                .filter(|member| !member.journal)
                .map(|member| member.id.clone()),
        );
        if let Err(error) = editor.take(sessions) {
            warnings.push(format!(
                "Could not remove the project's metrics from the live log: {error}"
            ));
        }
    }
    Ok(Archived {
        listing: Listing { manifest, path },
        warnings,
    })
}

/// Reads an archive's manifest from its first entry.
fn read_manifest(path: &Path) -> io::Result<Manifest> {
    let mut archive = tar::Archive::new(zstd::Decoder::new(File::open(path)?)?);
    let mut entries = archive.entries()?;
    let mut first = entries
        .next()
        .ok_or_else(|| io::Error::other("the archive is empty"))??;
    if first.path()?.as_ref() != Path::new(MANIFEST) {
        return Err(io::Error::other("the archive has no manifest"));
    }
    let mut body = Vec::new();
    first.read_to_end(&mut body)?;
    let manifest: Manifest = serde_json::from_slice(&body).map_err(io::Error::other)?;
    if manifest.format != FORMAT {
        return Err(io::Error::other("not an Omperor project archive"));
    }
    Ok(manifest)
}

/// Every archive, newest first.
pub(crate) fn list(archives_dir: &Path) -> Result<Vec<Listing>, String> {
    let _span = perf::span("gui.archive.list");
    let entries = match fs::read_dir(archives_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "could not list {}: {error}",
                archives_dir.display()
            ));
        }
    };
    let mut listings = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(id) = name.strip_suffix(&format!(".{ARCHIVE_EXTENSION}")) else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        let manifest = fs::read(sidecar_path(archives_dir, id))
            .ok()
            .and_then(|body| serde_json::from_slice::<Manifest>(&body).ok())
            .or_else(|| {
                let mut manifest = read_manifest(&path).ok()?;
                manifest.compressed_bytes =
                    entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                Some(manifest)
            });
        if let Some(manifest) = manifest {
            listings.push(Listing { manifest, path });
        }
    }
    listings.sort_by_key(|listing| std::cmp::Reverse(listing.manifest.archived_at_ms));
    Ok(listings)
}

/// Deletes an archive for good. Returns the bytes freed.
pub(crate) fn delete(archives_dir: &Path, id: &str) -> Result<u64, String> {
    if !valid_id(id) {
        return Err(format!("“{id}” is not an archive id"));
    }
    let path = archive_path(archives_dir, id);
    let freed = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => format!("there is no archive “{id}”"),
            _ => format!("could not read {}: {error}", path.display()),
        })?;
    fs::remove_file(&path)
        .map_err(|error| format!("could not delete {}: {error}", path.display()))?;
    let _ = fs::remove_file(sidecar_path(archives_dir, id));
    Ok(freed)
}

/// Moves a file or folder, copying when a rename cannot cross volumes.
fn move_path(source: &Path, destination: &Path) -> io::Result<()> {
    if fs::rename(source, destination).is_ok() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            move_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::remove_dir_all(source)
    } else {
        fs::copy(source, destination)?;
        fs::remove_file(source)
    }
}

/// Puts an archived project back: its sessions, blobs, registry rows, and
/// metrics. Refuses, changing nothing, when the project or one of its
/// sessions is already present. The archive is removed once all is back.
pub(crate) fn restore(
    archives_dir: &Path,
    id: &str,
    sessions_dir: &Path,
    database: &Path,
    metrics: Option<&metrics::Editor>,
) -> Result<Restored, String> {
    let _span = perf::span_with("gui.archive.restore", || serde_json::json!({ "id": id }));
    if !valid_id(id) {
        return Err(format!("“{id}” is not an archive id"));
    }
    let path = archive_path(archives_dir, id);
    let manifest = read_manifest(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if manifest.version > VERSION {
        return Err("this archive was written by a newer Omperor".into());
    }
    if preferences::project_exists_in(database, &manifest.project.id)
        .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "the project “{}” is already in Omperor",
            manifest.project.name
        ));
    }
    let staging = archives_dir.join(format!(".restore-{id}"));
    let _ = fs::remove_dir_all(&staging);
    let unpacked = File::open(&path)
        .and_then(zstd::Decoder::new)
        .and_then(|decoder| tar::Archive::new(decoder).unpack(&staging));
    if let Err(error) = unpacked {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("could not unpack the archive: {error}"));
    }
    let outcome = (|| -> Result<Restored, String> {
        let unpacked_sessions = staging.join(SESSIONS);
        let mut names = Vec::new();
        if unpacked_sessions.is_dir() {
            for entry in fs::read_dir(&unpacked_sessions).map_err(|error| error.to_string())? {
                names.push(entry.map_err(|error| error.to_string())?.file_name());
            }
        }
        // A shared sub-agent journal stayed in place; anything else already
        // there means a session of this archive is back in Omperor.
        let shared: HashSet<&str> = manifest
            .shared_journals
            .iter()
            .map(String::as_str)
            .collect();
        let is_shared = |name: &std::ffi::OsStr| {
            let name = name.to_string_lossy();
            let stem = name.split('.').next().unwrap_or_default().to_owned();
            shared.contains(stem.as_str())
        };
        let conflicts: Vec<String> = names
            .iter()
            .filter(|name| sessions_dir.join(name).exists() && !is_shared(name))
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        if !conflicts.is_empty() {
            return Err(format!(
                "these sessions are already in Omperor: {}",
                conflicts.into_iter().take(5).collect::<Vec<_>>().join(", ")
            ));
        }
        let mut warnings = Vec::new();
        fs::create_dir_all(sessions_dir).map_err(|error| error.to_string())?;
        let journals: Vec<PathBuf> = names
            .iter()
            .filter(|name| {
                Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension == journal::JOURNAL_EXTENSION)
            })
            .map(|name| unpacked_sessions.join(name))
            .collect();
        if !journals.is_empty() {
            let source = BlobStore::open(&staging).map_err(|error| error.to_string())?;
            let destination = BlobStore::open(sessions_dir).map_err(|error| error.to_string())?;
            copy_blobs(&source, &destination, &journals)?;
        }
        let mut moved = Vec::new();
        for name in &names {
            let destination = sessions_dir.join(name);
            if destination.exists() {
                continue;
            }
            if let Err(error) = move_path(&unpacked_sessions.join(name), &destination) {
                for path in &moved {
                    let _ = fs::remove_dir_all(path).or_else(|_| fs::remove_file(path));
                }
                return Err(format!(
                    "could not put {} back: {error}",
                    name.to_string_lossy()
                ));
            }
            moved.push(destination);
        }
        let taken_directories =
            match preferences::restore_project_rows_in(database, &manifest.project) {
                Ok(taken) => taken,
                Err(error) => {
                    for path in &moved {
                        let _ = fs::remove_dir_all(path).or_else(|_| fs::remove_file(path));
                    }
                    return Err(format!(
                        "could not restore the project's registry rows: {error}"
                    ));
                }
            };
        match fs::read_to_string(staging.join(METRICS)) {
            Ok(text) => {
                let result = metrics
                    .ok_or_else(|| "the metrics writer is unavailable".to_owned())
                    .and_then(|editor| {
                        editor
                            .restore(text.lines().map(str::to_owned).collect())
                            .map_err(|error| error.to_string())
                    });
                if let Err(error) = result {
                    warnings.push(format!("Could not restore the project's metrics: {error}"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => warnings.push(format!(
                "Could not read the project's archived metrics: {error}"
            )),
        }
        Ok(Restored {
            archive_removed: false,
            manifest: manifest.clone(),
            taken_directories,
            warnings,
        })
    })();
    let _ = fs::remove_dir_all(&staging);
    let mut restored = outcome?;
    if !restored.warnings.is_empty() {
        restored
            .warnings
            .push("The archive was kept because restoration was incomplete".into());
        return Ok(restored);
    }
    match delete(archives_dir, id) {
        Ok(_) => restored.archive_removed = true,
        Err(error) => restored.warnings.push(format!(
            "The project is back, but its archive could not be removed: {error}"
        )),
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::housekeeping::tests::{scratch, write_journal};

    fn fixture(label: &str) -> (PathBuf, Request) {
        let root = scratch(label);
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let database = root.join("omperor.sqlite3");
        let project = preferences::create_project_for_test(&database, "Demo", &root.join("demo"));
        let request = Request {
            archives_dir: root.join("archives"),
            sessions_dir: sessions,
            database,
            project_id: project,
            members: vec![Member {
                id: "parent".into(),
                title: "Parent".into(),
                journal: true,
                view: None,
                draft_key: None,
            }],
            worktrees: Vec::new(),
            active_fallback: None,
            metrics: None,
        };
        (root, request)
    }

    fn attachment(request: &Request) -> (BlobStore, omp_journal::blob::BlobRef) {
        use omp_journal::{EntryDraft, Journal, Kind, KindName};
        let store = BlobStore::open(&request.sessions_dir).unwrap();
        let blob = store.put(b"archived attachment bytes").unwrap();
        let path = request.sessions_dir.join("parent.oms");
        write_journal(&path, &[]);
        // A process another test thread is starting holds a copy of the
        // fixture's lock until it runs its program (see `housekeeping::lease`),
        // so the lock just released can look held for a moment.
        let mut tries = 0;
        let (mut journal, entries) = loop {
            match Journal::open(&path) {
                Err(omp_journal::JournalError::Locked { .. }) if tries < 50 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                opened => break opened.unwrap(),
            }
        };
        let turn = journal
            .append(EntryDraft {
                kind: Kind::known(KindName::TurnStart),
                by: Some(entries[0].id),
                prior: None,
                label: None,
                data: "{}".into(),
            })
            .unwrap();
        journal
            .append(EntryDraft {
                kind: Kind::known(KindName::MsgUser),
                by: Some(turn.id),
                prior: None,
                label: None,
                data: serde_json::json!({"text": "keep this attachment", "attachments": [{
                    "h": blob.to_hex().as_str(), "n": blob.size, "mime": "application/octet-stream"
                }]})
                .to_string()
                .into(),
            })
            .unwrap();
        (store, blob)
    }

    #[test]
    fn failed_blob_restore_keeps_the_archive_and_can_be_retried() {
        let (root, request) = fixture("restore-blob-failure");
        let (store, blob) = attachment(&request);
        let archived = archive(request.clone()).unwrap();
        let shard = store
            .path(&blob)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        // The archived content has been collected from the live store.
        fs::remove_dir_all(&shard).unwrap();
        fs::write(&shard, b"directory unavailable").unwrap();
        let restore = || {
            super::restore(
                &request.archives_dir,
                &archived.listing.manifest.id,
                &request.sessions_dir,
                &request.database,
                None,
            )
        };
        let error = restore().unwrap_err();
        assert!(error.contains("could not copy journal content"), "{error}");
        assert!(archived.listing.path.is_file());
        assert!(!request.sessions_dir.join("parent.oms").exists());
        assert!(!preferences::project_exists_in(&request.database, &request.project_id).unwrap());
        fs::remove_file(&shard).unwrap();
        assert!(restore().unwrap().warnings.is_empty());
        assert_eq!(
            store.get(&blob).unwrap().as_ref(),
            b"archived attachment bytes"
        );
        assert!(request.sessions_dir.join("parent.oms").is_file());
        assert!(preferences::project_exists_in(&request.database, &request.project_id).unwrap());
        assert!(!archived.listing.path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_blob_aborts_archiving_without_removing_the_project() {
        let (root, request) = fixture("archive-blob-failure");
        let (store, blob) = attachment(&request);
        fs::remove_file(store.path(&blob)).unwrap();
        let original = fs::read(request.sessions_dir.join("parent.oms")).unwrap();
        let error = archive(request.clone()).unwrap_err();
        assert!(error.contains("could not copy journal content"), "{error}");
        assert_eq!(
            fs::read(request.sessions_dir.join("parent.oms")).unwrap(),
            original
        );
        assert!(preferences::project_exists_in(&request.database, &request.project_id).unwrap());
        assert!(list(&request.archives_dir).unwrap().is_empty());
        assert_eq!(fs::read_dir(&request.archives_dir).unwrap().count(), 0);
        store.put(b"archived attachment bytes").unwrap();
        assert!(archive(request).unwrap().warnings.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archiving_keeps_locked_descendants_and_everything_they_reference() {
        for direct_leaf in [false, true] {
            let (root, request) = fixture("archive-locked-child");
            let sessions = &request.sessions_dir;
            let children: &[&str] = if direct_leaf {
                &["busy", "leaf", "free"]
            } else {
                &["busy", "free"]
            };
            write_journal(&sessions.join("parent.oms"), children);
            write_journal(&sessions.join("busy.oms"), &["leaf"]);
            write_journal(&sessions.join("leaf.oms"), &[]);
            write_journal(&sessions.join("free.oms"), &[]);
            let busy = housekeeping::lease(&sessions.join("busy.oms")).unwrap();
            let archived = archive(request.clone()).unwrap();
            assert_eq!(archived.listing.manifest.shared_journals, ["busy", "leaf"]);
            assert!(sessions.join("busy.oms").is_file());
            assert!(sessions.join("leaf.oms").is_file());
            assert!(!sessions.join("free.oms").exists());
            assert!(!sessions.join("parent.oms").exists());
            assert!(
                restore(
                    &request.archives_dir,
                    &archived.listing.manifest.id,
                    sessions,
                    &request.database,
                    None
                )
                .unwrap()
                .warnings
                .is_empty()
            );
            assert!(sessions.join("parent.oms").is_file());
            drop(busy);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn incomplete_metrics_restore_keeps_the_archive() {
        let (root, request) = fixture("restore-metrics-failure");
        write_journal(&request.sessions_dir.join("parent.oms"), &[]);
        fs::write(
            request.sessions_dir.join(metrics::FILE_NAME),
            "{\"k\":\"turn\",\"session\":\"parent\"}\n",
        )
        .unwrap();
        let archived = archive(request.clone()).unwrap();
        let restored = restore(
            &request.archives_dir,
            &archived.listing.manifest.id,
            &request.sessions_dir,
            &request.database,
            None,
        )
        .unwrap();
        assert!(restored.warnings.iter().any(|w| w.contains("metrics")));
        assert!(
            restored
                .warnings
                .iter()
                .any(|w| w.contains("archive was kept"))
        );
        assert!(!restored.archive_removed);
        assert!(archived.listing.path.is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_names_are_safe_and_sort_by_date() {
        assert_eq!(slug("Big Bang: Astra!"), "big-bang-astra");
        assert_eq!(slug("…"), "project");
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_790_000_000_000), "20260921-141320");
        assert!(valid_id("omperor-20260921-141320-2"));
        for bad in ["", ".staging-x", "../x", "a/b", "a.b"] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_project_archives_whole_lists_and_restores_exactly() {
        let root = scratch("archive");
        let sessions = root.join("sessions");
        let archives = root.join("archives");
        let database = root.join("omperor.sqlite3");
        fs::create_dir_all(&sessions).unwrap();
        let project = preferences::create_project_for_test(&database, "Demo", &root.join("demo"));
        let other = preferences::create_project_for_test(&database, "Other", &root.join("other"));
        // `one` belongs to the project; `outside` to another project and
        // stays exactly as it was.
        write_journal(&sessions.join("one.oms"), &["helper"]);
        write_journal(&sessions.join("helper.oms"), &[]);
        write_journal(&sessions.join("outside.oms"), &[]);
        let original = fs::read(sessions.join("one.oms")).unwrap();
        for stem in ["one", "outside"] {
            fs::write(sessions.join(format!("{stem}.heights")), b"heights").unwrap();
        }
        fs::create_dir_all(sessions.join("one").join("local")).unwrap();
        fs::write(
            sessions.join("one").join("local").join("note.md"),
            b"artifact",
        )
        .unwrap();
        fs::write(
            sessions.join(metrics::FILE_NAME),
            "{\"k\":\"turn\",\"session\":\"one\"}\n{\"k\":\"turn\",\"session\":\"outside\"}\n",
        )
        .unwrap();
        let metrics_log = metrics::Log::open(sessions.join(metrics::FILE_NAME));

        let archived = archive(Request {
            archives_dir: archives.clone(),
            sessions_dir: sessions.clone(),
            database: database.clone(),
            project_id: project.clone(),
            members: vec![
                Member {
                    id: "one".into(),
                    title: "One".into(),
                    journal: true,
                    view: Some(serde_json::json!({"y": 3})),
                    draft_key: None,
                },
                Member {
                    id: "draft-9".into(),
                    title: "Draft".into(),
                    journal: false,
                    view: Some(serde_json::json!({"draft": "hi"})),
                    draft_key: Some(9),
                },
            ],
            worktrees: vec![root.join("worktree")],
            active_fallback: None,
            metrics: Some(metrics_log.editor()),
        })
        .unwrap();
        assert!(archived.warnings.is_empty(), "{:?}", archived.warnings);
        let manifest = &archived.listing.manifest;
        assert_eq!(manifest.session_count(), 1);
        assert!(manifest.compressed_bytes > 0);
        // The project's files left what Omperor loads; the rest did not.
        for gone in [
            "one.oms",
            "one.heights",
            "one",
            "one.oms.lock",
            "helper.oms",
        ] {
            assert!(!sessions.join(gone).exists(), "{gone} remains");
        }
        assert!(sessions.join("outside.oms").is_file());
        assert!(!preferences::project_exists_in(&database, &project).unwrap());
        assert!(preferences::project_exists_in(&database, &other).unwrap());
        let log = fs::read_to_string(sessions.join(metrics::FILE_NAME)).unwrap();
        assert!(!log.contains("\"one\"") && log.contains("\"outside\""));
        // Private to the app.
        let mode = fs::metadata(&archives).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let mode = fs::metadata(&archived.listing.path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        let listed = list(&archives).unwrap();
        assert_eq!(listed, std::slice::from_ref(&archived.listing));
        // Without its sidecar, the manifest is read from the tarball.
        fs::remove_file(sidecar_path(&archives, &manifest.id)).unwrap();
        let listed = list(&archives).unwrap();
        assert_eq!(listed[0].manifest.sessions, manifest.sessions);

        let restored = restore(
            &archives,
            &manifest.id,
            &sessions,
            &database,
            Some(&metrics_log.editor()),
        )
        .unwrap();
        assert!(restored.warnings.is_empty(), "{:?}", restored.warnings);
        assert_eq!(fs::read(sessions.join("one.oms")).unwrap(), original);
        assert!(sessions.join("helper.oms").is_file());
        assert_eq!(
            fs::read(sessions.join("one").join("local").join("note.md")).unwrap(),
            b"artifact"
        );
        assert!(preferences::project_exists_in(&database, &project).unwrap());
        assert_eq!(restored.manifest.sessions[1].draft_key, Some(9));
        let log = fs::read_to_string(sessions.join(metrics::FILE_NAME)).unwrap();
        assert!(log.contains("\"one\""));
        assert!(list(&archives).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_session_that_is_back_blocks_a_restore_and_nothing_changes() {
        let root = scratch("archive-conflict");
        let sessions = root.join("sessions");
        let archives = root.join("archives");
        let database = root.join("omperor.sqlite3");
        fs::create_dir_all(&sessions).unwrap();
        let project = preferences::create_project_for_test(&database, "Demo", &root.join("demo"));
        write_journal(&sessions.join("one.oms"), &[]);
        let archived = archive(Request {
            archives_dir: archives.clone(),
            sessions_dir: sessions.clone(),
            database: database.clone(),
            project_id: project,
            members: vec![Member {
                id: "one".into(),
                title: "One".into(),
                journal: true,
                view: None,
                draft_key: None,
            }],
            worktrees: Vec::new(),
            active_fallback: None,
            metrics: None,
        })
        .unwrap();
        fs::write(sessions.join("one.oms"), b"a new session with the same id").unwrap();
        let error = restore(
            &archives,
            &archived.listing.manifest.id,
            &sessions,
            &database,
            None,
        )
        .unwrap_err();
        assert!(error.contains("one.oms"), "{error}");
        assert_eq!(list(&archives).unwrap().len(), 1);
        assert_eq!(
            fs::read(sessions.join("one.oms")).unwrap(),
            b"a new session with the same id"
        );
        assert_eq!(
            delete(&archives, &archived.listing.manifest.id).unwrap(),
            archived.listing.manifest.compressed_bytes
        );
        assert!(list(&archives).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
