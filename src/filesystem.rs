use crate::inode::{Inode};
use crate::extent_tree::{ExtentTreeNode,Extent};
use crate::block::{SuperBlock,BlockHeader,Block,Flag};
use crate::bitmaps::{mark_blocks_used,mark_inode_used,find_free_inode};
use crate::file_errors::FileError;
use std::fs::File;
use std::time::{SystemTime,UNIX_EPOCH};
use std::os::unix::prelude::FileExt;
use crate::constants::*;

pub struct Filesystem{
    pub disk : std::fs::File,
}

impl Filesystem{
    pub fn open(path:String)->Result<Self,FileError>{
        let disk=std::fs::File::options()
            .read(true)
            .write(true)
            .open(path).map_err(|_| FileError::ReadError)?;
        Ok(Self{disk})
    }
}

fn new_block(id: u64) -> Block {
    let mut header = BlockHeader { lsn: 0, checksum: 0, flag: Flag::Clean };
    let buf = vec![0u8; BLOCK_SIZE];
    let checksum=crate::crc32::crc32(&buf[BLOCK_HEADER_SIZE..]);
    header.checksum=checksum;
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

fn new_superblock(
    disk_size:u64,
    block_count:u64,
    total_inodes:u64,
    inode_bitmap_start:u64,
    block_bitmap_start:u64,
    inode_map_start:u64,
    data_start:u64
) -> SuperBlock {

    SuperBlock {
        header: BlockHeader { lsn: 0, checksum: 0, flag: Flag::Clean },
        magic: MAGIC,
        version: 1,
        total_size: disk_size,
        block_size: BLOCK_SIZE as u16,
        inode_size: INODE_SIZE as u16,
        block_count: block_count,
        inode_count: total_inodes,
        free_blocks: block_count - data_start,
        free_inodes: (total_inodes - 1),
        inode_bitmap_start: inode_bitmap_start,
        block_bitmap_start: block_bitmap_start,
        inode_map_start: inode_map_start,
        data_start: data_start,
        state: 0,
        root_inode: ROOT_INODE_NUM as u32,
    }
}

pub fn create_disk(path: &str,disk_size:u64,inode_ratio:u64) -> Result<(), FileError> {
    let disk = File::create(path).map_err(|_| FileError::WriteError)?;
    disk.set_len(disk_size)
        .map_err(|_| FileError::WriteError)?;

    //calculations
    let block_count=if disk_size%(BLOCK_SIZE as u64)==0{
        disk_size/BLOCK_SIZE as u64
    }else{
        disk_size/BLOCK_SIZE as u64 +1
    };
    let total_inodes=inode_ratio*block_count;

    let inode_bitmap_start:u64=1;
    let num_inode_bitmap_bytes=if total_inodes%8==0{
        total_inodes/8
    }else{
        total_inodes/8+1
    };
    let num_inode_bitmap_blocks=if num_inode_bitmap_bytes%(BLOCK_SIZE-BLOCK_HEADER_SIZE) as u64 ==0{
        num_inode_bitmap_bytes/(BLOCK_SIZE-BLOCK_HEADER_SIZE) as u64
    }else{
        num_inode_bitmap_bytes/(BLOCK_SIZE-BLOCK_HEADER_SIZE) as u64 +1
    };

    let block_bitmap_start=inode_bitmap_start+num_inode_bitmap_blocks;
    let num_block_bitmap_bytes=if block_count%8==0{
        block_count/8
    }else{
        block_count/8+1
    };
    let num_block_bitmap_blocks=if num_block_bitmap_bytes%(BLOCK_SIZE-BLOCK_HEADER_SIZE) as u64 ==0{
        num_block_bitmap_bytes/(BLOCK_SIZE-BLOCK_HEADER_SIZE) as u64
    }else{
        num_block_bitmap_bytes/(BLOCK_SIZE-BLOCK_HEADER_SIZE) as u64 +1
    };

    let inode_map_start=block_bitmap_start+num_block_bitmap_blocks;
    let inode_map_bytes:u64=total_inodes*(INODE_SIZE as u64);
    let inode_map_blocks=if inode_map_bytes%(BLOCK_SIZE-BLOCK_HEADER_SIZE) as u64 == 0{
        inode_map_bytes/(BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64
    }else{
        inode_map_bytes/(BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64 + 1
    };

    let data_start=inode_map_start+inode_map_blocks;

    //make superblock
    let mut sb = new_superblock(
        disk_size,
        block_count,
        total_inodes,
        inode_bitmap_start,
        block_bitmap_start,
        inode_map_start,
        data_start
    );
    let mut sb_buf = sb.serialise();
    sb.header.checksum = crate::crc32::crc32(&sb_buf[BLOCK_HEADER_SIZE..]);
    sb_buf = sb.serialise();
    disk.write_all_at(&sb_buf, 0)
        .map_err(|_| FileError::WriteError)?;

    //inode bitmap creation part. needs rewrite. make the root inode be written in as 1 where
    //needed. don't call mark_inode_used.
    let mut curr_block:u64=inode_bitmap_start;
    let mut first_block=new_block(curr_block);
    first_block.buf[BLOCK_HEADER_SIZE]=0x01;
    first_block.write_block(&disk)?;
    curr_block+=1;
    for block_id in curr_block..inode_bitmap_start+num_inode_bitmap_blocks{
        let mut block=new_block(block_id);
        block.write_block(&disk)?;
    }

    //block bitmap creation.
    curr_block = block_bitmap_start;
    let total_reserved_blocks = 1 + num_inode_bitmap_blocks + num_block_bitmap_blocks + inode_map_blocks;
    let reserved_block_bytes=if total_reserved_blocks%8==0{
        total_reserved_blocks/8
    }else{
        total_reserved_blocks/8+1
    };
    let blocks_needed=if reserved_block_bytes%(BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64==0{
        reserved_block_bytes/(BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64
    }else{
        reserved_block_bytes/(BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64 + 1
    };
    let mut remaining=reserved_block_bytes;
    for _ in 0..blocks_needed{
        let mut block=new_block(curr_block);
        let to_write=if remaining>(BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64{
            let ret=remaining-(BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
            remaining-=(BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
            ret
        }else{
            let ret=remaining;
            remaining=0;
            ret
        };
        let mut write_buf=vec![0u8;BLOCK_SIZE - BLOCK_HEADER_SIZE];
        write_buf[0..to_write as usize].copy_from_slice(&vec![0xFFu8;to_write as usize]);
        if remaining==0 && total_reserved_blocks%8 !=0 {
            let valid_bits=total_reserved_blocks%8;
            write_buf[to_write as usize-1]=(1u8 << valid_bits) -1;
        }
        block.buf[BLOCK_HEADER_SIZE..].copy_from_slice(&write_buf);
        block.write_block(&disk)?;
        curr_block+=1;
    }
    for block_id in curr_block..block_bitmap_start+num_block_bitmap_blocks{
        let mut block=new_block(block_id);
        block.write_block(&disk)?;
    }

    //inode map creation
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
    let secs: u64 = curr_time
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
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
    for block_idx in 0..inode_map_blocks {
        let mut block = new_block(inode_map_start + block_idx);
        let inodes_in_this_block = if block_idx == inode_map_blocks - 1 {
            total_inodes - block_idx * INODES_PER_BLOCK as u64
        } else {
            INODES_PER_BLOCK as u64
        };
        for slot in 0..inodes_in_this_block {
            let start = BLOCK_HEADER_SIZE as u64 + slot * INODE_SIZE as u64;
            let inode_num = block_idx * INODES_PER_BLOCK as u64 + slot;
            let bytes = if inode_num == ROOT_INODE_NUM as u64{
                &root_inode_bytes
            } else {
                &empty_inode_bytes
            };
            block.buf[start as usize..start as usize + INODE_SIZE]
                .copy_from_slice(bytes);
        }
        block.serialise();
        disk.write_all_at(
            &block.buf,
            block.id * BLOCK_SIZE as u64,
        )
        .map_err(|_| FileError::WriteError)?;
    }
    for id in data_start..block_count {
        let mut block = new_block(id);
        block.serialise();
        disk.write_all_at(&block.buf, id * BLOCK_SIZE as u64)
            .map_err(|_| FileError::WriteError)?;
    }
    Ok(())
}

//needs rewrite. come back later after fixed bitmap functions.
pub fn reserve_inode(disk : &File, mode : u16, uid : u16, gid : u16)->Result<usize,FileError>{
    let mut superblock=SuperBlock::deserialise(disk)?;
    //fix from here
    let bitmap_block=superblock.inode_bitmap_start;
    let mut bitmap_block=Block::deserialise(disk,bitmap_block as usize)?;
    let inode_bitmap_len = (TOTAL_INODES + 7) / 8;
    let inode_id=find_free_inode(
        &bitmap_block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + inode_bitmap_len],
    );
    //to here
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
