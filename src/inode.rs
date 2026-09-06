use crate::file_errors::FileError;
use crate::extent_tree::{ExtentTreeNode, ROOT_MAX_ENTRIES,ENTRY_SIZE,TREE_MAGIC};

pub const INODE_SIZE: usize = 256;
pub const TOTAL_INODES: usize = 10_000;

pub struct Inode {
    pub i_mode: u16,
    pub i_uid: u16,
    pub i_size: u64,
    pub i_atime: u64,
    pub i_ctime: u64,
    pub i_mtime: u64,
    pub i_dtime: u64,
    pub i_gid: u16,
    pub i_links_count: u16,
    pub i_blocks: u64,
    pub i_flags: u32,
    pub i_extents: ExtentTreeNode,
    pub i_generation: u32,
    pub i_reserved: [u8; 4],
}


impl Inode {
    pub fn deserialise(buf: &[u8]) -> Result<Self, FileError> {
        let mut offset: usize = 0;

        let i_mode = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
        let i_uid = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
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
        let i_gid = u16::from_le_bytes(buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset += 2;
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

        let mut i_reserved = [0u8; 4];
        i_reserved.copy_from_slice(&buf[offset..offset+4]);
        offset += 4;
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
        buf[offset..offset+2].copy_from_slice(&self.i_uid.to_le_bytes());
        offset += 2;
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
        buf[offset..offset+2].copy_from_slice(&self.i_gid.to_le_bytes());
        offset += 2;
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
        buf[offset..offset+4].copy_from_slice(&self.i_reserved);
        offset += 4;
        debug_assert_eq!(offset, INODE_SIZE);
        buf
    }
}
