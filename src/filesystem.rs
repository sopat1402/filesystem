use crate::inode::{Inode};
use crate::extent_tree::{ExtentTreeNode};
use crate::block::{SuperBlock,BlockHeader,Block,BLOCK_SIZE,NUM_BLOCKS,Flag};
use crate::file_errors::FileError;
use std::fs::File;
use std::os::unix::prelude::FileExt;

const TOTAL_INODES: usize = 10_000;
const INODE_SIZE: usize = 256;
const INODES_PER_BLOCK: usize = (BLOCK_SIZE - 14) / INODE_SIZE;
const INODE_TABLE_BLOCKS: usize = (TOTAL_INODES + INODES_PER_BLOCK - 1) / INODES_PER_BLOCK;
const MAGIC:u32=69_420;
const INODE_BITMAP_START: usize = 1;
const BLOCK_BITMAP_START: usize = 2;
const INODE_MAP_START: usize = 3;
const DATA_START: usize = INODE_MAP_START + INODE_TABLE_BLOCKS;
const TOTAL_SIZE: usize = NUM_BLOCKS * BLOCK_SIZE;

fn new_block(id:usize)->Block{
    let header=BlockHeader{lsn:0,checksum:0,flag:Flag::Clean};
    let buf=vec![0u8;BLOCK_SIZE];
    Block{header,id,buf}
}

fn new_superblock() -> SuperBlock {
    SuperBlock {
        header: BlockHeader {lsn:0,checksum:0,flag: Flag::Clean},
        magic: MAGIC,
        version: 1,
        total_size: TOTAL_SIZE as u32,
        block_size: BLOCK_SIZE as u16,
        inode_size: INODE_SIZE as u16,
        block_count: NUM_BLOCKS as u32,
        inode_count: TOTAL_INODES as u32,
        free_blocks: (NUM_BLOCKS - DATA_START) as u32,
        free_inodes: TOTAL_INODES as u32,
        inode_bitmap_start: INODE_BITMAP_START as u16,
        block_bitmap_start: BLOCK_BITMAP_START as u16,
        inode_map_start: INODE_MAP_START as u16,
        data_start: DATA_START as u16,
        state: 0,
        root_inode: 1,
    }
}

pub fn create_disk(path: &str) -> Result<(), FileError> {
    let disk = File::create(path).map_err(|_| FileError::WriteError)?;
    disk.set_len(TOTAL_SIZE as u64).map_err(|_| FileError::WriteError)?;
    let sb = new_superblock();
    disk.write_at(&sb.serialise(), 0).map_err(|_| FileError::WriteError)?;
    let mut inode_bitmap_block = new_block(INODE_BITMAP_START);
    inode_bitmap_block.serialise();
    disk.write_at(&inode_bitmap_block.buf, (INODE_BITMAP_START * BLOCK_SIZE) as u64).map_err(|_| FileError::WriteError)?;
    let mut block_bitmap_block = new_block(BLOCK_BITMAP_START);
    let mut buf:Vec<u8>=vec![0xFF;84];
    buf[83]=0x3F;
    block_bitmap_block.buf[12..96].copy_from_slice(&buf);
    block_bitmap_block.serialise();
    disk.write_at(&block_bitmap_block.buf, (BLOCK_BITMAP_START * BLOCK_SIZE) as u64)
        .map_err(|_| FileError::WriteError)?;
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
        i_extents: ExtentTreeNode {
            magic: 0,
            depth: 0,
            entry_count: 0,
            max_entries: 0,
            entries: vec![[0u8; 12]; 15],
        },
        i_generation: 0,
        i_reserved: [0u8; 4],
    };
    let empty_inode_bytes = empty_inode.serialise();
    for block_idx in 0..INODE_TABLE_BLOCKS {
        let mut block = new_block(INODE_MAP_START + block_idx);
        block.serialise();
        let inodes_in_this_block = if block_idx == INODE_TABLE_BLOCKS - 1 {
            TOTAL_INODES - block_idx * INODES_PER_BLOCK
        } else {
            INODES_PER_BLOCK
        };
        for slot in 0..inodes_in_this_block {
            let start = 14 + slot * INODE_SIZE;
            block.buf[start..start + INODE_SIZE].copy_from_slice(&empty_inode_bytes);
        }
        disk.write_at(&block.buf, (block.id * BLOCK_SIZE) as u64)
            .map_err(|_| FileError::WriteError)?;
    }
    Ok(())
}
