use filesystem::block::{Block, BlockHeader, Flag, SuperBlock};
use filesystem::block_cache::BlockCache;
use filesystem::constants::*;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TestImage {
    path: PathBuf,
}

impl Drop for TestImage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn make_test_image() -> (TestImage, File) {
    let (path, mut file) = loop {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "filesystem-block-cache-{}-{id}.img",
            std::process::id(),
        ));

        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create test image: {error}"),
        }
    };

    let block_count = 4u64;
    let total_size = block_count * BLOCK_SIZE as u64;
    file.set_len(total_size).expect("size test image");

    let superblock = SuperBlock {
        header: BlockHeader {
            lsn: 0,
            checksum: 0,
            flag: Flag::Clean,
        },
        magic: MAGIC,
        version: 1,
        total_size,
        block_size: BLOCK_SIZE as u16,
        inode_size: INODE_SIZE as u16,
        block_count,
        inode_count: 1,
        free_blocks: 3,
        free_inodes: 0,
        inode_bitmap_start: 1,
        block_bitmap_start: 1,
        inode_map_start: 1,
        data_start: 1,
        state: 1,
        root_inode: 0,
    };

    file.write_all(&superblock.serialise())
        .expect("write test superblock");

    for id in 1..block_count {
        let mut block = Block {
            id,
            header: BlockHeader {
                lsn: 0,
                checksum: 0,
                flag: Flag::Clean,
            },
            buf: vec![0; BLOCK_SIZE],
        };
        block.buf[BLOCK_HEADER_SIZE] = id as u8;
        block.write_block(&file).expect("write initial test block");
    }

    file.sync_all().expect("sync test image");
    (TestImage { path }, file)
}

#[test]
fn flush_writes_dirty_block_to_disk() {
    let (image, file) = make_test_image();
    let mut cache = BlockCache::with_capacity(file, 2);

    {
        let block = cache.get_mut(1).expect("get block");
        block.buf[BLOCK_HEADER_SIZE] = 0xA5;
    }

    cache.flush().expect("flush cache");

    let disk = File::open(&image.path).expect("reopen image");
    let block = Block::deserialise(&disk, 1).expect("read persisted block");
    assert_eq!(block.buf[BLOCK_HEADER_SIZE], 0xA5);
}

#[test]
fn eviction_writes_back_dirty_least_recently_used_block() {
    let (image, file) = make_test_image();
    let mut cache = BlockCache::with_capacity(file, 2);

    // Make block 2 dirty, then make it the least-recently-used entry.
    {
        let block = cache.get_mut(2).expect("get block 2");
        block.buf[BLOCK_HEADER_SIZE] = 0xB6;
    }
    cache.get(1).expect("touch block 1");

    // Loading block 3 exceeds capacity and should evict dirty block 2.
    cache.get(3).expect("load block 3");

    let disk = File::open(&image.path).expect("reopen image");
    let block = Block::deserialise(&disk, 2).expect("read evicted block");
    assert_eq!(block.buf[BLOCK_HEADER_SIZE], 0xB6);
}

#[test]
fn flush_writes_dirty_superblock_to_disk() {
    let (image, file) = make_test_image();
    let mut cache = BlockCache::with_capacity(file, 2);

    cache
        .get_superblock_mut()
        .expect("get superblock")
        .free_blocks = 123;

    cache.flush().expect("flush cache");

    let disk = File::open(&image.path).expect("reopen image");
    let superblock = SuperBlock::deserialise(&disk).expect("read persisted superblock");
    assert_eq!(superblock.free_blocks, 123);
}
