use filesystem::bitmaps::{find_blocks, find_free_inode, mark_blocks_used};
use filesystem::block::{Block, SuperBlock};
use filesystem::constants::*;
use filesystem::directories::{add_dirent, delete, make_dir, read_dir, resolve_path};
use filesystem::extent_tree::{
    delete_extent_range, insert_extent, lookup_extent, range_lookup, Extent, ExtentTreeNode,
};
use filesystem::file_errors::FileError;
use filesystem::files::{Cred, File as FsFile};
use filesystem::filesystem::{create_disk, Filesystem};
use filesystem::inode::find_inode;
use std::fs::{remove_file, File as StdFile};
use std::os::unix::prelude::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const ROOT: u64 = ROOT_INODE_NUM as u64;
const TEST_DISK_SIZE: u64 = 16 * 1024 * 1024;
const TEST_INODE_RATIO: u64 = 16 * 1024;
const PAYLOAD: usize = BLOCK_SIZE - BLOCK_HEADER_SIZE;

static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(0);

struct TestImage {
    path: PathBuf,
    fs: Filesystem,
}

impl TestImage {
    fn new(label: &str) -> Result<Self, FileError> {
        Self::with_geometry(label, TEST_DISK_SIZE, TEST_INODE_RATIO)
    }

    fn with_geometry(label: &str, disk_size: u64, inode_ratio: u64) -> Result<Self, FileError> {
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

    fn disk(&self) -> &StdFile {
        &self.fs.disk
    }
}

impl Drop for TestImage {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

fn cred(uid: u32, gid: u32) -> Cred {
    Cred { uid, gid, groups: Vec::new() }
}

fn root_cred() -> Cred {
    cred(0, 0)
}

fn open_as<'a>(
    image: &'a TestImage,
    path: &str,
    flags: u32,
    who: &Cred,
) -> Result<FsFile<'a>, FileError> {
    FsFile::open(&image.fs, &path.to_string(), ROOT, flags, who)
}

fn is_denied<T>(r: &Result<T, FileError>) -> bool {
    matches!(r, Err(FileError::PermissionDenied))
}

fn count_clear_bits(
    disk: &StdFile,
    first_block: u64,
    end_block: u64,
    limit: u64,
) -> Result<u64, FileError> {
    let mut idx = 0u64;
    let mut clear = 0u64;
    for b in first_block..end_block {
        let block = Block::deserialise(disk, b)?;
        for &byte in &block.buf[BLOCK_HEADER_SIZE..] {
            for bit in 0..8 {
                if idx >= limit {
                    return Ok(clear);
                }
                if byte & (1u8 << bit) == 0 {
                    clear += 1;
                }
                idx += 1;
            }
        }
    }
    Ok(clear)
}

fn assert_accounting(disk: &StdFile) -> Result<(), FileError> {
    let sb = SuperBlock::deserialise(disk)?;
    let free_blocks =
        count_clear_bits(disk, sb.block_bitmap_start, sb.inode_map_start, sb.block_count)?;
    let free_inodes =
        count_clear_bits(disk, sb.inode_bitmap_start, sb.block_bitmap_start, sb.inode_count)?;
    assert_eq!(sb.free_blocks, free_blocks, "superblock free_blocks != block bitmap");
    assert_eq!(sb.free_inodes, free_inodes, "superblock free_inodes != inode bitmap");
    Ok(())
}

fn tree_allocator(disk: &StdFile) -> impl FnMut() -> Result<u64, FileError> + '_ {
    move || {
        let free = find_blocks(disk, 1)?;
        let block_id = free
            .first()
            .map(|e| e.physical_start)
            .ok_or(FileError::NoMoreBlocks)?;
        mark_blocks_used(disk, &free)?;
        Ok(block_id)
    }
}

fn physical_of(
    disk: &StdFile,
    root: &ExtentTreeNode,
    lb: u64,
) -> Result<Option<u64>, FileError> {
    Ok(lookup_extent(disk, root, lb)?.map(|e| e.physical_start + (lb - e.logical_start)))
}

fn assert_disjoint(extents: &[Extent]) {
    let mut sorted = extents.to_vec();
    sorted.sort_by_key(|e| e.logical_start);
    for w in sorted.windows(2) {
        assert!(
            w[0].logical_end() <= w[1].logical_start,
            "overlapping extents: [{}, {}) and [{}, {})",
            w[0].logical_start,
            w[0].logical_end(),
            w[1].logical_start,
            w[1].logical_end()
        );
    }
}

