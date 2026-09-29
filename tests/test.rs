use filesystem::bitmaps::{find_blocks, find_free_inode, mark_blocks_used};
use filesystem::block::SuperBlock;
use filesystem::constants::*;
use filesystem::directories::{
    add_dirent, delete, make_dir, read_dir, resolve_path,
};
use filesystem::extent_tree::{
    delete_extent_range, insert_extent, lookup_extent, range_lookup, Extent,
    ExtentTreeNode,
};
use filesystem::file_errors::FileError;
use filesystem::files::File as FsFile;
use filesystem::filesystem::{create_disk, reserve_inode, Filesystem};
use filesystem::inode::find_inode;
use std::fs::{remove_file, File as StdFile};
use std::panic::{catch_unwind, AssertUnwindSafe};
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
    fn new(label: &str) -> Result<Self, FileError> {
        Self::with_geometry(label, TEST_DISK_SIZE, TEST_INODE_RATIO)
    }

    fn with_geometry(
        label: &str,
        disk_size: u64,
        inode_ratio: u64,
    ) -> Result<Self, FileError> {
        let id = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "filesystem-test-{}-{id}-{label}.img",
            std::process::id()
        ));
        let _ = remove_file(&path);
        let path_string = path.to_str().ok_or(FileError::OpenError)?;
        create_disk(path_string, disk_size, inode_ratio)?;
        let fs = Filesystem::open(path.to_string_lossy().into_owned())?;
        Ok(Self { path, fs })
    }
}

