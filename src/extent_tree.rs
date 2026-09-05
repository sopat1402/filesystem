use crate::block::{Block, BLOCK_SIZE};
use crate::file_errors::FileError;
use crate::inode::INODE_SIZE;

use std::fs::File;
use std::os::unix::prelude::FileExt;

pub const ENTRY_SIZE: usize = 12;
const NODE_HEADER_SIZE: usize = 8;
const BLOCK_HEADER_SIZE: usize = 12;
const NON_EXTENT_FIELDS_SIZE: usize = 64;
const INODE_EXTENTS_BUDGET: usize = INODE_SIZE - NON_EXTENT_FIELDS_SIZE;
pub const ROOT_MAX_ENTRIES: usize =(INODE_EXTENTS_BUDGET - NODE_HEADER_SIZE) / ENTRY_SIZE;
pub const BLOCK_MAX_ENTRIES: usize =(BLOCK_SIZE - BLOCK_HEADER_SIZE - NODE_HEADER_SIZE) / ENTRY_SIZE;

const TREE_MAGIC:u16=1234;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Extent {
    pub logical_start: u32,
    pub physical_start: u32,
    pub length: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IndexEntry {
    pub logical_start: u32,
    pub child_block: u32,
    pub _reserved: u32,
}

pub struct ExtentTreeNode{
    pub magic: u16,
    pub depth: u16,
    pub entry_count: u16,
    pub max_entries: u16,
    pub entries: Vec<[u8; ENTRY_SIZE]>,
}

impl ExtentTreeNode {
    pub fn serialise(&self) -> Vec<u8> {
        let mut buf = vec![0u8; NODE_HEADER_SIZE + self.entries.len() * ENTRY_SIZE];
        let mut offset = 0;
        buf[offset..offset + 2].copy_from_slice(&self.magic.to_le_bytes());
        offset += 2;
        buf[offset..offset + 2].copy_from_slice(&self.depth.to_le_bytes());
        offset += 2;
        buf[offset..offset + 2].copy_from_slice(&self.entry_count.to_le_bytes());
        offset += 2;
        buf[offset..offset + 2].copy_from_slice(&self.max_entries.to_le_bytes());
        offset += 2;
        for entry in &self.entries {
            buf[offset..offset + ENTRY_SIZE].copy_from_slice(entry);
            offset += ENTRY_SIZE;
        }
        buf
    }

    pub fn read_node(disk: &File,block_num: u32)->Result<Self,FileError>{
        let block = Block::deserialise(disk, block_num as usize)?;
        let mut offset = BLOCK_HEADER_SIZE;
        let magic = u16::from_le_bytes(block.buf[offset..offset + 2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset += 2;
        if magic!=TREE_MAGIC{
            return Err(FileError::CorruptedBlock);
        }
        let depth = u16::from_le_bytes(block.buf[offset..offset + 2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset += 2;
        let entry_count = u16::from_le_bytes(block.buf[offset..offset + 2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset += 2;
        let max_entries = u16::from_le_bytes(block.buf[offset..offset + 2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset += 2;
        if entry_count>max_entries || max_entries as usize>BLOCK_MAX_ENTRIES{
            return Err(FileError::CorruptedBlock);
        }
        let mut entries = vec![[0u8; ENTRY_SIZE]; entry_count as usize];
        for entry in &mut entries{
            entry.copy_from_slice(&block.buf[offset..offset + ENTRY_SIZE]);
            offset += ENTRY_SIZE;
        }
        Ok(Self{
            magic,
            depth,
            entry_count,
            max_entries,
            entries,
        })
    }
}

pub fn write_external_block(disk: &File,block: &mut Block,ext_block: &ExtentTreeNode) -> Result<(), FileError> {
    let serialised = ext_block.serialise();
    let end = BLOCK_HEADER_SIZE + serialised.len();
    block.buf[BLOCK_HEADER_SIZE..end].copy_from_slice(&serialised);
    block.serialise();
    disk.write_at(&block.buf, (block.id * BLOCK_SIZE) as u64).map_err(|_| FileError::WriteError)?;
    Ok(())
}
