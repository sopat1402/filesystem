use crate::block_cache::BlockCache;
use crate::bitmaps::{
    allocate_block, find_blocks, mark_block_free, mark_blocks_free, mark_blocks_used,
};
use crate::constants::*;
use crate::directories::{add_dirent, free_data_extents, is_dir, lexer, read_dir};
use crate::extent_tree::{delete_extent_range, insert_extent, range_lookup, Extent};
use crate::file_errors::FileError;
use crate::filesystem::Filesystem;
use crate::inode::{find_inode, write_inode, Inode};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct File<'a> {
    pub fs: &'a mut Filesystem,
    pub inode: u64,
    pub offset: u64,
    pub flags: u32,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub struct Cred {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}

/// want: 4 = read, 2 = write, 1 = execute/search
fn may_access(inode: &Inode, cred: &Cred, want: u16) -> bool {
    let mode = inode.i_mode;

    if cred.uid == 0 {
        return want & 1 == 0 || is_dir(mode) || mode & 0o111 != 0;
    }

    let bits = if cred.uid == inode.i_uid {
        (mode >> 6) & 7
    } else if cred.gid == inode.i_gid || cred.groups.contains(&inode.i_gid) {
        (mode >> 3) & 7
    } else {
        mode & 7
    };

    bits & want == want
}

fn physical_of(extents: &[Extent], lb: u64) -> Option<u64> {
    let idx = extents.partition_point(|e| e.logical_start <= lb);

    if idx == 0 {
        return None;
    }

    let extent = &extents[idx - 1];

    if lb < extent.logical_end() {
        extent.physical_start.checked_add(lb - extent.logical_start)
    } else {
        None
    }
}

fn restore_blocks(
    block_cache: &mut BlockCache,
    originals: &[(u64, Vec<u8>)],
) -> Result<(), FileError> {
    for (block_id, original) in originals {
        let block = block_cache.get_mut(*block_id)?;
        block.buf.copy_from_slice(original);
    }

    Ok(())
}

pub fn truncate_to_zero(
    block_cache: &mut BlockCache,
    inode_id: u64,
) -> Result<(), FileError> {
    let mut inode = find_inode(block_cache, inode_id)?;
    let mut freed_tree_blocks = Vec::new();

    let freed = {
        let mut free_block = |block_id: u64| -> Result<(), FileError> {
            freed_tree_blocks.push(block_id);
            Ok(())
        };

        delete_extent_range(
            block_cache,
            &mut inode.i_extents,
            0,
            u64::MAX,
            &mut free_block,
        )?
    };

    for block_id in &freed_tree_blocks {
        mark_block_free(block_cache, *block_id)?;
    }

    if !freed_tree_blocks.is_empty() {
        let superblock = block_cache.get_superblock_mut()?;
        superblock.free_blocks += freed_tree_blocks.len() as u64;
    }

    free_data_extents(block_cache, &freed)?;

    let now = now_secs();
    inode.i_size = 0;
    inode.i_blocks = 0;
    inode.i_mtime = now;
    inode.i_ctime = now;

    write_inode(block_cache, inode_id, &inode.serialise())?;
    Ok(())
}

