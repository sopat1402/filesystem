use crate::file_errors::FileError;
use std::fs::File;
use crate::block::{Block};
use crate::extent_tree::{ExtentTreeNode};
use crate::constants::*;

#[repr(C)]
pub struct Inode {
    pub i_mode: u16,
    pub i_uid: u32,
    pub i_size: u64,
    pub i_atime: u64,
    pub i_ctime: u64,
    pub i_mtime: u64,
    pub i_dtime: u64,
    pub i_gid: u32,
    pub i_links_count: u16,
    pub i_blocks: u64,
    pub i_flags: u32,
    pub i_extents: ExtentTreeNode,
    pub i_generation: u32,
    pub i_reserved: [u8; 12],
}

#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub ino: u64,        // raw, 0-based; the FUSE layer adds 1
    pub generation: u32,
    pub mode: u16,
    pub nlink: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blocks: u64,     // filesystem blocks; the FUSE layer multiplies by 8 for 512-byte units
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
}

impl Stat {
    pub fn from_inode(ino: u64, i: &Inode) -> Self {
        Self {
            ino,
            generation: i.i_generation,
            mode: i.i_mode,
            nlink: i.i_links_count,
            uid: i.i_uid,
            gid: i.i_gid,
            size: i.i_size,
            blocks: i.i_blocks,
            atime: i.i_atime,
            mtime: i.i_mtime,
            ctime: i.i_ctime,
        }
    }
}

pub fn stat(disk: &File, inode_id: u64) -> Result<Stat, FileError> {
    let inode = find_inode(disk, inode_id)?;
    Ok(Stat::from_inode(inode_id, &inode))
}

