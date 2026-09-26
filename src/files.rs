use crate::constants::*;
use crate::file_errors::FileError;
use crate::directories::{read_dir,lexer,add_dirent,is_dir};
use crate::inode::find_inode;

pub struct File{
    inode   :   usize,
    offset  :   u32,
    flags   :   u32,
}

impl File{
    pub fn open(disk:&std::fs::File, path:&String, cwd:usize, flags:u32) -> Result<Self, FileError> {
        let tokens = lexer(path);
        if tokens.len() == 0 {
            return Err(FileError::NameNotFound);
        }
        let filename = tokens[tokens.len() - 1].clone();
        let mut dir_inode = if path.starts_with('/') { ROOT_INODE_NUM } else { cwd };
        for component in &tokens[..tokens.len() - 1] {
            let dirents = read_dir(disk, dir_inode)?;
            dir_inode = dirents.iter()
                .find(|(name, _)| name == component)
                .map(|(_, inode)| *inode)
                .ok_or(FileError::NameNotFound)?;
        }
        let dirents = read_dir(disk, dir_inode)?;
        let existing = dirents.iter().find(|(name, _)| *name == filename).map(|(_, id)| *id);
        let file_inode = match existing {
            Some(id) => {
                if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                    return Err(FileError::NameExists);
                }
                if flags & O_DIRECTORY != 0 {
                    let node = find_inode(disk, id)?;
                    if !is_dir(node.i_mode) {
                        return Err(FileError::NotDirectory);
                    }
                }
                id
            }
            None => {
                if flags & O_CREAT == 0 {
                    return Err(FileError::NameNotFound);
                }
                add_dirent(disk, dir_inode, filename, S_IFREG | 0o644, 0, 0)?
            }
        };
        Ok(Self { inode: file_inode, offset: 0, flags })
    }

    pub fn read(&mut self, disk:&std::fs::File, length:usize) -> Result<Vec<u8>, FileError> {
        let inode = find_inode(disk, self.inode)?;
        if is_dir(inode.i_mode) {
            return Err(FileError::NotFile);
        }
        if (self.offset as u64) >= inode.i_size {
            return Ok(Vec::new());
        }
        let payload_size = (BLOCK_SIZE - BLOCK_HEADER_SIZE) as u32;
        let end_offset = ((self.offset as u64 + length as u64).min(inode.i_size)) as u32;
        let read_len = (end_offset - self.offset) as usize;
        let start_block = self.offset / payload_size;
        let end_block = (end_offset + payload_size - 1) / payload_size;
        let extents = crate::extent_tree::range_lookup(disk, &inode.i_extents, start_block, end_block)?;
        let mut buf: Vec<u8> = Vec::new();
        for extent in &extents {
            for (i, block_num) in (extent.physical_start..extent.physical_start + extent.length).enumerate() {
                let logical_block = extent.logical_start + i as u32;
                if logical_block < start_block || logical_block >= end_block {
                    continue;
                }
                let block = crate::block::Block::deserialise(disk, block_num as usize)?;
                buf.extend_from_slice(&block.buf[BLOCK_HEADER_SIZE..]);
            }
        }
        let clip_start = (self.offset - start_block * payload_size) as usize;
        let result = buf[clip_start..clip_start + read_len].to_vec();
        self.offset = end_offset;
        Ok(result)
    }
}