fn build_split_extent_tree(disk: &StdFile) -> Result<ExtentTreeNode, FileError> {
    let mut root = ExtentTreeNode::empty_root();
    let mut allocate = tree_allocator(disk);
    for i in 0..8u64 {
        insert_extent(
            disk,
            &mut root,
            Extent { logical_start: i * 10, physical_start: 1000 + i, length: 1 },
            &mut allocate,
        )?;
    }
    assert!(!root.is_leaf(), "layout assumption: root should have split");
    Ok(root)
}

fn build_deep_tree(disk: &StdFile, count: u64) -> Result<ExtentTreeNode, FileError> {
    let mut root = ExtentTreeNode::empty_root();
    let mut allocate = tree_allocator(disk);
    for i in 0..count {
        insert_extent(
            disk,
            &mut root,
            Extent { logical_start: i * 2, physical_start: 100_000 + i * 2, length: 1 },
            &mut allocate,
        )?;
    }
    Ok(root)
}

#[test]
fn inserting_an_extent_across_a_leaf_boundary_replaces_old_mapping() -> Result<(), FileError> {
    let image = TestImage::new("extent-tree-insert")?;
    let mut root = build_split_extent_tree(image.disk())?;
    let mut allocate = tree_allocator(image.disk());
    let displaced = insert_extent(
        image.disk(),
        &mut root,
        Extent { logical_start: 35, physical_start: 9000, length: 10 },
        &mut allocate,
    )?;

    assert_eq!(displaced.len(), 1);
    assert_eq!(displaced[0].physical_start, 1004);
    assert_eq!(displaced[0].length, 1);
    for lb in 35..45 {
        assert_eq!(physical_of(image.disk(), &root, lb)?, Some(9000 + (lb - 35)), "lb {lb}");
    }
    assert_eq!(physical_of(image.disk(), &root, 30)?, Some(1003));
    assert_eq!(physical_of(image.disk(), &root, 50)?, Some(1005));

    let all = range_lookup(image.disk(), &root, 0, u64::MAX)?;
    assert_disjoint(&all);
    Ok(())
}

#[test]
fn mode_zero_file_cannot_be_opened_for_read_or_write() -> Result<(), FileError> {
    let image = TestImage::new("mode-zero")?;
    let inode_id = add_dirent(image.disk(), ROOT, "private".to_string(), S_IFREG, 1000, 1000)?;
    assert_eq!(find_inode(image.disk(), inode_id)?.i_mode, S_IFREG);

    let owner = cred(1000, 1000);
    for flags in [O_RDONLY, O_WRONLY, O_RDWR] {
        assert!(
            is_denied(&open_as(&image, "/private", flags, &owner)),
            "flags {flags:#b} should be denied"
        );
    }
    Ok(())
}

#[test]
fn insert_spanning_three_leaves_trims_every_overlap() -> Result<(), FileError> {
    let image = TestImage::new("insert-span")?;
    let mut root = build_split_extent_tree(image.disk())?;
    let mut allocate = tree_allocator(image.disk());
    let mut displaced = insert_extent(
        image.disk(),
        &mut root,
        Extent { logical_start: 5, physical_start: 20_000, length: 60 },
        &mut allocate,
    )?;
    displaced.sort_by_key(|e| e.physical_start);
    let phys: Vec<u64> = displaced.iter().map(|e| e.physical_start).collect();
    assert_eq!(phys, vec![1001, 1002, 1003, 1004, 1005, 1006]);
    assert_eq!(displaced.iter().map(|e| e.length).sum::<u64>(), 6);
    assert_eq!(physical_of(image.disk(), &root, 0)?, Some(1000));
    for lb in 1..5 {
        assert_eq!(physical_of(image.disk(), &root, lb)?, None, "lb {lb}");
    }
    for lb in 5..65 {
        assert_eq!(physical_of(image.disk(), &root, lb)?, Some(20_000 + (lb - 5)), "lb {lb}");
    }
    for lb in 65..70 {
        assert_eq!(physical_of(image.disk(), &root, lb)?, None, "lb {lb}");
    }
    assert_eq!(physical_of(image.disk(), &root, 70)?, Some(1007));

    assert_disjoint(&range_lookup(image.disk(), &root, 0, u64::MAX)?);
    Ok(())
}