impl Drop for TestImage {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

fn root_path(name: &str) -> String {
    format!("/{name}")
}

#[test]
fn disk_format_and_root_directory_are_initialized() -> Result<(), FileError> {
    let image = TestImage::new("format")?;
    let sb = SuperBlock::deserialise(&image.fs.disk)?;

    assert_eq!(sb.total_size, TEST_DISK_SIZE);
    assert_eq!(sb.block_size as usize, BLOCK_SIZE);
    assert_eq!(sb.inode_size as usize, INODE_SIZE);
    assert_eq!(sb.block_count, TEST_DISK_SIZE / BLOCK_SIZE as u64);
    assert_eq!(sb.inode_count, TEST_DISK_SIZE / TEST_INODE_RATIO);
    assert!(sb.data_start < sb.block_count);
    assert!(sb.free_blocks > 0);
    assert_eq!(sb.root_inode as u64, ROOT);
    assert_eq!(find_free_inode(&image.fs.disk)?, Some(1));

    let root = find_inode(&image.fs.disk, ROOT)?;
    assert!(root.i_mode & S_IFDIR != 0);
    assert_eq!(root.i_links_count, 2);
    assert_eq!(
        read_dir(&image.fs.disk, ROOT)?,
        vec![(".".to_string(), ROOT), ("..".to_string(), ROOT)]
    );
    Ok(())
}

#[test]
fn create_read_overwrite_and_append_across_payload_boundary() -> Result<(), FileError> {
    let image = TestImage::new("file-boundary")?;
    let name = format!("boundary-{}", std::process::id());
    let path = root_path(&name);
    let payload_size = BLOCK_SIZE - BLOCK_HEADER_SIZE;

    let mut expected: Vec<u8> = (0..payload_size + 37)
        .map(|i| (i % 251) as u8)
        .collect();
    let mut file = FsFile::open(
        &image.fs,
        &path,
        ROOT,
        O_CREAT | O_EXCL | O_RDWR,
    )?;

    assert_eq!(file.write(&expected)?, expected.len());
    assert_eq!(file.ftell(), expected.len() as u64);

    file.fseek(0)?;
    assert_eq!(file.read(expected.len())?, expected);

    let overwrite_at = payload_size - 3;
    let replacement = b"UPDATED";
    expected[overwrite_at..overwrite_at + replacement.len()]
        .copy_from_slice(replacement);
    file.fseek(overwrite_at as u64)?;
    assert_eq!(file.write(replacement)?, replacement.len());

    file.fseek(0)?;
    assert_eq!(file.read(expected.len())?, expected);

    let duplicate = FsFile::open(
        &image.fs,
        &path,
        ROOT,
        O_CREAT | O_EXCL | O_RDWR,
    );
    assert!(matches!(duplicate, Err(FileError::NameExists)));

    let mut append = FsFile::open(&image.fs, &path, ROOT, O_APPEND | O_RDWR)?;
    let suffix = b" tail";
    assert_eq!(append.write(suffix)?, suffix.len());
    expected.extend_from_slice(suffix);
    assert_eq!(append.ftell(), expected.len() as u64);
    append.fseek(0)?;
    assert_eq!(append.read(expected.len())?, expected);
    Ok(())
}

#[test]
fn seek_past_eof_writes_zero_filled_sparse_gap() -> Result<(), FileError> {
    let image = TestImage::new("sparse")?;
    let path = root_path("sparse-file");
    let mut file = FsFile::open(&image.fs, &path, ROOT, O_CREAT | O_RDWR)?;
    let payload_size = (BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
    let write_at = payload_size + 11;

    file.fseek(write_at)?;
    assert_eq!(file.write(b"X")?, 1);
    assert_eq!(file.ftell(), write_at + 1);

    file.fseek(0)?;
    let contents = file.read((write_at + 1) as usize)?;
    assert_eq!(contents.len(), (write_at + 1) as usize);
    assert!(contents[..write_at as usize].iter().all(|&b| b == 0));
    assert_eq!(contents[write_at as usize], b'X');
    Ok(())
}

#[test]
fn truncate_clears_file_and_returns_its_data_blocks() -> Result<(), FileError> {
    let image = TestImage::new("truncate")?;
    let path = root_path("truncate-file");
    let payload_size = BLOCK_SIZE - BLOCK_HEADER_SIZE;
    let mut file = FsFile::open(&image.fs, &path, ROOT, O_CREAT | O_RDWR)?;
    let data = vec![0x5a; payload_size * 2 + 19];
    assert_eq!(file.write(&data)?, data.len());
    drop(file);

    let blocks_before_truncate = SuperBlock::deserialise(&image.fs.disk)?.free_blocks;
    let _truncate = FsFile::open(&image.fs, &path, ROOT, O_WRONLY | O_TRUNC)?;
    let blocks_after_truncate = SuperBlock::deserialise(&image.fs.disk)?.free_blocks;
    assert_eq!(blocks_after_truncate, blocks_before_truncate + 3);

    let inode_id = resolve_path(&image.fs.disk, path.clone(), ROOT)?;
    let inode = find_inode(&image.fs.disk, inode_id)?;
    assert_eq!(inode.i_size, 0);
    assert_eq!(inode.i_blocks, 0);
    assert_eq!(inode.i_extents.entry_count, 0);

    let mut reader = FsFile::open(&image.fs, &path, ROOT, O_RDONLY)?;
    assert!(reader.read(10)?.is_empty());
    Ok(())
}

#[test]
fn create_read_duplicate_and_delete_regular_dirent() -> Result<(), FileError> {
    let image = TestImage::new("dirent-lifecycle")?;
    let name = "alpha.txt".to_string();
    let inode_id = add_dirent(
        &image.fs.disk,
        ROOT,
        name.clone(),
        S_IFREG | 0o644,
        1000,
        1000,
    )?;

    let entries = read_dir(&image.fs.disk, ROOT)?;
    assert!(entries.iter().any(|(entry, id)| entry == &name && *id == inode_id));
    assert_eq!(
        resolve_path(&image.fs.disk, format!("/{name}"), ROOT)?,
        inode_id
    );

    assert!(matches!(
        add_dirent(
            &image.fs.disk,
            ROOT,
            name.clone(),
            S_IFREG | 0o644,
            1000,
            1000,
        ),
        Err(FileError::NameExists)
    ));

    delete(&image.fs.disk, ROOT, name.clone())?;
    assert!(!read_dir(&image.fs.disk, ROOT)?
        .iter()
        .any(|(entry, _)| entry == &name));
    assert!(matches!(
        resolve_path(&image.fs.disk, format!("/{name}"), ROOT),
        Err(FileError::NameNotFound)
    ));
    Ok(())
}

#[test]
fn nested_directories_resolve_and_recursive_delete() -> Result<(), FileError> {
    let image = TestImage::new("nested-dirs")?;
    let parent = make_dir(&image.fs.disk, ROOT, "projects".to_string(), 1000, 1000, 0o755)?;
    let child = make_dir(&image.fs.disk, parent, "notes".to_string(), 1000, 1000, 0o750)?;

    assert_eq!(
        resolve_path(&image.fs.disk, "/projects/notes".to_string(), ROOT)?,
        child
    );
    assert_eq!(
        resolve_path(&image.fs.disk, "notes".to_string(), parent)?,
        child
    );

    let child_entries = read_dir(&image.fs.disk, child)?;
    assert!(child_entries.contains(&(".".to_string(), child)));
    assert!(child_entries.contains(&("..".to_string(), parent)));

    let leaf = add_dirent(
        &image.fs.disk,
        child,
        "readme".to_string(),
        S_IFREG | 0o644,
        1000,
        1000,
    )?;
    assert!(read_dir(&image.fs.disk, child)?
        .iter()
        .any(|(name, id)| name == "readme" && *id == leaf));

    delete(&image.fs.disk, ROOT, "projects".to_string())?;
    assert!(matches!(
        resolve_path(&image.fs.disk, "/projects".to_string(), ROOT),
        Err(FileError::NameNotFound)
    ));
    assert_eq!(read_dir(&image.fs.disk, ROOT)?.len(), 2);
    Ok(())
}

#[test]
fn directory_dirents_grow_past_one_block_and_survive_rewrite() -> Result<(), FileError> {
    let image = TestImage::new("directory-growth")?;
    let count = 220usize;

    for i in 0..count {
        let name = format!("entry-{i:04}");
        add_dirent(
            &image.fs.disk,
            ROOT,
            name,
            S_IFREG | 0o644,
            1000,
            1000,
        )?;
    }

    let entries = read_dir(&image.fs.disk, ROOT)?;
    assert_eq!(entries.len(), count + 2);
    assert!(entries.iter().any(|(name, _)| name == "entry-0219"));

    let extents = range_lookup(
        &image.fs.disk,
        &find_inode(&image.fs.disk, ROOT)?.i_extents,
        0,
        u64::MAX,
    )?;
    assert!(extents.iter().map(|extent| extent.length).sum::<u64>() >= 2);

    delete(&image.fs.disk, ROOT, "entry-0100".to_string())?;
    let after_delete = read_dir(&image.fs.disk, ROOT)?;
    assert_eq!(after_delete.len(), count + 1);
    assert!(!after_delete.iter().any(|(name, _)| name == "entry-0100"));
    assert!(after_delete.iter().any(|(name, _)| name == "entry-0219"));
    Ok(())
}

#[test]
fn reserve_inode_uses_bitmap_and_reports_exhaustion() -> Result<(), FileError> {
    let image = TestImage::with_geometry("inode-exhaustion", 7 * BLOCK_SIZE as u64, 14_336)?;
    assert_eq!(find_free_inode(&image.fs.disk)?, Some(1));
    assert_eq!(reserve_inode(&image.fs.disk, S_IFREG | 0o600, 1, 1)?, 1);
    assert_eq!(find_free_inode(&image.fs.disk)?, None);
    assert!(matches!(
        reserve_inode(&image.fs.disk, S_IFREG | 0o600, 1, 1),
        Err(FileError::NoInodes)
    ));
    Ok(())
}

#[test]
fn invalid_access_modes_and_descriptor_permissions_are_rejected() -> Result<(), FileError> {
    let image = TestImage::new("flags")?;
    let path = root_path("flags-file");
    let _file = FsFile::open(&image.fs, &path, ROOT, O_CREAT | O_RDWR)?;

    assert!(matches!(
        FsFile::open(&image.fs, &path, ROOT, O_ACCMODE),
        Err(FileError::InvalidFlags)
    ));
    assert!(matches!(
        FsFile::open(&image.fs, &path, ROOT, O_RDONLY | O_TRUNC),
        Err(FileError::InvalidFlags)
    ));

    let mut writer = FsFile::open(&image.fs, &path, ROOT, O_WRONLY)?;
    assert!(matches!(writer.read(1), Err(FileError::PermissionDenied)));
    let mut reader = FsFile::open(&image.fs, &path, ROOT, O_RDONLY)?;
    assert!(matches!(reader.write(b"x"), Err(FileError::PermissionDenied)));
    Ok(())
}

#[test]
fn opening_root_as_a_directory_succeeds() -> Result<(), FileError> {
    let image = TestImage::new("open-root")?;
    let root = FsFile::open(
        &image.fs,
        &"/".to_string(),
        ROOT,
        O_RDONLY | O_DIRECTORY,
    );
    assert!(root.is_ok(), "opening / as a directory should work");
    Ok(())
}

#[test]
fn trailing_slash_on_regular_file_is_rejected() -> Result<(), FileError> {
    let image = TestImage::new("trailing-slash")?;
    let path = root_path("plain-file");
    let _file = FsFile::open(&image.fs, &path, ROOT, O_CREAT | O_RDWR)?;

    assert!(matches!(
        FsFile::open(&image.fs, &format!("{path}/"), ROOT, O_RDONLY),
        Err(FileError::NotDirectory)
    ));
    Ok(())
}

#[test]
fn regular_file_link_count_is_one_after_dirent_creation() -> Result<(), FileError> {
    let image = TestImage::new("link-count")?;
    let inode_id = add_dirent(
        &image.fs.disk,
        ROOT,
        "linked-file".to_string(),
        S_IFREG | 0o644,
        1000,
        1000,
    )?;
    let inode = find_inode(&image.fs.disk, inode_id)?;
    assert_eq!(inode.i_links_count, 1);
    Ok(())
}

#[test]
fn newly_created_directory_accounts_for_its_data_block() -> Result<(), FileError> {
    let image = TestImage::new("directory-block-count")?;
    let directory = make_dir(
        &image.fs.disk,
        ROOT,
        "with-data".to_string(),
        1000,
        1000,
        0o755,
    )?;
    let inode = find_inode(&image.fs.disk, directory)?;
    assert!(inode.i_blocks >= 1);
    Ok(())
}

#[test]
fn deleting_a_subdirectory_decrements_parent_link_count() -> Result<(), FileError> {
    let image = TestImage::new("parent-links")?;
    let before = find_inode(&image.fs.disk, ROOT)?.i_links_count;
    make_dir(
        &image.fs.disk,
        ROOT,
        "child-dir".to_string(),
        1000,
        1000,
        0o755,
    )?;
    assert_eq!(find_inode(&image.fs.disk, ROOT)?.i_links_count, before + 1);

    delete(&image.fs.disk, ROOT, "child-dir".to_string())?;
    assert_eq!(find_inode(&image.fs.disk, ROOT)?.i_links_count, before);
    Ok(())
}

#[test]
fn mode_zero_file_cannot_be_opened_for_read_or_write() -> Result<(), FileError> {
    let image = TestImage::new("mode-zero")?;
    let inode_id = add_dirent(
        &image.fs.disk,
        ROOT,
        "private".to_string(),
        S_IFREG,
        1000,
        1000,
    )?;
    assert_eq!(find_inode(&image.fs.disk, inode_id)?.i_mode, S_IFREG);

    assert!(matches!(
        FsFile::open(&image.fs, &root_path("private"), ROOT, O_RDWR),
        Err(FileError::PermissionDenied)
    ));
    Ok(())
}

#[test]
fn overlong_dirent_name_returns_an_error_instead_of_panicking() -> Result<(), FileError> {
    let image = TestImage::new("long-name")?;
    let max_payload = BLOCK_SIZE - BLOCK_HEADER_SIZE;
    let too_long = "n".repeat(max_payload - 10 + 1);

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        add_dirent(
            &image.fs.disk,
            ROOT,
            too_long,
            S_IFREG | 0o644,
            1000,
            1000,
        )
    }));

    assert!(outcome.is_ok(), "add_dirent panicked on an oversized name");
    assert!(
        outcome.expect("panic was checked above").is_err(),
        "an oversized dirent should be rejected"
    );
    Ok(())
}

