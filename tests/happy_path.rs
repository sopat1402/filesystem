use filesystem::constants::*;
use filesystem::directories::{delete, make_dir, read_dir, resolve_path};
use filesystem::file_errors::FileError;
use filesystem::files::File as FsFile;
use filesystem::filesystem::{create_disk, Filesystem};
use std::fs::remove_file;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const ROOT: u64 = ROOT_INODE_NUM as u64;
const TEST_DISK_SIZE: u64 = 16 * 1024 * 1024;
const TEST_INODE_RATIO: u64 = 16 * 1024;

static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(0);

struct TestImage {
    path: PathBuf,
    fs: Filesystem,
}

impl TestImage {
    fn new() -> Result<Self, FileError> {
        let id = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "filesystem-typical-workflow-{}-{id}.img",
            std::process::id()
        ));
        let _ = remove_file(&path);
        let path_string = path.to_str().ok_or(FileError::OpenError)?;
        create_disk(path_string, TEST_DISK_SIZE, TEST_INODE_RATIO)?;
        let fs = Filesystem::open(path.to_string_lossy().into_owned())?;
        Ok(Self { path, fs })
    }
}

impl Drop for TestImage {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

#[test]
fn typical_workflow_creates_edits_lists_and_removes_files() -> Result<(), FileError> {
    let image = TestImage::new()?;
    let projects = make_dir(
        &image.fs.disk,
        ROOT,
        "projects".to_string(),
        1000,
        1000,
        0o755,
    )?;
    let notes = make_dir(
        &image.fs.disk,
        projects,
        "notes".to_string(),
        1000,
        1000,
        0o755,
    )?;
    let file_path = "/projects/notes/today.txt".to_string();

    let mut file = FsFile::open(
        &image.fs,
        &file_path,
        ROOT,
        O_CREAT | O_EXCL | O_RDWR,
    )?;
    let initial = b"Write the project update.";
    assert_eq!(file.write(initial)?, initial.len());

    file.fseek(0)?;
    assert_eq!(file.read(initial.len())?, initial);
    drop(file);

    let mut append_file = FsFile::open(
        &image.fs,
        &file_path,
        ROOT,
        O_APPEND | O_RDWR,
    )?;
    let addition = b"\nReview the directory tests.";
    assert_eq!(append_file.write(addition)?, addition.len());

    let expected = [initial.as_slice(), addition.as_slice()].concat();
    append_file.fseek(0)?;
    assert_eq!(append_file.read(expected.len())?, expected);
    drop(append_file);

    let inode_id = resolve_path(&image.fs.disk, file_path, ROOT)?;
    assert!(read_dir(&image.fs.disk, notes)?
        .iter()
        .any(|(name, id)| name == "today.txt" && *id == inode_id));
    assert!(read_dir(&image.fs.disk, projects)?
        .iter()
        .any(|(name, id)| name == "notes" && *id == notes));

    delete(&image.fs.disk, notes, "today.txt".to_string())?;
    assert_eq!(read_dir(&image.fs.disk, notes)?.len(), 2);
    assert!(matches!(
        resolve_path(
            &image.fs.disk,
            "/projects/notes/today.txt".to_string(),
            ROOT
        ),
        Err(FileError::NameNotFound)
    ));

    delete(&image.fs.disk, projects, "notes".to_string())?;
    delete(&image.fs.disk, ROOT, "projects".to_string())?;
    assert_eq!(read_dir(&image.fs.disk, ROOT)?.len(), 2);
    Ok(())
}