#[test]
fn delete_range_spanning_two_leaves_removes_both_sides() -> Result<(), FileError> {
    let image = TestImage::new("delete-span")?;
    let mut root = build_split_extent_tree(image.disk())?;
    let mut free_block = |_id: u64| -> Result<(), FileError> { Ok(()) };
    let mut freed = delete_extent_range(image.disk(), &mut root, 25, 55, &mut free_block)?;
    freed.sort_by_key(|e| e.physical_start);
    let phys: Vec<u64> = freed.iter().map(|e| e.physical_start).collect();
    assert_eq!(phys, vec![1003, 1004, 1005]);

    for (lb, expected) in
        [(0, Some(1000)), (10, Some(1001)), (20, Some(1002)), (30, None), (40, None), (50, None), (60, Some(1006)), (70, Some(1007))]
    {
        assert_eq!(physical_of(image.disk(), &root, lb)?, expected, "lb {lb}");
    }
    assert_eq!(range_lookup(image.disk(), &root, 0, u64::MAX)?.len(), 5);
    Ok(())
}

#[test]
fn deleting_everything_frees_every_leaf_and_resets_the_root() -> Result<(), FileError> {
    let image = TestImage::new("delete-all")?;
    let mut root = build_split_extent_tree(image.disk())?;
    let mut released = 0u32;
    let mut free_block = |_id: u64| -> Result<(), FileError> {
        released += 1;
        Ok(())
    };
    let freed = delete_extent_range(image.disk(), &mut root, 0, u64::MAX, &mut free_block)?;
    assert_eq!(freed.iter().map(|e| e.length).sum::<u64>(), 8);
    assert!(root.is_leaf());
    assert_eq!(root.entry_count, 0);
    assert_eq!(released, 2, "both leaf blocks should be released");
    Ok(())
}

#[test]
fn adjacent_extents_merge_only_when_physically_contiguous() -> Result<(), FileError> {
    let image = TestImage::new("merge")?;
    let mut root = ExtentTreeNode::empty_root();
    let mut allocate = tree_allocator(image.disk());
    let mut put = |l: u64, p: u64, root: &mut ExtentTreeNode| {
        insert_extent(
            image.disk(),
            root,
            Extent { logical_start: l, physical_start: p, length: 1 },
            &mut allocate,
        )
    };
    put(0, 500, &mut root)?;
    put(1, 501, &mut root)?;
    let merged = range_lookup(image.disk(), &root, 0, 10)?;
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].length, 2);

    put(2, 900, &mut root)?; // logically adjacent, physically not
    assert_eq!(range_lookup(image.disk(), &root, 0, 10)?.len(), 2);
    Ok(())
}

#[test]
fn overwriting_the_middle_of_an_extent_splits_it_in_three() -> Result<(), FileError> {
    let image = TestImage::new("split-extent")?;
    let mut root = ExtentTreeNode::empty_root();
    let mut allocate = tree_allocator(image.disk());
    insert_extent(
        image.disk(),
        &mut root,
        Extent { logical_start: 0, physical_start: 100, length: 10 },
        &mut allocate,
    )?;
    let freed = insert_extent(
        image.disk(),
        &mut root,
        Extent { logical_start: 4, physical_start: 900, length: 2 },
        &mut allocate,
    )?;
    assert_eq!(freed.len(), 1);
    assert_eq!(freed[0].physical_start, 104);
    assert_eq!(freed[0].length, 2);
    let mut parts = range_lookup(image.disk(), &root, 0, 10)?;
    parts.sort_by_key(|e| e.logical_start);
    let shape: Vec<(u64, u64, u64)> =
        parts.iter().map(|e| (e.logical_start, e.physical_start, e.length)).collect();
    assert_eq!(shape, vec![(0, 100, 4), (4, 900, 2), (6, 106, 4)]);
    Ok(())
}

#[test]
fn depth_two_tree_keeps_every_mapping() -> Result<(), FileError> {
    let image = TestImage::new("deep-insert")?;
    let count = 1500u64;
    let root = build_deep_tree(image.disk(), count)?;
    assert!(root.depth >= 2, "tree should be at least depth 2, got {}", root.depth);

    for i in 0..count {
        assert_eq!(physical_of(image.disk(), &root, i * 2)?, Some(100_000 + i * 2), "i {i}");
        if i % 7 == 0 {
            assert_eq!(physical_of(image.disk(), &root, i * 2 + 1)?, None, "gap after {i}");
        }
    }
    let all = range_lookup(image.disk(), &root, 0, u64::MAX)?;
    assert_eq!(all.len() as u64, count);
    assert!(all.windows(2).all(|w| w[0].logical_start < w[1].logical_start));
    Ok(())
}