#[test]
fn delete_refuses_dot_entries() -> Result<(), FileError> {
    let image = TestImage::new("delete-dot")?;
    let result = delete(&image.fs.disk, ROOT, ".".to_string());
    assert!(result.is_err(), "deleting . must not free the root inode");
    assert_ne!(find_free_inode(&image.fs.disk)?, Some(ROOT));
    assert_eq!(read_dir(&image.fs.disk, ROOT)?.len(), 2);
    Ok(())
}

#[test]
fn a_one_block_write_succeeds_when_two_free_blocks_remain() -> Result<(), FileError> {
    let image = TestImage::with_geometry("low-space", 7 * BLOCK_SIZE as u64, 14_336)?;
    assert_eq!(SuperBlock::deserialise(&image.fs.disk)?.free_blocks, 2);
    let mut file = FsFile::open(
        &image.fs,
        &root_path("last-block"),
        ROOT,
        O_CREAT | O_RDWR,
    )?;
    assert_eq!(file.write(b"x")?, 1);
    Ok(())
}

fn build_split_extent_tree(disk: &StdFile) -> Result<ExtentTreeNode, FileError> {
    let mut root = ExtentTreeNode::empty_root();

    for i in 0..8u64 {
        let mut allocate = || -> Result<u64, FileError> {
            let free = find_blocks(disk, 1)?;
            let block_id = free
                .first()
                .map(|extent| extent.physical_start)
                .ok_or(FileError::NoMoreBlocks)?;
            mark_blocks_used(disk, &free)?;
            Ok(block_id)
        };
        insert_extent(
            disk,
            &mut root,
            Extent {
                logical_start: i * 10,
                physical_start: 1000 + i,
                length: 1,
            },
            &mut allocate,
        )?;
    }

    Ok(root)
}