impl<'a> File<'a> {
    pub fn open(
        fs: &'a mut Filesystem,
        path: &String,
        cwd: u64,
        flags: u32,
        cred: &Cred,
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

        let block_cache = &mut fs.block_cache;
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
            let filename = &tokens[tokens.len() - 1];

            let mut dir_inode = if path.starts_with('/') {
                ROOT_INODE_NUM as u64
            } else {
                cwd
            };

            for component in &tokens[..tokens.len() - 1] {
                let dir = find_inode(block_cache, dir_inode)?;

                if !is_dir(dir.i_mode) {
                    return Err(FileError::NotDirectory);
                }

                if !may_access(&dir, cred, 1) {
                    return Err(FileError::PermissionDenied);
                }

                let dirents = read_dir(block_cache, dir_inode)?;

                dir_inode = dirents
                    .iter()
                    .find(|(name, _)| name == component)
                    .map(|(_, inode)| *inode)
                    .ok_or(FileError::NameNotFound)?;
            }

            let parent = find_inode(block_cache, dir_inode)?;

            if !is_dir(parent.i_mode) {
                return Err(FileError::NotDirectory);
            }

            if !may_access(&parent, cred, 1) {
                return Err(FileError::PermissionDenied);
            }

            let dirents = read_dir(block_cache, dir_inode)?;

            let existing = dirents
                .iter()
                .find(|(name, _)| name == filename)
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

                    if !may_access(&parent, cred, 3) {
                        return Err(FileError::PermissionDenied);
                    }

                    (
                        add_dirent(
                            block_cache,
                            dir_inode,
                            filename.clone(),
                            S_IFREG | 0o644,
                            cred.uid,
                            cred.gid,
                        )?,
                        false,
                    )
                }
            }
        };

        if already_exists && flags & O_CREAT != 0 && flags & O_EXCL != 0 {
            return Err(FileError::NameExists);
        }

        let node = find_inode(block_cache, file_inode)?;
        let node_is_dir = is_dir(node.i_mode);

        if (flags & O_DIRECTORY != 0 || trailing_slash) && !node_is_dir {
            return Err(FileError::NotDirectory);
        }

        if node_is_dir && access != O_RDONLY {
            return Err(FileError::NotFile);
        }

        if already_exists {
            let mut want = match access {
                O_RDONLY => 4,
                O_WRONLY => 2,
                _ => 6,
            };

            if flags & O_TRUNC != 0 {
                want |= 2;
            }

            if !may_access(&node, cred, want) {
                return Err(FileError::PermissionDenied);
            }
        }

        if flags & O_TRUNC != 0 && !node_is_dir {
            truncate_to_zero(block_cache, file_inode)?;
        }

        Ok(Self {
            fs,
            inode: file_inode,
            offset: 0,
            flags,
        })
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

        if buf.is_empty() {
            return Ok(0);
        }

        let block_cache = &mut self.fs.block_cache;
        let mut inode = find_inode(block_cache, self.inode)?;

        if is_dir(inode.i_mode) {
            return Err(FileError::NotFile);
        }

        let payload = (BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
        let old_size = inode.i_size;
        let total_size = block_cache.get_superblock()?.total_size;

        if old_size > total_size {
            return Err(FileError::CorruptedINode);
        }

        let base = if self.flags & O_APPEND != 0 {
            old_size
        } else {
            self.offset
        };

        let end = base
            .checked_add(buf.len() as u64)
            .ok_or(FileError::EOverflow)?;

        if end > total_size {
            return Err(FileError::NoMoreBlocks);
        }

        let write_begin = base.min(old_size);
        let first_block = write_begin / payload;
        let last_block = (end - 1) / payload;

        let extents = range_lookup(
            block_cache,
            &inode.i_extents,
            first_block,
            last_block + 1,
        )?;

        let mut missing_blocks = Vec::new();

        for lb in first_block..=last_block {
            if physical_of(&extents, lb).is_none() {
                missing_blocks.push(lb);
            }
        }

        let missing = missing_blocks.len() as u64;

        let root_can_absorb = inode.i_extents.is_leaf()
            && inode.i_extents.entry_count as usize + missing_blocks.len()
                <= ROOT_MAX_ENTRIES;

        let reserve = if root_can_absorb {
            0
        } else {
            missing.saturating_mul(inode.i_extents.depth as u64 + 2)
        };

        let free_blocks = block_cache.get_superblock()?.free_blocks;

        if missing.saturating_add(reserve) > free_blocks {
            return Err(FileError::NoMoreBlocks);
        }

        let mut runs = Vec::new();
        let mut new_blocks = Vec::with_capacity(missing_blocks.len());

        if missing > 0 {
            runs = find_blocks(block_cache, missing)?;

            if runs.is_empty() {
                return Err(FileError::NoMoreBlocks);
            }

            mark_blocks_used(block_cache, &runs)?;

            for extent in &runs {
                for block_id in
                    extent.physical_start..extent.physical_start + extent.length
                {
                    new_blocks.push(block_id);
                }
            }

            block_cache.get_superblock_mut()?.free_blocks -= missing;
        }

        let mut originals = Vec::new();

        let data_result = (|| -> Result<(), FileError> {
            let mut new_index = 0usize;

            for lb in first_block..=last_block {
                let physical = match physical_of(&extents, lb) {
                    Some(p) => p,

                    None => {
                        let p = new_blocks[new_index];
                        new_index += 1;

                        let block = block_cache.get_mut(p)?;
                        block.buf[BLOCK_HEADER_SIZE..].fill(0);

                        p
                    }
                };

                let block = block_cache.get_mut(physical)?;

                if physical_of(&extents, lb).is_some() {
                    originals.push((
                        physical,
                        block.buf[BLOCK_HEADER_SIZE..].to_vec(),
                    ));
                }

                let block_start = lb * payload;
                let block_end = block_start + payload;

                if base > old_size {
                    let hole_start = old_size.max(block_start);
                    let hole_end = base.min(block_end);

                    if hole_start < hole_end {
                        block.buf[
                            BLOCK_HEADER_SIZE + (hole_start - block_start) as usize
                                ..BLOCK_HEADER_SIZE + (hole_end - block_start) as usize
                        ]
                            .fill(0);
                    }
                }

                let data_start = base.max(block_start);
                let data_end = end.min(block_end);

                if data_start < data_end {
                    block.buf[
                        BLOCK_HEADER_SIZE + (data_start - block_start) as usize
                            ..BLOCK_HEADER_SIZE + (data_end - block_start) as usize
                    ]
                        .copy_from_slice(
                            &buf[
                                (data_start - base) as usize
                                    ..(data_end - base) as usize
                            ],
                        );
                }
            }

            Ok(())
        })();

        if let Err(error) = data_result {
            restore_blocks(block_cache, &originals)?;

            if !runs.is_empty() {
                mark_blocks_free(block_cache, &runs)?;
                block_cache.get_superblock_mut()?.free_blocks += missing;
            }

            return Err(error);
        }

        let mut pieces: Vec<Extent> = Vec::new();

        for (i, &lb) in missing_blocks.iter().enumerate() {
            let pb = new_blocks[i];

            if let Some(last) = pieces.last_mut() {
                if last.logical_end() == lb
                    && last.physical_start + last.length == pb
                {
                    last.length += 1;
                    continue;
                }
            }

            pieces.push(Extent {
                logical_start: lb,
                physical_start: pb,
                length: 1,
            });
        }

        let mut tree_blocks = 0u64;

        for piece in &pieces {
            let mut alloc = |cache: &mut BlockCache| -> Result<u64, FileError> {
                let block = allocate_block(cache)?;
                tree_blocks += 1;
                Ok(block)
            };

            let freed = insert_extent(
                block_cache,
                &mut inode.i_extents,
                *piece,
                &mut alloc,
            )?;

            if !freed.is_empty() {
                let freed_count: u64 =
                    freed.iter().map(|e| e.length).sum();

                mark_blocks_free(block_cache, &freed)?;
                block_cache.get_superblock_mut()?.free_blocks += freed_count;
            }
        }

        let now = now_secs();

        inode.i_size = old_size.max(end);
        inode.i_blocks += missing + tree_blocks;
        inode.i_mtime = now;
        inode.i_ctime = now;

        write_inode(block_cache, self.inode, &inode.serialise())?;

        self.offset = end;
        Ok(buf.len())
    }

    pub fn read(&mut self, length: usize) -> Result<Vec<u8>, FileError> {
        if self.flags & O_ACCMODE == O_WRONLY {
            return Err(FileError::PermissionDenied);
        }

        let out = read_at(
            &mut self.fs.block_cache,
            self.inode,
            self.offset,
            length,
        )?;

        self.offset += out.len() as u64;
        Ok(out)
    }
}