#[test]
fn depth_two_tree_range_delete_keeps_the_rest_intact() -> Result<(), FileError> {
    let image = TestImage::new("deep-delete")?;
    let count = 1500u64;
    let mut root = build_deep_tree(image.disk(), count)?;
    let mut released = 0u32;
    let mut free_block = |_id: u64| -> Result<(), FileError> {
        released += 1;
        Ok(())
    };
    let mut freed = delete_extent_range(image.disk(), &mut root, 0, 1000, &mut free_block)?;
    freed.sort_by_key(|e| e.physical_start);
    let phys: Vec<u64> = freed.iter().map(|e| e.physical_start).collect();
    let expected: Vec<u64> = (0..500).map(|i| 100_000 + i * 2).collect();
    assert_eq!(phys, expected);
    assert!(released > 0, "emptied leaves should have been released");
    for i in 0..count {
        let want = if i < 500 { None } else { Some(100_000 + i * 2) };
        assert_eq!(physical_of(image.disk(), &root, i * 2)?, want, "i {i}");
    }
    assert_eq!(range_lookup(image.disk(), &root, 0, u64::MAX)?.len(), 1000);
    Ok(())
}

#[test]
fn mode_bits_select_owner_group_or_other_class() -> Result<(), FileError> {
    let image = TestImage::new("mode-classes")?;
    add_dirent(image.disk(), ROOT, "shared".to_string(), S_IFREG | 0o640, 1000, 2000)?;
    add_dirent(image.disk(), ROOT, "odd".to_string(), S_IFREG | 0o070, 1000, 2000)?;

    let owner = cred(1000, 2000);
    let group_member = cred(3000, 2000);
    let stranger = cred(4000, 4000);
    let supplementary = Cred { uid: 4000, gid: 4000, groups: vec![2000] };
    assert!(open_as(&image, "/shared", O_RDWR, &owner).is_ok());
    assert!(open_as(&image, "/shared", O_RDONLY, &group_member).is_ok());
    assert!(is_denied(&open_as(&image, "/shared", O_WRONLY, &group_member)));
    assert!(is_denied(&open_as(&image, "/shared", O_RDONLY, &stranger)));
    assert!(open_as(&image, "/shared", O_RDONLY, &supplementary).is_ok());
    assert!(is_denied(&open_as(&image, "/odd", O_RDWR, &owner)));
    assert!(open_as(&image, "/odd", O_RDWR, &group_member).is_ok());
    Ok(())
}

#[test]
fn root_bypasses_read_write_bits() -> Result<(), FileError> {
    let image = TestImage::new("root-bypass")?;
    add_dirent(image.disk(), ROOT, "locked".to_string(), S_IFREG, 1000, 1000)?;

    let mut f = open_as(&image, "/locked", O_RDWR, &root_cred())?;
    assert_eq!(f.write(b"hello")?, 5);
    f.fseek(0)?;
    assert_eq!(f.read(5)?, b"hello");
    Ok(())
}

#[test]
fn denied_truncate_leaves_the_file_untouched() -> Result<(), FileError> {
    let image = TestImage::new("denied-trunc")?;
    let data = b"precious data".to_vec();
    let mut f = open_as(&image, "/keep", O_CREAT | O_RDWR, &root_cred())?;
    assert_eq!(f.write(&data)?, data.len());
    drop(f);
    let attacker = cred(1000, 1000);
    assert!(is_denied(&open_as(&image, "/keep", O_WRONLY | O_TRUNC, &attacker)));
    let mut r = open_as(&image, "/keep", O_RDONLY, &root_cred())?;
    assert_eq!(r.read(100)?, data);
    let id = resolve_path(image.disk(), "/keep".to_string(), ROOT)?;
    assert_eq!(find_inode(image.disk(), id)?.i_size, data.len() as u64);
    Ok(())
}

