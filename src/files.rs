use crate::constants::*;
use crate::file_errors::FileError;
use crate::directories::{read_dir, lexer, add_dirent, is_dir, free_data_extents};
use crate::inode::{find_inode, write_inode};
use crate::extent_tree::{insert_extent, range_lookup, delete_extent_range, Extent};
use crate::block::{Block, BlockHeader, Flag, SuperBlock};
use crate::bitmaps::{allocate_block, find_blocks, mark_block_free, mark_blocks_free, mark_blocks_used};
use crate::filesystem::Filesystem;
use std::os::unix::prelude::FileExt;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct File<'a> {
    fs: &'a Filesystem,
    inode: u64,
    offset: u64,
    flags: u32,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn physical_of(extents: &[Extent], lb: u64) -> Option<u64> {
    let idx = extents.partition_point(|e| e.logical_start <= lb);
    if idx == 0 {
        return None;
    }
    let e = &extents[idx - 1];
    if lb < e.logical_end() {
        e.physical_start.checked_add(lb - e.logical_start)
    } else {
        None
    }
}

fn restore_blocks(disk: &std::fs::File, originals: &[(u64, Vec<u8>)]) {
    for (id, buf) in originals {
        let mut block = Block {
            id: *id,
            header: BlockHeader { lsn: 0, checksum: 0, flag: Flag::Clean },
            buf: buf.clone(),
        };
        let _ = block.write_block(disk);
    }
}

fn truncate_to_zero(disk: &std::fs::File, inode_id: u64) -> Result<(), FileError> {
    let mut inode = find_inode(disk, inode_id)?;
    let mut superblock = SuperBlock::deserialise(disk)?;
    let freed = {
        let mut free_block = |block_id: u64| -> Result<(), FileError> {
            mark_block_free(disk, block_id)?;
            superblock.free_blocks += 1;
            Ok(())
        };
        delete_extent_range(disk, &mut inode.i_extents, 0, u64::MAX, &mut free_block)?
    };
    free_data_extents(disk, &mut superblock, &freed)?;
    let now = now_secs();
    inode.i_size = 0;
    inode.i_blocks = 0;
    inode.i_mtime = now;
    inode.i_ctime = now;
    write_inode(disk, inode_id, &inode.serialise())?;
    disk.write_all_at(&superblock.serialise(), 0)
        .map_err(|_| FileError::WriteError)?;
    Ok(())
}

impl<'a> File<'a> {

    pub fn open(
        fs: &'a Filesystem,
        path: &String,
        cwd: u64,
        flags: u32,
    ) -> Result<Self, FileError> {
        let access = flags & O_ACCMODE;
        if access == O_ACCMODE {
            return Err(FileError::InvalidFlags);
        }
        if flags & O_TRUNC != 0 && access == O_RDONLY {
            return Err(FileError::InvalidFlags);
        }
        if flags & O_CREAT != 0 && flags & O_DIRECTORY != 0 {
            return Err(FileError::InvalidFlags);
        }

        let disk = &fs.disk;
        let tokens = lexer(path);

        let trailing_slash = path.ends_with('/')
            && path
                .chars()
                .rev()
                .skip(1)
                .take_while(|c| *c == '\\')
                .count()
                % 2
                == 0;

        let (file_inode, already_exists) = if tokens.is_empty() {
            if path.is_empty() || !path.starts_with('/') {
                return Err(FileError::NameNotFound);
            }
            (ROOT_INODE_NUM as u64, true)
        } else {
            let filename = tokens[tokens.len() - 1].clone();
            let mut dir_inode = if path.starts_with('/') {
                ROOT_INODE_NUM as u64
            } else {
                cwd
            };

            for component in &tokens[..tokens.len() - 1] {
                let dirents = read_dir(disk, dir_inode)?;
                dir_inode = dirents
                    .iter()
                    .find(|(name, _)| name == component)
                    .map(|(_, inode)| *inode)
                    .ok_or(FileError::NameNotFound)?;
            }

            let dirents = read_dir(disk, dir_inode)?;
            let existing = dirents
                .iter()
                .find(|(name, _)| *name == filename)
                .map(|(_, id)| *id);

            match existing {
                Some(id) => (id, true),
                None => {
                    if trailing_slash {
                        return Err(FileError::NotDirectory);
                    }
                    if flags & O_CREAT == 0 {
                        return Err(FileError::NameNotFound);
                    }
                    (
                        add_dirent(
                            disk,
                            dir_inode,
                            filename,
                            S_IFREG | 0o644,
                            0,
                            0,
                        )?,
                        false,
                    )
                }
            }
        };
        if already_exists && flags & O_CREAT != 0 && flags & O_EXCL != 0 {
            return Err(FileError::NameExists);
        }
        let node = find_inode(disk, file_inode)?;
        let node_is_dir = is_dir(node.i_mode);

        if (flags & O_DIRECTORY != 0 || trailing_slash) && !node_is_dir {
            return Err(FileError::NotDirectory);
        }
        if node_is_dir && access != O_RDONLY {
            return Err(FileError::NotFile);
        }
        if flags & O_TRUNC != 0 && !node_is_dir {
            truncate_to_zero(disk, file_inode)?;
        }
        Ok(Self {
            fs,
            inode: file_inode,
            offset: 0,
            flags,
        })
    }