#[test]
fn extent_tree_lookup_and_range_lookup_work_after_root_split() -> Result<(), FileError> {
    let image = TestImage::new("extent-tree-read")?;
    let root = build_split_extent_tree(&image.fs.disk)?;

    let extents = range_lookup(&image.fs.disk, &root, 0, 80)?;
    assert_eq!(extents.len(), 8);
    assert_eq!(
        lookup_extent(&image.fs.disk, &root, 40)?
            .expect("logical block 40 should be mapped")
            .physical_start,
        1004
    );
    assert!(lookup_extent(&image.fs.disk, &root, 41)?.is_none());
    Ok(())
}

#[test]
fn inserting_an_extent_across_a_leaf_boundary_replaces_old_mapping() -> Result<(), FileError> {
    let image = TestImage::new("extent-tree-insert")?;
    let mut root = build_split_extent_tree(&image.fs.disk)?;

    let mut allocate = || -> Result<u64, FileError> {
        let free = find_blocks(&image.fs.disk, 1)?;
        let block_id = free
            .first()
            .map(|extent| extent.physical_start)
            .ok_or(FileError::NoMoreBlocks)?;
        mark_blocks_used(&image.fs.disk, &free)?;
        Ok(block_id)
    };
    let displaced = insert_extent(
        &image.fs.disk,
        &mut root,
        Extent {
            logical_start: 35,
            physical_start: 9000,
            length: 10,
        },
        &mut allocate,
    )?;

    assert_eq!(displaced.len(), 1);
    assert_eq!(displaced[0].physical_start, 1004);
    assert_eq!(displaced[0].length, 1);
    assert_eq!(range_lookup(&image.fs.disk, &root, 35, 45)?.len(), 1);
    Ok(())
}

#[test]
fn partial_delete_starting_in_a_gap_removes_the_later_leaf_extent() -> Result<(), FileError> {
    let image = TestImage::new("extent-tree-delete")?;
    let mut root = build_split_extent_tree(&image.fs.disk)?;
    let mut free_block = |_block_id: u64| Ok(());

    let freed = delete_extent_range(
        &image.fs.disk,
        &mut root,
        39,
        41,
        &mut free_block,
    )?;

    assert_eq!(freed.len(), 1);
    assert_eq!(freed[0].logical_start, 40);
    assert_eq!(freed[0].physical_start, 1004);
    assert!(lookup_extent(&image.fs.disk, &root, 40)?.is_none());
    Ok(())
}