#[test]
fn creating_a_file_needs_write_permission_on_the_parent() -> Result<(), FileError> {
    let image = TestImage::new("create-perm")?;
    let user = cred(1000, 1000);
    assert!(is_denied(&open_as(&image, "/x", O_CREAT | O_RDWR, &user)));
    assert_eq!(read_dir(image.disk(), ROOT)?.len(), 2);
    make_dir(image.disk(), ROOT, "shared".to_string(), 0, 0, 0o777)?;
    let f = open_as(&image, "/shared/x", O_CREAT | O_RDWR, &user);
    assert!(f.is_ok());
    drop(f);
    let id = resolve_path(image.disk(), "/shared/x".to_string(), ROOT)?;
    let inode = find_inode(image.disk(), id)?;
    assert_eq!(inode.i_uid, 1000);
    assert_eq!(inode.i_gid, 1000);
    assert_eq!(inode.i_mode & 0o170000, S_IFREG);
    Ok(())
}

#[test]
fn every_directory_on_the_path_needs_search_permission() -> Result<(), FileError> {
    let image = TestImage::new("search-perm")?;
    let locked = make_dir(image.disk(), ROOT, "locked".to_string(), 0, 0, 0o700)?;
    add_dirent(image.disk(), locked, "f".to_string(), S_IFREG | 0o644, 0, 0)?;
    let user = cred(1000, 1000);
    assert!(is_denied(&open_as(&image, "/locked/f", O_RDONLY, &user)));
    assert!(is_denied(&open_as(&image, "/locked/missing", O_RDONLY, &user)));
    assert!(open_as(&image, "/locked/f", O_RDONLY, &root_cred()).is_ok());
    Ok(())
}

#[test]
fn interleaved_writes_fragment_files_past_the_inode_root_and_truncate_cleanly(
) -> Result<(), FileError> {
    let image = TestImage::new("fragmented")?;
    let root = root_cred();
    let mut a = open_as(&image, "/a", O_CREAT | O_RDWR, &root)?;
    let mut b = open_as(&image, "/b", O_CREAT | O_RDWR, &root)?;
    let free_before = SuperBlock::deserialise(image.disk())?.free_blocks;
    let mut expect_a = Vec::new();
    let mut expect_b = Vec::new();
    for i in 0..24u8 {
        let ca = vec![i + 1; PAYLOAD];
        let cb = vec![i + 101; PAYLOAD];
        assert_eq!(a.write(&ca)?, PAYLOAD);
        assert_eq!(b.write(&cb)?, PAYLOAD);
        expect_a.extend_from_slice(&ca);
        expect_b.extend_from_slice(&cb);
    }
    let a_id = resolve_path(image.disk(), "/a".to_string(), ROOT)?;
    let a_inode = find_inode(image.disk(), a_id)?;
    assert_eq!(a_inode.i_extents.depth, 1, "24 split extents should force a depth-1 tree");
    assert_eq!(
        a_inode.i_blocks,
        24 + a_inode.i_extents.entry_count as u64,
        "i_blocks = data blocks + tree leaves"
    );
    a.fseek(0)?;
    assert!(a.read(expect_a.len())? == expect_a, "file a content mismatch");
    b.fseek(0)?;
    assert!(b.read(expect_b.len())? == expect_b, "file b content mismatch");
    assert_accounting(image.disk())?;
    drop(a);
    drop(b);
    drop(open_as(&image, "/a", O_WRONLY | O_TRUNC, &root)?);
    drop(open_as(&image, "/b", O_WRONLY | O_TRUNC, &root)?);
    assert_eq!(SuperBlock::deserialise(image.disk())?.free_blocks, free_before);
    assert_accounting(image.disk())?;
    Ok(())
}

#[test]
fn failed_write_on_a_full_disk_changes_nothing() -> Result<(), FileError> {
    let image = TestImage::with_geometry("full", 7 * BLOCK_SIZE as u64, 14_336)?;
    let mut f = open_as(&image, "/f", O_CREAT | O_RDWR, &root_cred())?;
    let before = SuperBlock::deserialise(image.disk())?.free_blocks;
    assert_eq!(before, 2);
    let too_big = vec![1u8; PAYLOAD * 3];
    assert!(matches!(f.write(&too_big), Err(FileError::NoMoreBlocks)));
    assert_eq!(SuperBlock::deserialise(image.disk())?.free_blocks, before);
    assert_eq!(f.ftell(), 0);
    let id = resolve_path(image.disk(), "/f".to_string(), ROOT)?;
    let inode = find_inode(image.disk(), id)?;
    assert_eq!(inode.i_size, 0);
    assert_eq!(inode.i_blocks, 0);
    assert_accounting(image.disk())?;
    assert_eq!(f.write(b"ok")?, 2, "file should still be usable");
    Ok(())
}

