// The SMP's files under the world's folder (`world/horizon-smp/`): JSON
// documents written whole and atomically (a temporary file, flushed, renamed
// over the old one), and the audit log, one JSON line per transaction,
// appended before the change it records. Added to the game as
// `game::smp_store`; also a module of this crate, for its tests.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Condvar, Mutex},
    time::Duration,
};

/// The folder of the SMP's files in a world's folder.
pub const DIR: &str = "horizon-smp";

/// Writes `bytes` to `path` so that a crash leaves either the old file or the
/// new one: a temporary file beside it, flushed to disk, then renamed over it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = dir.join(format!(".{name}.tmp"));
    {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temp, path)?;
    // The rename itself reaches the disk with the folder's entry (Unix).
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// A JSON document: its default when the file doesn't exist, an error when it
/// exists but can't be read, so that a damaged file is never replaced by an
/// empty one.
pub fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    wait_for(path);
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Writes a JSON document atomically, pretty-printed.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// A JSON document saved like [`write_json`], but written by the SMP's
/// writer thread so the server's tick doesn't wait for the disk: the bytes
/// are made now, and a newer save of the same file replaces one still
/// waiting. [`flush`] writes what waits (the world closes); [`read_json`]
/// waits for the file it reads. A failed write is tried again a second
/// later, its error kept for [`take_errors`].
pub fn save_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut q = WRITER.queue.lock().unwrap_or_else(|e| e.into_inner());
    if q.bytes.insert(path.to_path_buf(), bytes).is_none() {
        q.order.push_back(path.to_path_buf());
    }
    if !q.thread {
        q.thread = std::thread::Builder::new()
            .name("smp-writer".into())
            .spawn(writer)
            .is_ok();
    }
    drop(q);
    WRITER.changed.notify_all();
    Ok(())
}

/// Writes every save still waiting, on this thread, once the one being
/// written is done; the errors of those that failed.
pub fn flush() -> Vec<String> {
    let mut q = WRITER.queue.lock().unwrap_or_else(|e| e.into_inner());
    while q.writing.is_some() {
        q = WRITER.changed.wait(q).unwrap_or_else(|e| e.into_inner());
    }
    let mut errors = std::mem::take(&mut q.errors);
    while let Some(path) = q.order.pop_front() {
        if let Some(bytes) = q.bytes.remove(&path)
            && let Err(e) = write_atomic(&path, &bytes)
        {
            errors.push(format!("{}: {e}", path.display()));
        }
    }
    drop(q);
    WRITER.changed.notify_all();
    errors
}

/// Whether `path` exists once its last save is on disk.
pub fn exists(path: &Path) -> bool {
    wait_for(path);
    path.exists()
}

/// The errors of writes the writer thread failed since the last call.
pub fn take_errors() -> Vec<String> {
    std::mem::take(
        &mut WRITER
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .errors,
    )
}

/// [`flush`] for a test: writes what waits, and returns only the errors of
/// files under `dir`. The errors of every other folder stay for the test
/// they belong to. The writer is one for the whole process, so a plain
/// `flush` in one test takes the errors another test is waiting for.
#[cfg(test)]
pub(crate) fn flush_dir(dir: &Path) -> Vec<String> {
    let dir = dir.to_string_lossy().into_owned();
    let mut q = WRITER.queue.lock().unwrap_or_else(|e| e.into_inner());
    while q.writing.is_some() {
        q = WRITER.changed.wait(q).unwrap_or_else(|e| e.into_inner());
    }
    let mut errors = std::mem::take(&mut q.errors);
    while let Some(path) = q.order.pop_front() {
        if let Some(bytes) = q.bytes.remove(&path)
            && let Err(e) = write_atomic(&path, &bytes)
        {
            errors.push(format!("{}: {e}", path.display()));
        }
    }
    let (mine, others): (Vec<String>, Vec<String>) = errors.into_iter().partition(|e| {
        e.strip_prefix(dir.as_str())
            .is_some_and(|rest| rest.starts_with(['/', std::path::MAIN_SEPARATOR]))
    });
    // Put back while the queue is still locked: no other flush sees a gap.
    q.errors.extend(others);
    drop(q);
    WRITER.changed.notify_all();
    mine
}

struct Writer {
    queue: Mutex<Queue>,
    changed: Condvar,
}

