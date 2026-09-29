use crate::constants::*;
use crate::file_errors::FileError;
use crate::directories::{read_dir,lexer,add_dirent,is_dir};
use crate::inode::{find_inode,write_inode};
use crate::extent_tree::{insert_extent, lookup_extent, Extent};
use crate::block::{Block,SuperBlock};
use crate::bitmaps::{find_blocks,mark_blocks_used};
use crate::filesystem::Filesystem;
use std::os::unix::prelude::FileExt;

pub struct File<'a>{
    fs      :   &'a Filesystem,
    inode   :   usize,
    offset  :   u32,
    flags   :   u32,
}

fn allocate_block(disk: &std::fs::File, superblock: &mut SuperBlock) -> Result<u32, FileError> {
    let mut bitmap_block = Block::deserialise(disk, superblock.block_bitmap_start as usize)?;
    let block_bitmap_len = (NUM_BLOCKS + 7) / 8;
    let mut buf = bitmap_block.buf
        [BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + block_bitmap_len]
        .to_vec();
    let new_extent = find_blocks(&buf, 1);
    if new_extent.is_empty() {
        return Err(FileError::NoMoreBlocks);
    }
    mark_blocks_used(&mut buf, &new_extent);
    bitmap_block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + block_bitmap_len]
        .copy_from_slice(&buf);
    bitmap_block.write_block(disk)?;
    superblock.free_blocks = superblock
        .free_blocks
        .checked_sub(1)
        .ok_or(FileError::NoMoreBlocks)?;
    Ok(new_extent[0].physical_start)
}

impl<'a> File<'a>{
    pub fn open(fs:&'a Filesystem, path:&String, cwd:usize, flags:u32) -> Result<Self, FileError> {
        let disk = &fs.disk;
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
        Ok(Self { fs, inode: file_inode, offset: 0, flags })
    }

    pub fn read(&mut self, length:usize) -> Result<Vec<u8>, FileError> {
        let disk = &self.fs.disk;
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
                let block = Block::deserialise(disk, block_num as usize)?;
                buf.extend_from_slice(&block.buf[BLOCK_HEADER_SIZE..]);
            }
        }
        let clip_start = (self.offset - start_block * payload_size) as usize;
        let result = buf[clip_start..clip_start + read_len].to_vec();
        self.offset = end_offset;
        Ok(result)
    }

    pub fn ftell(&self)->u32{
        self.offset
    }

    pub fn fseek(&mut self, pos:u32) -> Result<(), FileError> {
        let disk = &self.fs.disk;
        let inode = find_inode(disk, self.inode)?;
        if pos as u64 > inode.i_size {
            return Err(FileError::Overflow);
        }
        self.offset = pos;
        Ok(())
    }

    pub fn write(&mut self, buf:&[u8], rel_offset:u32) -> Result<usize, FileError> {
        let disk = &self.fs.disk;
        let mut inode = find_inode(disk, self.inode)?;
        if is_dir(inode.i_mode) {
            return Err(FileError::NotFile);
        }
        let payload_size = (BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
        let base_offset = if self.flags & O_APPEND != 0 {
            inode.i_size
        } else {
            (self.offset as u64)
                .checked_add(rel_offset as u64)
                .ok_or(FileError::Overflow)?
        };
        let end_offset = base_offset
            .checked_add(buf.len() as u64)
            .ok_or(FileError::Overflow)?;
        if end_offset > u32::MAX as u64 {
            return Err(FileError::Overflow);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let old_size = inode.i_size;
        let write_begin = base_offset.min(old_size);
        let first_block = write_begin / payload_size;
        let last_block = (end_offset - 1) / payload_size;
        let mut superblock = SuperBlock::deserialise(disk)?;

        for logical_block in first_block..=last_block {
            let block_start = logical_block * payload_size;
            let block_end = block_start + payload_size;
            let mut physical_block = match lookup_extent(
                disk,
                &inode.i_extents,
                logical_block as u32,
            )? {
                Some(extent) => Some(
                    extent.physical_start
                        + (logical_block as u32 - extent.logical_start),
                ),
                None => None,
            };
            if physical_block.is_none() {
                physical_block = Some(allocate_block(disk, &mut superblock)?);
            }
            let physical_block = physical_block.unwrap();
            let mut block = Block::deserialise(disk, physical_block as usize)?;

            let touched_start = write_begin.max(block_start);
            let touched_end = end_offset.min(block_end);
            let local_start = (touched_start - block_start) as usize;
            let local_end = (touched_end - block_start) as usize;
            if base_offset > old_size {
                let hole_start = old_size.max(block_start);
                let hole_end = base_offset.min(block_end);
                if hole_start < hole_end {
                    block.buf[BLOCK_HEADER_SIZE + (hole_start - block_start) as usize
                        ..BLOCK_HEADER_SIZE + (hole_end - block_start) as usize]
                        .fill(0);
                }
            }

            let data_start = touched_start.max(base_offset);
            let data_end = touched_end;
            if data_start < data_end {
                let src_start = (data_start - base_offset) as usize;
                let src_end = (data_end - base_offset) as usize;
                block.buf[BLOCK_HEADER_SIZE + (data_start - block_start) as usize
                    ..BLOCK_HEADER_SIZE + (data_end - block_start) as usize]
                    .copy_from_slice(&buf[src_start..src_end]);
            }
            debug_assert!(local_start <= local_end);
            debug_assert!(BLOCK_HEADER_SIZE + local_end <= BLOCK_SIZE);
            block.write_block(disk)?;

            if lookup_extent(disk, &inode.i_extents, logical_block as u32)?.is_none() {
                let new_extent = Extent {
                    logical_start: logical_block as u32,
                    physical_start: physical_block,
                    length: 1,
                };
                let mut alloc_block = || allocate_block(disk, &mut superblock);
                insert_extent(
                    disk,
                    &mut inode.i_extents,
                    new_extent,
                    &mut alloc_block,
                )?;
            }
        }
        inode.i_size = inode.i_size.max(end_offset);
        write_inode(disk, self.inode, &inode.serialise())?;
        let sb_buf = superblock.serialise();
        disk.write_all_at(&sb_buf, 0)
            .map_err(|_| FileError::WriteError)?;
        self.offset = end_offset as u32;
        Ok(buf.len())
    }
}