#[test]
fn truncate_does_not_leak_stale_data_into_a_sparse_gap() -> Result<(), FileError> {
    let image = TestImage::new("stale")?;
    let root = root_cred();
    let mut f = open_as(&image, "/f", O_CREAT | O_RDWR, &root)?;
    assert_eq!(f.write(&vec![0xFF; PAYLOAD * 3])?, PAYLOAD * 3);
    drop(f);
    drop(open_as(&image, "/f", O_WRONLY | O_TRUNC, &root)?);
    let mut f = open_as(&image, "/f", O_RDWR, &root)?;
    let at = PAYLOAD * 2 + 5;
    f.fseek(at as u64)?;
    assert_eq!(f.write(b"X")?, 1);
    f.fseek(0)?;
    let data = f.read(at + 1)?;
    assert_eq!(data.len(), at + 1);
    assert!(data[..at].iter().all(|&b| b == 0), "old contents leaked into the gap");
    assert_eq!(data[at], b'X');
    Ok(())
}

#[test]
fn flipped_payload_byte_is_reported_as_corruption() -> Result<(), FileError> {
    let image = TestImage::new("corrupt")?;
    let root = root_cred();
    let mut f = open_as(&image, "/f", O_CREAT | O_RDWR, &root)?;
    assert_eq!(f.write(&vec![b'x'; 100])?, 100);
    drop(f);

    let id = resolve_path(image.disk(), "/f".to_string(), ROOT)?;
    let inode = find_inode(image.disk(), id)?;
    let pb = lookup_extent(image.disk(), &inode.i_extents, 0)?
        .expect("block 0 should be mapped")
        .physical_start;

    let off = pb * BLOCK_SIZE as u64 + BLOCK_HEADER_SIZE as u64 + 5;
    let mut byte = [0u8; 1];
    image.disk().read_at(&mut byte, off).map_err(|_| FileError::ReadError)?;
    byte[0] ^= 0xFF;
    image.disk().write_all_at(&byte, off).map_err(|_| FileError::WriteError)?;

    let mut r = open_as(&image, "/f", O_RDONLY, &root)?;
    assert!(matches!(r.read(100), Err(FileError::CorruptedBlock)));
    Ok(())
}

#[test]
fn directory_i_blocks_tracks_growth_through_add_dirent() -> Result<(), FileError> {
    let image = TestImage::new("dir-iblocks")?;
    for i in 0..220 {
        add_dirent(image.disk(), ROOT, format!("entry-{i:04}"), S_IFREG | 0o644, 0, 0)?;
    }
    let root = find_inode(image.disk(), ROOT)?;
    let extents = range_lookup(image.disk(), &root.i_extents, 0, u64::MAX)?;
    let data_blocks: u64 = extents.iter().map(|e| e.length).sum();
    assert!(data_blocks >= 2, "directory should have spilled into a second block");
    assert_eq!(root.i_blocks, data_blocks);
    assert_accounting(image.disk())?;
    Ok(())
}

#[test]
fn emptying_a_large_directory_returns_to_baseline() -> Result<(), FileError> {
    let image = TestImage::new("dir-drain")?;
    let base_sb = SuperBlock::deserialise(image.disk())?;
    let base_root = find_inode(image.disk(), ROOT)?;
    for i in 0..220 {
        add_dirent(image.disk(), ROOT, format!("entry-{i:04}"), S_IFREG | 0o644, 0, 0)?;
    }
    for i in 0..220 {
        delete(image.disk(), ROOT, format!("entry-{i:04}"))?;
    }
    assert_eq!(read_dir(image.disk(), ROOT)?.len(), 2);
    let sb = SuperBlock::deserialise(image.disk())?;
    assert_eq!(sb.free_blocks, base_sb.free_blocks);
    assert_eq!(sb.free_inodes, base_sb.free_inodes);
    let root = find_inode(image.disk(), ROOT)?;
    assert_eq!(root.i_size, base_root.i_size);
    assert_accounting(image.disk())?;
    Ok(())
}