pub fn read_at(
    block_cache: &mut BlockCache,
    inode_id: u64,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>, FileError> {
    let total_size = block_cache.get_superblock()?.total_size;
    let inode = find_inode(block_cache, inode_id)?;

    if is_dir(inode.i_mode) {
        return Err(FileError::NotFile);
    }

    if inode.i_size > total_size {
        return Err(FileError::CorruptedINode);
    }

    if offset >= inode.i_size || length == 0 {
        return Ok(Vec::new());
    }

    let payload = (BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64;
    let end_offset = offset.saturating_add(length as u64).min(inode.i_size);
    let read_len = (end_offset - offset) as usize;
    let start_block = offset / payload;
    let end_block = (end_offset - 1) / payload + 1;

    let extents = range_lookup(
        block_cache,
        &inode.i_extents,
        start_block,
        end_block,
    )?;

    let mut result = vec![0u8; read_len];

    for extent in &extents {
        let first = start_block.max(extent.logical_start);
        let last = end_block.min(extent.logical_end());

        for lb in first..last {
            let physical = extent
                .physical_start
                .checked_add(lb - extent.logical_start)
                .ok_or(FileError::CorruptedINode)?;

            let block = block_cache.get(physical)?;
            let block_start = lb * payload;
            let start = offset.max(block_start);
            let end = end_offset.min(block_start.saturating_add(payload));

            let dst = (start - offset) as usize..(end - offset) as usize;
            let src = BLOCK_HEADER_SIZE + (start - block_start) as usize
                ..BLOCK_HEADER_SIZE + (end - block_start) as usize;

            result[dst].copy_from_slice(&block.buf[src]);
        }
    }

    let mut inode = inode;
    inode.i_atime = now_secs();

    write_inode(block_cache, inode_id, &inode.serialise())?;
    Ok(result)
}