/// Files waiting, in the order first saved, and the newest bytes of each.
struct Queue {
    order: VecDeque<PathBuf>,
    bytes: BTreeMap<PathBuf, Vec<u8>>,
    writing: Option<PathBuf>,
    errors: Vec<String>,
    thread: bool,
}

static WRITER: Writer = Writer {
    queue: Mutex::new(Queue {
        order: VecDeque::new(),
        bytes: BTreeMap::new(),
        writing: None,
        errors: Vec::new(),
        thread: false,
    }),
    changed: Condvar::new(),
};

/// Returns once `path` holds its last save: the one being written is done,
/// the one waiting is written here.
fn wait_for(path: &Path) {
    let mut q = WRITER.queue.lock().unwrap_or_else(|e| e.into_inner());
    while q.writing.as_deref() == Some(path) {
        q = WRITER.changed.wait(q).unwrap_or_else(|e| e.into_inner());
    }
    if let Some(bytes) = q.bytes.remove(path) {
        q.order.retain(|p| p != path);
        if let Err(e) = write_atomic(path, &bytes) {
            q.errors.push(format!("{}: {e}", path.display()));
        }
    }
}

fn writer() {
    let mut q = WRITER.queue.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let Some(path) = q.order.pop_front() else {
            q = WRITER.changed.wait(q).unwrap_or_else(|e| e.into_inner());
            continue;
        };
        let Some(bytes) = q.bytes.remove(&path) else {
            continue;
        };
        q.writing = Some(path.clone());
        drop(q);
        let result = write_atomic(&path, &bytes);
        q = WRITER.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.writing = None;
        if let Err(e) = result {
            q.errors.push(format!("{}: {e}", path.display()));
            // Tried again unless a newer save came meanwhile.
            if !q.bytes.contains_key(&path) {
                q.bytes.insert(path.clone(), bytes);
                q.order.push_back(path);
            }
            WRITER.changed.notify_all();
            q = WRITER
                .changed
                .wait_timeout(q, Duration::from_secs(1))
                .unwrap_or_else(|e| e.into_inner())
                .0;
            continue;
        }
        WRITER.changed.notify_all();
    }
}

/// `value` as compact JSON in `path`, atomically (large files: the
/// market's prices and the web pages).
pub fn write_json_compact<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The audit log: `audit.log`, one JSON object a line.
pub struct Audit {
    path: PathBuf,
    file: Option<File>,
    /// Lines appended since the last sync.
    unsynced: bool,
}

impl Audit {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            unsynced: false,
        }
    }
    /// Appends one line; the change it records waits for it.
    pub fn append(&mut self, entry: &serde_json::Value) -> Result<(), String> {
        let mut line = entry.to_string();
        line.push('\n');
        let path = &self.path;
        if self.file.is_none() {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            self.file = Some(file);
        }
        let file = self.file.as_mut().expect("opened above");
        file.write_all(line.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.unsynced = true;
        Ok(())
    }
    /// Flushes what was appended to disk (at the end of a tick that appended,
    /// and when the world closes); nothing when nothing was appended, so a
    /// quiet tick makes no system call.
    pub fn sync(&mut self) {
        if self.unsynced
            && let Some(f) = &self.file
        {
            let _ = f.sync_data();
            self.unsynced = false;
        }
    }
}