    pub fn read(&mut self, length: usize) -> Result<Vec<u8>, FileError> {
        if self.flags & O_ACCMODE == O_WRONLY {
            return Err(FileError::PermissionDenied);
        }
        let disk = &self.fs.disk;
        let superblock = SuperBlock::deserialise(disk)?;
        let mut inode = find_inode(disk, self.inode)?;
        if is_dir(inode.i_mode) {
            return Err(FileError::NotFile);
        }
        if inode.i_size > superblock.total_size {
            return Err(FileError::CorruptedINode);
        }
        if self.offset >= inode.i_size || length == 0 {
            return Ok(Vec::new());
        }

        let payload = (BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
        let end_offset = self.offset.saturating_add(length as u64).min(inode.i_size);
        let read_len = (end_offset - self.offset) as usize;
        let start_block = self.offset / payload;
        let end_block = (end_offset - 1) / payload + 1; // exclusive, overflow-safe
        let extents = range_lookup(disk, &inode.i_extents, start_block, end_block)?;

        let mut result = vec![0u8; read_len];
        for e in &extents {
            let first = start_block.max(e.logical_start);
            let last = end_block.min(e.logical_end());
            for lb in first..last {
                let pb = e
                    .physical_start
                    .checked_add(lb - e.logical_start)
                    .ok_or(FileError::CorruptedINode)?;
                let block = Block::deserialise(disk, pb)?;
                let block_start = lb * payload; // lb < end_block, cannot overflow
                let s = self.offset.max(block_start);
                let en = end_offset.min(block_start.saturating_add(payload));
                let dst = (s - self.offset) as usize..(en - self.offset) as usize;
                let src = BLOCK_HEADER_SIZE + (s - block_start) as usize
                    ..BLOCK_HEADER_SIZE + (en - block_start) as usize;
                result[dst].copy_from_slice(&block.buf[src]);
            }
        }

        self.offset = end_offset;
        inode.i_atime = now_secs();
        write_inode(disk, self.inode, &inode.serialise())?;
        Ok(result)
    }

    pub fn ftell(&self) -> u64 {
        self.offset
    }

    pub fn fseek(&mut self, pos: u64) -> Result<(), FileError> {
        self.offset = pos;
        Ok(())
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, FileError> {
        if self.flags & O_ACCMODE == O_RDONLY {
            return Err(FileError::PermissionDenied);
        }
        let disk = &self.fs.disk;
        let mut inode = find_inode(disk, self.inode)?;
        if is_dir(inode.i_mode) {
            return Err(FileError::NotFile);
        }
        if buf.is_empty() {
            return Ok(0);
        }

        let mut superblock = SuperBlock::deserialise(disk)?;
        let payload = (BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
        let old_size = inode.i_size;
        if old_size > superblock.total_size {
            return Err(FileError::CorruptedINode);
        }
        let base = if self.flags & O_APPEND != 0 { old_size } else { self.offset };
        let end = base.checked_add(buf.len() as u64).ok_or(FileError::Overflow)?;
        if end > superblock.total_size {
            return Err(FileError::NoMoreBlocks);
        }

        let write_begin = base.min(old_size);
        let first_block = write_begin / payload;
        let last_block = (end - 1) / payload;

        let extents = range_lookup(disk, &inode.i_extents, first_block, last_block + 1)?;
        let mut missing_blocks: Vec<u64> = Vec::new();
        for lb in first_block..=last_block {
            if physical_of(&extents, lb).is_none() {
                missing_blocks.push(lb);
            }
        }
        let missing = missing_blocks.len() as u64;

        let pieces_upper_bound = missing;
        let reserve = pieces_upper_bound.saturating_mul(inode.i_extents.depth as u64 + 2);
        if missing.saturating_add(reserve) > superblock.free_blocks {
            return Err(FileError::NoMoreBlocks);
        }

        let mut runs: Vec<Extent> = Vec::new();
        let mut new_blocks: Vec<u64> = Vec::with_capacity(missing_blocks.len());
        if missing > 0 {
            runs = find_blocks(disk, missing)?;
            if runs.is_empty() {
                return Err(FileError::NoMoreBlocks);
            }
            mark_blocks_used(disk, &runs)?;
            for r in &runs {
                for p in r.physical_start..r.physical_start + r.length {
                    new_blocks.push(p);
                }
            }
            superblock.free_blocks -= missing;
        }

        let mut originals: Vec<(u64, Vec<u8>)> = Vec::new();
        let data_result = (|| -> Result<(), FileError> {
            let mut ni = 0usize;
            for lb in first_block..=last_block {
                let mut block = match physical_of(&extents, lb) {
                    Some(p) => {
                        let b = Block::deserialise(disk, p)?;
                        originals.push((p, b.buf.clone()));
                        b
                    }
                    None => {
                        let p = new_blocks[ni];
                        ni += 1;
                        Block {
                            id: p,
                            header: BlockHeader { lsn: 0, checksum: 0, flag: Flag::Clean },
                            buf: vec![0u8; BLOCK_SIZE],
                        }
                    }
                };
                let block_start = lb * payload;
                let block_end = block_start + payload;

                if base > old_size {
                    let hs = old_size.max(block_start);
                    let he = base.min(block_end);
                    if hs < he {
                        block.buf[BLOCK_HEADER_SIZE + (hs - block_start) as usize
                            ..BLOCK_HEADER_SIZE + (he - block_start) as usize]
                            .fill(0);
                    }
                }

                let ds = base.max(block_start);
                let de = end.min(block_end);
                if ds < de {
                    block.buf[BLOCK_HEADER_SIZE + (ds - block_start) as usize
                        ..BLOCK_HEADER_SIZE + (de - block_start) as usize]
                        .copy_from_slice(&buf[(ds - base) as usize..(de - base) as usize]);
                }
                block.write_block(disk)?;
            }
            Ok(())
        })();
        if let Err(e) = data_result {
            restore_blocks(disk, &originals);
            if !runs.is_empty() {
                let _ = mark_blocks_free(disk, &runs);
            }
            return Err(e);
        }

        let mut pieces: Vec<Extent> = Vec::new();
        for (i, &lb) in missing_blocks.iter().enumerate() {
            let pb = new_blocks[i];
            if let Some(last) = pieces.last_mut() {
                if last.logical_end() == lb && last.physical_start + last.length == pb {
                    last.length += 1;
                    continue;
                }
            }
            pieces.push(Extent { logical_start: lb, physical_start: pb, length: 1 });
        }

        let mut tree_blocks: u64 = 0;
        for piece in &pieces {
            let mut alloc = || -> Result<u64, FileError> {
                let b = allocate_block(disk, &mut superblock)?;
                tree_blocks += 1;
                Ok(b)
            };
            let freed = insert_extent(disk, &mut inode.i_extents, *piece, &mut alloc)?;
            if !freed.is_empty() {
                let n: u64 = freed.iter().map(|e| e.length).sum();
                mark_blocks_free(disk, &freed)?;
                superblock.free_blocks += n;
            }
        }

        let now = now_secs();
        inode.i_size = old_size.max(end);
        inode.i_blocks += missing + tree_blocks;
        inode.i_mtime = now;
        inode.i_ctime = now;
        write_inode(disk, self.inode, &inode.serialise())?;
        disk.write_all_at(&superblock.serialise(), 0)
            .map_err(|_| FileError::WriteError)?;
        self.offset = end;
        Ok(buf.len())
    }
}
