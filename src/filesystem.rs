use crate::inode::Inode;
use crate::extent_tree::{Extent, ExtentTreeNode};
use crate::block::{SuperBlock, BlockHeader, Block, Flag};
use crate::bitmaps::{mark_inode_used, find_free_inode};
use crate::file_errors::FileError;
use std::fs::File;
use std::time::{SystemTime, UNIX_EPOCH};
use std::os::unix::prelude::FileExt;
use crate::constants::*;

pub struct Filesystem {
    pub disk: std::fs::File,
}

impl Filesystem {
    pub fn open(path: String) -> Result<Self, FileError> {
        let disk = std::fs::File::options()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| FileError::OpenError)?;
        Ok(Self { disk })
    }
}

fn new_block(id: u64) -> Block {
    Block {
        id,
        header: BlockHeader { lsn: 0, checksum: 0, flag: Flag::Clean },
        buf: vec![0u8; BLOCK_SIZE],
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn create_disk(path: &str, disk_size: u64, inode_ratio: u64) -> Result<(), FileError> {
    let bs = BLOCK_SIZE as u64;
    if disk_size == 0 || disk_size % bs != 0 {
        return Err(FileError::MisalignedSize);
    }
    let payload = (BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
    let block_count = disk_size / bs;
    let total_inodes = inode_ratio
        .checked_mul(block_count)
        .filter(|&n| n > 0)
        .ok_or(FileError::NoInodes)?;

    // Layout: superblock | inode bitmap | block bitmap | inode table | data
    let inode_bitmap_start = 1u64;
    let inode_bitmap_blocks = total_inodes.div_ceil(8).div_ceil(payload);
    let block_bitmap_start = inode_bitmap_start + inode_bitmap_blocks;
    let block_bitmap_blocks = block_count.div_ceil(8).div_ceil(payload);
    let inode_map_start = block_bitmap_start + block_bitmap_blocks;
    let inode_map_blocks = total_inodes.div_ceil(INODES_PER_BLOCK as u64);
    let data_start = inode_map_start + inode_map_blocks;
    let root_dir_block = data_start;
    let reserved = root_dir_block + 1;
    if reserved > block_count {
        return Err(FileError::NoMoreBlocks);
    }

    let disk = File::create(path).map_err(|_| FileError::WriteError)?;
    disk.set_len(disk_size).map_err(|_| FileError::WriteError)?;

    // Superblock
    let sb = SuperBlock {
        header: BlockHeader { lsn: 0, checksum: 0, flag: Flag::Clean },
        magic: MAGIC,
        version: 1,
        total_size: disk_size,
        block_size: BLOCK_SIZE as u16,
        inode_size: INODE_SIZE as u16,
        block_count,
        inode_count: total_inodes,
        free_blocks: block_count - reserved,
        free_inodes: total_inodes - 1,
        inode_bitmap_start,
        block_bitmap_start,
        inode_map_start,
        data_start,
        state: 0,
        root_inode: ROOT_INODE_NUM as u32,
    };
    disk.write_all_at(&sb.serialise(), 0)
        .map_err(|_| FileError::WriteError)?;

    // Inode bitmap: only the root inode is used.
    for k in 0..inode_bitmap_blocks {
        let mut block = new_block(inode_bitmap_start + k);
        if k == 0 {
            block.buf[BLOCK_HEADER_SIZE] = 0x01;
        }
        block.write_block(&disk)?;
    }

    // Block bitmap: every block below `reserved` is used.
    let bits_per_block = payload * 8;
    for k in 0..block_bitmap_blocks {
        let mut block = new_block(block_bitmap_start + k);
        let first_bit = k * bits_per_block;
        for byte in 0..payload {
            let base = first_bit + byte * 8;
            if base >= reserved {
                break;
            }
            let bits = (reserved - base).min(8);
            block.buf[BLOCK_HEADER_SIZE + byte as usize] =
                if bits == 8 { 0xFF } else { (1u8 << bits) - 1 };
        }
        block.write_block(&disk)?;
    }

    // Root directory contents format len, name, u64 inode
    let mut root_dir: Vec<u8> = Vec::new();
    for name in [".", ".."] {
        root_dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
        root_dir.extend_from_slice(name.as_bytes());
        root_dir.extend_from_slice(&(ROOT_INODE_NUM as u64).to_le_bytes());
    }

    // Inode table
    let empty_inode = Inode {
        i_mode: 0,
        i_uid: 0,
        i_size: 0,
        i_atime: 0,
        i_ctime: 0,
        i_mtime: 0,
        i_dtime: 0,
        i_gid: 0,
        i_links_count: 0,
        i_blocks: 0,
        i_flags: 0,
        i_extents: ExtentTreeNode::empty_root(),
        i_generation: 0,
        i_reserved: [0u8; 16],
    };
    let empty_bytes = empty_inode.serialise();

    let now = now_secs();
    let mut root_extents = ExtentTreeNode::empty_root();
    root_extents.entries = vec![Extent {
        logical_start: 0,
        physical_start: root_dir_block,
        length: 1,
    }
    .to_bytes()];
    root_extents.entry_count = 1;
    let root_inode = Inode {
        i_mode: 0o040755,
        i_uid: 0,
        i_size: root_dir.len() as u64,
        i_atime: now,
        i_ctime: now,
        i_mtime: now,
        i_dtime: 0,
        i_gid: 0,
        i_links_count: 2,
        i_blocks: 1,
        i_flags: 0,
        i_extents: root_extents,
        i_generation: 0,
        i_reserved: [0u8; 16],
    };
    let root_bytes = root_inode.serialise();

    for block_idx in 0..inode_map_blocks {
        let mut block = new_block(inode_map_start + block_idx);
        let first_inode = block_idx * INODES_PER_BLOCK as u64;
        let in_block = (total_inodes - first_inode).min(INODES_PER_BLOCK as u64);
        for slot in 0..in_block {
            let start = BLOCK_HEADER_SIZE + slot as usize * INODE_SIZE;
            let bytes = if first_inode + slot == ROOT_INODE_NUM as u64 {
                &root_bytes
            } else {
                &empty_bytes
            };
            block.buf[start..start + INODE_SIZE].copy_from_slice(bytes);
        }
        block.write_block(&disk)?;
    }

    for id in data_start..block_count {
        let mut block = new_block(id);
        if id == root_dir_block {
            block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + root_dir.len()]
                .copy_from_slice(&root_dir);
        }
        block.write_block(&disk)?;
    }
    Ok(())
}

pub fn reserve_inode(disk: &File, mode: u16, uid: u16, gid: u16) -> Result<u64, FileError> {
    let mut superblock = SuperBlock::deserialise(disk)?;
    let inode_id = find_free_inode(disk)?.ok_or(FileError::NoInodes)?;
    let mut inode = crate::inode::find_inode(disk, inode_id)?;
    let now = now_secs();
    inode.i_uid = uid;
    inode.i_gid = gid;
    inode.i_mode = mode;
    inode.i_extents = ExtentTreeNode::empty_root();
    inode.i_flags = 0;
    inode.i_blocks = 0;
    inode.i_generation = 0;
    inode.i_ctime = now;
    inode.i_mtime = now;
    inode.i_atime = now;
    inode.i_dtime = 0;
    inode.i_reserved = [0u8; 16];
    inode.i_size = 0;
    inode.i_links_count = 0;
    crate::inode::write_inode(disk, inode_id, &inode.serialise())?;
    mark_inode_used(disk, inode_id)?;
    superblock.free_inodes = superblock
        .free_inodes
        .checked_sub(1)
        .ok_or(FileError::NoInodes)?;
    disk.write_all_at(&superblock.serialise(), 0)
        .map_err(|_| FileError::WriteError)?;
    Ok(inode_id)
}