/// `UUID.toString()`: 8-4-4-4-12 lowercase hex digits.
pub fn uuid_string(u: u128) -> String {
    let h = format!("{u:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// The UUID a [`uuid_string`] wrote.
pub fn parse_uuid(s: &str) -> Option<u128> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    (hex.len() == 32)
        .then(|| u128::from_str_radix(&hex, 16).ok())
        .flatten()
}

/// `2026-11-02 14:05` from ms since the epoch (UTC).
pub fn utc_minute(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// The last `bytes` bytes of a file, from its first whole line; empty when
/// it can't be read.
pub fn tail(path: &Path, bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(bytes);
    let mut bytes = Vec::new();
    if f.seek(SeekFrom::Start(from)).is_err() || f.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    match (from > 0).then(|| text.find('\n')).flatten() {
        Some(i) => text[i + 1..].to_owned(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("smp-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// The writer is shared by every test in the crate: other modules' tests queue saves into folders they
    /// remove, and their errors surface in whichever `flush` runs next. Take only this test's, and leave the
    /// others' (`flush_dir`).
    fn flush_in(dir: &Path) -> Vec<String> {
        flush_dir(dir)
    }

    #[test]
    fn atomic_writes_replace_the_file_and_leave_no_temporary() {
        let dir = scratch("atomic");
        let path = dir.join("economy.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["economy.json"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_damaged_document_is_an_error_not_an_empty_one() {
        let dir = scratch("damaged");
        let path = dir.join("economy.json");
        let missing: Vec<u32> = read_json(&path).unwrap();
        assert!(missing.is_empty());
        write_atomic(&path, b"{ not json").unwrap();
        assert!(read_json::<Vec<u32>>(&path).is_err());
        write_json(&path, &vec![1u32, 2]).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), [1, 2]);
        fs::remove_dir_all(dir).unwrap();
    }

    /// The writer is one for the process: its tests take turns.
    static WRITER_TESTS: Mutex<()> = Mutex::new(());

    #[test]
    fn saves_reach_the_disk_newest_last_and_flush_waits_for_them() {
        let _turn = WRITER_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("later");
        let a = dir.join("economy.json");
        let b = dir.join("places.json");
        for i in 0..50u32 {
            save_json(&a, &vec![i]).unwrap();
            save_json(&b, &vec![i, i]).unwrap();
        }
        assert!(flush_in(&dir).is_empty());
        // Nothing waits once flushed: the files hold the last saves.
        assert_eq!(fs::read_to_string(&a).unwrap(), "[\n  49\n]\n");
        assert_eq!(read_json::<Vec<u32>>(&b).unwrap(), [49, 49]);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "no temporary left: {names:?}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_read_sees_the_save_before_it() {
        let _turn = WRITER_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("read-after");
        let path = dir.join("claims.json");
        for i in 0..20u32 {
            save_json(&path, &vec![i]).unwrap();
            assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), [i]);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_save_is_reported_and_kept_for_the_next_try() {
        let _turn = WRITER_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("failed");
        fs::create_dir_all(&dir).unwrap();
        // A file where the folder should be: every write under it fails.
        let blocker = dir.join("sub");
        fs::write(&blocker, b"not a folder").unwrap();
        let path = blocker.join("nicks.json");
        save_json(&path, &vec![1u32]).unwrap();
        let errors = flush_in(&dir);
        assert!(
            !errors.is_empty() && errors.iter().all(|e| e.contains("nicks.json")),
            "{errors:?}"
        );
        // Once the folder can be made, the next save lands.
        fs::remove_file(&blocker).unwrap();
        save_json(&path, &vec![2u32]).unwrap();
        assert!(flush_in(&dir).is_empty());
        assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), [2]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_flush_for_one_folder_leaves_the_errors_of_another_folders_saves() {
        // The admin and economy tests flush for their own folders while this
        // module's failed-save test has a save waiting that must fail: their
        // flush must not take its error.
        let _turn = WRITER_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let failing = scratch("keep-failing");
        let other = scratch("keep-other");
        fs::create_dir_all(&failing).unwrap();
        let blocker = failing.join("sub");
        fs::write(&blocker, b"not a folder").unwrap();
        save_json(&blocker.join("nicks.json"), &vec![1u32]).unwrap();
        assert!(flush_in(&other).is_empty());
        let errors = flush_in(&failing);
        assert!(
            !errors.is_empty() && errors.iter().all(|e| e.contains("nicks.json")),
            "{errors:?}"
        );
        assert!(flush_in(&failing).is_empty(), "taken once");
        fs::remove_dir_all(failing).unwrap();
    }

    #[test]
    fn the_audit_log_appends_lines() {
        let dir = scratch("audit");
        let mut audit = Audit::new(dir.join("audit.log"));
        audit
            .append(&serde_json::json!({"kind": "pay", "amount": 5}))
            .unwrap();
        audit
            .append(&serde_json::json!({"kind": "pay", "amount": 6}))
            .unwrap();
        audit.sync();
        let text = fs::read_to_string(dir.join("audit.log")).unwrap();
        assert_eq!(
            text,
            "{\"amount\":5,\"kind\":\"pay\"}\n{\"amount\":6,\"kind\":\"pay\"}\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn uuids_round_trip() {
        let u = 0x0000_0000_0000_4000_8000_0000_0000_0001u128;
        assert_eq!(uuid_string(u), "00000000-0000-4000-8000-000000000001");
        assert_eq!(parse_uuid(&uuid_string(u)), Some(u));
        assert_eq!(parse_uuid("nope"), None);
    }
}
