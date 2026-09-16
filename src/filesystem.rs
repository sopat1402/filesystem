use crate::inode::{Inode};
use crate::extent_tree::{ExtentTreeNode,Extent};
use crate::block::{SuperBlock,BlockHeader,Block,Flag};
use crate::bitmaps::{mark_blocks_used,mark_inode_used,find_free_inode};
use crate::file_errors::FileError;
use std::fs::File;
use std::time::{SystemTime,UNIX_EPOCH};
use std::os::unix::prelude::FileExt;
use crate::constants::*;


fn new_block(id: usize) -> Block {
    let header = BlockHeader { lsn: 0, checksum: 0, flag: Flag::Clean };
    let buf = vec![0u8; BLOCK_SIZE];
    Block { header, id, buf }
}

fn empty_extent_root() -> ExtentTreeNode {
    ExtentTreeNode {
        magic: TREE_MAGIC,
        depth: 0,
        entry_count: 0,
        max_entries: ROOT_MAX_ENTRIES as u16,
        entries: vec![[0u8; 12]; ROOT_MAX_ENTRIES],
    }
}

fn new_superblock() -> SuperBlock {
    SuperBlock {
        header: BlockHeader { lsn: 0, checksum: 0, flag: Flag::Clean },
        magic: MAGIC,
        version: 1,
        total_size: TOTAL_SIZE as u32,
        block_size: BLOCK_SIZE as u16,
        inode_size: INODE_SIZE as u16,
        block_count: NUM_BLOCKS as u32,
        inode_count: TOTAL_INODES as u32,
        free_blocks: (NUM_BLOCKS - DATA_START) as u32,
        free_inodes: (TOTAL_INODES - 1) as u32,
        inode_bitmap_start: INODE_BITMAP_START as u16,
        block_bitmap_start: BLOCK_BITMAP_START as u16,
        inode_map_start: INODE_MAP_START as u16,
        data_start: DATA_START as u16,
        state: 0,
        root_inode: ROOT_INODE_NUM as u32,
    }
}

pub fn create_disk(path: &str) -> Result<(), FileError> {
    let disk = File::create(path).map_err(|_| FileError::WriteError)?;
    disk.set_len(TOTAL_SIZE as u64).map_err(|_| FileError::WriteError)?;
    let sb = new_superblock();
    disk.write_at(&sb.serialise(), 0).map_err(|_| FileError::WriteError)?;
    let mut inode_bitmap_block = new_block(INODE_BITMAP_START);
    let mut inode_bitmap_buf = vec![0u8; (TOTAL_INODES + 7) / 8];
    mark_inode_used(&mut inode_bitmap_buf, ROOT_INODE_NUM);
    inode_bitmap_block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + inode_bitmap_buf.len()].copy_from_slice(&inode_bitmap_buf);
    inode_bitmap_block.serialise();
    disk.write_at(&inode_bitmap_block.buf, (INODE_BITMAP_START * BLOCK_SIZE) as u64)
        .map_err(|_| FileError::WriteError)?;
    let mut block_bitmap_block = new_block(BLOCK_BITMAP_START);
    let block_bitmap_bytes = (DATA_START + 7) / 8;
    let mut block_bitmap_buf = vec![0u8; block_bitmap_bytes];
    mark_blocks_used(&mut block_bitmap_buf, &[Extent {
        logical_start: 0,
        physical_start: 0,
        length: DATA_START as u32,
    }]);
    block_bitmap_block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + block_bitmap_buf.len()].copy_from_slice(&block_bitmap_buf);
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
        i_extents: empty_extent_root(),
        i_generation: 0,
        i_reserved: [0u8; 4],
    };
    let empty_inode_bytes = empty_inode.serialise();
    let curr_time = SystemTime::now();
    let secs: u64 = curr_time.duration_since(UNIX_EPOCH).unwrap().as_secs();
    let root_inode = Inode {
        i_mode: 0o040755,
        i_uid: 0,
        i_size: 0,
        i_atime: secs,
        i_ctime: secs,
        i_mtime: secs,
        i_dtime: 0,
        i_gid: 0,
        i_links_count: 2,
        i_blocks: 0,
        i_flags: 0,
        i_extents: empty_extent_root(),
        i_generation: 0,
        i_reserved: [0u8; 4],
    };
    let root_inode_bytes = root_inode.serialise();

    for block_idx in 0..INODE_TABLE_BLOCKS {
        let mut block = new_block(INODE_MAP_START + block_idx);
        block.serialise();
        let inodes_in_this_block = if block_idx == INODE_TABLE_BLOCKS - 1 {
            TOTAL_INODES - block_idx * INODES_PER_BLOCK
        } else {
            INODES_PER_BLOCK
        };
        for slot in 0..inodes_in_this_block {
            let start = BLOCK_HEADER_SIZE + slot * INODE_SIZE;
            let inode_num = block_idx * INODES_PER_BLOCK + slot;
            let bytes = if inode_num == ROOT_INODE_NUM { &root_inode_bytes } else { &empty_inode_bytes };
            block.buf[start..start + INODE_SIZE].copy_from_slice(bytes);
        }
        disk.write_at(&block.buf, (block.id * BLOCK_SIZE) as u64)
            .map_err(|_| FileError::WriteError)?;
    }

    Ok(())
}

pub fn reserve_inode(disk : &File, mode : u16, uid : u16, gid : u16)->Result<usize,FileError>{
    let mut superblock=SuperBlock::deserialise(disk)?;
    let bitmap_block=superblock.inode_bitmap_start;
    let mut bitmap_block=Block::deserialise(disk,bitmap_block as usize)?;
    let inode_id=find_free_inode(&bitmap_block.buf[BLOCK_HEADER_SIZE..]);
    let inode_id=match inode_id{
        Some(t)=>t,
        None=>return Err(FileError::NoInodes),
    };
    let mut inode=crate::inode::find_inode(disk,inode_id)?;
    inode.i_uid=uid;
    inode.i_gid=gid;
    inode.i_mode=mode;
    inode.i_extents=empty_extent_root();
    inode.i_flags=0;
    inode.i_blocks=0;
    inode.i_generation=0;
    let curr_time=SystemTime::now();
    let secs: u64 = curr_time.duration_since(UNIX_EPOCH).unwrap().as_secs();
    inode.i_ctime=secs;
    inode.i_dtime=0;
    inode.i_reserved=[0u8;4];
    inode.i_mtime=secs;
    inode.i_atime=secs;
    inode.i_size=0;
    inode.i_links_count=0;
    let buf=inode.serialise();
    crate::inode::write_inode(disk,inode_id,&buf)?;
    mark_inode_used(&mut bitmap_block.buf[BLOCK_HEADER_SIZE..],inode_id);
    bitmap_block.write_block(disk)?;
    superblock.free_inodes-=1;
    let buf=superblock.serialise();
    disk.write_all_at(&buf,0).map_err(|_| FileError::WriteError)?;
    Ok(inode_id)
}