impl Inode {
    pub fn deserialise(buf: &[u8]) -> Result<Self, FileError> {
        if buf.len() < INODE_SIZE {
            return Err(FileError::CorruptedINode);
        }
        let mut offset: usize = 0;

        let i_mode = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
        let i_uid = u32::from_le_bytes(buf[offset..offset+4].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 4;
        let i_size = u64::from_le_bytes(buf[offset..offset+8].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 8;
        let i_atime = u64::from_le_bytes(buf[offset..offset+8].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 8;
        let i_ctime = u64::from_le_bytes(buf[offset..offset+8].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 8;
        let i_mtime = u64::from_le_bytes(buf[offset..offset+8].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 8;
        let i_dtime = u64::from_le_bytes(buf[offset..offset+8].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 8;
        let i_gid = u32::from_le_bytes(buf[offset..offset+4].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 4;
        let i_links_count = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
        let i_blocks = u64::from_le_bytes(buf[offset..offset+8].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 8;
        let i_flags = u32::from_le_bytes(buf[offset..offset+4].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 4;
        let magic = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
        if magic!=TREE_MAGIC{
            return Err(FileError::CorruptedINode);
        }
        let depth = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
        let entry_count = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
        let max_entries = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
        if entry_count>max_entries || entry_count as usize>ROOT_MAX_ENTRIES{
            return Err(FileError::CorruptedINode);
        }
        let mut entries = vec![[0u8; ENTRY_SIZE]; entry_count as usize];
        for slot in &mut entries {
            slot.copy_from_slice(&buf[offset..offset + ENTRY_SIZE]);
            offset += ENTRY_SIZE;
        }
        offset += (ROOT_MAX_ENTRIES - entry_count as usize) * ENTRY_SIZE;
        let i_extents = ExtentTreeNode {
            magic,
            depth,
            entry_count,
            max_entries,
            entries,
        };
        let i_generation = u32::from_le_bytes(buf[offset..offset+4].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 4;

        let mut i_reserved = [0u8; 12];
        i_reserved.copy_from_slice(&buf[offset..offset+12]);
        offset += 12;
        if offset!=INODE_SIZE{
            return Err(FileError::CorruptedINode);
        }
        Ok(Self {
            i_mode,
            i_uid,
            i_size,
            i_atime,
            i_ctime,
            i_mtime,
            i_dtime,
            i_gid,
            i_links_count,
            i_blocks,
            i_flags,
            i_extents,
            i_generation,
            i_reserved,
        })
    }

    pub fn serialise(&self) -> Vec<u8> {
        let mut buf = vec![0u8; INODE_SIZE];
        let mut offset: usize = 0;
        buf[offset..offset+2].copy_from_slice(&self.i_mode.to_le_bytes());
        offset += 2;
        buf[offset..offset+4].copy_from_slice(&self.i_uid.to_le_bytes());
        offset += 4;
        buf[offset..offset+8].copy_from_slice(&self.i_size.to_le_bytes());
        offset += 8;
        buf[offset..offset+8].copy_from_slice(&self.i_atime.to_le_bytes());
        offset += 8;
        buf[offset..offset+8].copy_from_slice(&self.i_ctime.to_le_bytes());
        offset += 8;
        buf[offset..offset+8].copy_from_slice(&self.i_mtime.to_le_bytes());
        offset += 8;
        buf[offset..offset+8].copy_from_slice(&self.i_dtime.to_le_bytes());
        offset += 8;
        buf[offset..offset+4].copy_from_slice(&self.i_gid.to_le_bytes());
        offset += 4;
        buf[offset..offset+2].copy_from_slice(&self.i_links_count.to_le_bytes());
        offset += 2;
        buf[offset..offset+8].copy_from_slice(&self.i_blocks.to_le_bytes());
        offset += 8;
        buf[offset..offset+4].copy_from_slice(&self.i_flags.to_le_bytes());
        offset += 4;
        buf[offset..offset+2].copy_from_slice(&self.i_extents.magic.to_le_bytes());
        offset += 2;
        buf[offset..offset+2].copy_from_slice(&self.i_extents.depth.to_le_bytes());
        offset += 2;
        buf[offset..offset+2].copy_from_slice(&self.i_extents.entry_count.to_le_bytes());
        offset += 2;
        buf[offset..offset+2].copy_from_slice(&self.i_extents.max_entries.to_le_bytes());
        offset += 2;
        for i in 0..ROOT_MAX_ENTRIES {
            if i < self.i_extents.entries.len() {
                buf[offset..offset + ENTRY_SIZE].copy_from_slice(&self.i_extents.entries[i]);
            }
            offset += ENTRY_SIZE;
        }
        buf[offset..offset+4].copy_from_slice(&self.i_generation.to_le_bytes());
        offset += 4;
        buf[offset..offset+12].copy_from_slice(&self.i_reserved);
        offset += 12;
        debug_assert_eq!(offset, INODE_SIZE);
        buf
    }
}

pub fn find_inode(disk:&File,inode_id:u64)->Result<Inode,FileError>{
    let superblock=crate::block::SuperBlock::deserialise(disk)?;
    if inode_id >= superblock.inode_count {
        return Err(FileError::CorruptedINode);
    }
    let block_id=superblock.inode_map_start+inode_id/INODES_PER_BLOCK as u64;
    let offset=BLOCK_HEADER_SIZE+((inode_id%INODES_PER_BLOCK as u64)*INODE_SIZE as u64) as usize;
    let block=Block::deserialise(disk,block_id)?;
    let inode=Inode::deserialise(&block.buf[offset..offset+INODE_SIZE])?;
    Ok(inode)
}

pub fn write_inode(disk:&File, inode_id:u64, buf:&[u8])->Result<(),FileError>{
    let superblock=crate::block::SuperBlock::deserialise(disk)?;
    if inode_id >= superblock.inode_count || buf.len() != INODE_SIZE {
        return Err(FileError::CorruptedINode);
    }
    let block_id=superblock.inode_map_start+inode_id/INODES_PER_BLOCK as u64;
    let offset=BLOCK_HEADER_SIZE+((inode_id%INODES_PER_BLOCK as u64)*INODE_SIZE as u64) as usize;
    let mut block=Block::deserialise(disk,block_id)?;
    block.buf[offset..offset+INODE_SIZE].copy_from_slice(buf);
    block.write_block(disk)?;
    Ok(())
}