#[test]
fn create_write_delete_cycles_do_not_leak_blocks_or_inodes() -> Result<(), FileError> {
    let image = TestImage::new("cycles")?;
    let root = root_cred();
    let base = SuperBlock::deserialise(image.disk())?;
    for round in 0..3 {
        for i in 0..20usize {
            let mut f = open_as(&image, &format!("/f{i}"), O_CREAT | O_RDWR, &root)?;
            let data = vec![i as u8; PAYLOAD * 2 + i * 10];
            assert_eq!(f.write(&data)?, data.len());
        }
        let d = make_dir(image.disk(), ROOT, "dir".to_string(), 0, 0, 0o755)?;
        add_dirent(image.disk(), d, "inner".to_string(), S_IFREG | 0o644, 0, 0)?;

        for i in 0..20 {
            delete(image.disk(), ROOT, format!("f{i}"))?;
        }
        delete(image.disk(), ROOT, "dir".to_string())?;
        let sb = SuperBlock::deserialise(image.disk())?;
        assert_eq!(sb.free_blocks, base.free_blocks, "round {round}: blocks leaked");
        assert_eq!(sb.free_inodes, base.free_inodes, "round {round}: inodes leaked");
        assert_eq!(find_inode(image.disk(), ROOT)?.i_links_count, 2, "round {round}");
        assert_accounting(image.disk())?;
    }
    Ok(())
}

#[test]
fn a_freed_inode_is_reused_with_clean_state() -> Result<(), FileError> {
    let image = TestImage::new("inode-reuse")?;
    let mut f = open_as(&image, "/f1", O_CREAT | O_RDWR, &root_cred())?;
    f.write(&vec![7u8; 5000])?;
    drop(f);
    let old = resolve_path(image.disk(), "/f1".to_string(), ROOT)?;
    delete(image.disk(), ROOT, "f1".to_string())?;
    let new = add_dirent(image.disk(), ROOT, "f2".to_string(), S_IFREG | 0o600, 5, 6)?;
    assert_eq!(new, old);

    let inode = find_inode(image.disk(), new)?;
    assert_eq!(inode.i_size, 0);
    assert_eq!(inode.i_blocks, 0);
    assert_eq!(inode.i_links_count, 1);
    assert_eq!(inode.i_extents.entry_count, 0);
    assert_eq!(inode.i_uid, 5);
    assert_eq!(inode.i_gid, 6);
    assert_eq!(inode.i_mode, S_IFREG | 0o600);
    Ok(())
}

#[test]
fn inode_exhaustion_fails_cleanly_and_recovers_after_delete() -> Result<(), FileError> {
    let image = TestImage::with_geometry("inodes", 7 * BLOCK_SIZE as u64, 14_336)?;
    let first = add_dirent(image.disk(), ROOT, "one".to_string(), S_IFREG | 0o644, 0, 0)?;
    assert_eq!(first, 1);
    assert_eq!(find_free_inode(image.disk())?, None);
    assert!(matches!(
        add_dirent(image.disk(), ROOT, "two".to_string(), S_IFREG | 0o644, 0, 0),
        Err(FileError::NoInodes)
    ));
    assert_eq!(read_dir(image.disk(), ROOT)?.len(), 3, "failed add must not touch the directory");
    assert_accounting(image.disk())?;
    delete(image.disk(), ROOT, "one".to_string())?;
    assert_eq!(add_dirent(image.disk(), ROOT, "two".to_string(), S_IFREG | 0o644, 0, 0)?, 1);
    assert_accounting(image.disk())?;
    Ok(())
}

#[test]
fn dot_entries_resolve_and_files_are_not_path_components() -> Result<(), FileError> {
    let image = TestImage::new("paths")?;
    let d = make_dir(image.disk(), ROOT, "d".to_string(), 0, 0, 0o755)?;
    add_dirent(image.disk(), ROOT, "plain".to_string(), S_IFREG | 0o644, 0, 0)?;
    let resolve = |p: &str, from: u64| resolve_path(image.disk(), p.to_string(), from);
    assert_eq!(resolve("/d/..", ROOT)?, ROOT);
    assert_eq!(resolve("/d/./.", ROOT)?, d);
    assert_eq!(resolve("d/../d", ROOT)?, d);
    assert_eq!(resolve("/d/../d/..", ROOT)?, ROOT);
    assert!(matches!(resolve("/plain/x", ROOT), Err(FileError::NotDirectory)));
    assert!(matches!(
        open_as(&image, "/plain/x", O_CREAT | O_RDWR, &root_cred()),
        Err(FileError::NotDirectory)
    ));
    Ok(())
}
