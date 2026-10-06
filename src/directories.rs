use crate::constants::*;
use crate::file_errors::FileError;
use crate::inode::{find_inode, write_inode, Stat, stat};
use crate::extent_tree::{range_lookup, insert_extent, delete_extent_range};
use crate::filesystem::reserve_inode;
use crate::bitmaps::*;
use crate::block_cache::BlockCache;

pub fn is_dir(mode: u16) -> bool {
    mode & 0o170000 == S_IFDIR
}

pub fn read_dir(
    block_cache: &mut BlockCache,
    inode_id: u64,
) -> Result<Vec<(String, u64)>, FileError> {
    let node = find_inode(block_cache, inode_id)?;

    if !is_dir(node.i_mode) {
        return Err(FileError::NotDirectory);
    }

    let extents = range_lookup(block_cache, &node.i_extents, 0, u64::MAX)?;
    let mut dirents: Vec<(String, u64)> = Vec::new();

    for extent in extents {
        for b in extent.physical_start..extent.physical_start + extent.length {
            let block = block_cache.get(b)?;
            let payload = &block.buf[BLOCK_HEADER_SIZE..];
            let mut offset = 0usize;

            while offset + 2 <= payload.len() {
                let name_len =
                    u16::from_le_bytes([payload[offset], payload[offset + 1]]) as usize;

                if name_len == 0 {
                    break;
                }

                let name_start = offset + 2;
                let name_end = name_start + name_len;
                let entry_end = name_end + 8;

                if entry_end > payload.len() {
                    return Err(FileError::CorruptedBlock);
                }

                let name = String::from_utf8(payload[name_start..name_end].to_vec())
                    .map_err(|_| FileError::CorruptedBlock)?;

                let inode_num = u64::from_le_bytes(
                    payload[name_end..entry_end]
                        .try_into()
                        .map_err(|_| FileError::CorruptedBlock)?,
                );

                dirents.push((name, inode_num));
                offset = entry_end;
            }
        }
    }

    Ok(dirents)
}

pub fn print_dir(
    block_cache: &mut BlockCache,
    inode_id: u64,
) -> Result<(), FileError> {
    let dirents = read_dir(block_cache, inode_id)?;

    for (dirent, _) in dirents {
        print!("{}\t", dirent);
    }

    println!();
    Ok(())
}

pub fn free_data_extents(
    block_cache: &mut BlockCache,
    extents: &[crate::extent_tree::Extent],
) -> Result<(), FileError> {
    for extent in extents {
        for block_id in extent.physical_start..extent.physical_start + extent.length {
            {
                let block = block_cache.get_mut(block_id)?;
                block.buf[BLOCK_HEADER_SIZE..].fill(0);
            }

            mark_block_free(block_cache, block_id)?;

            let superblock = block_cache.get_superblock_mut()?;
            superblock.free_blocks += 1;
        }
    }

    Ok(())
}

pub fn lookup(
    block_cache: &mut BlockCache,
    parent: u64,
    name: &str,
) -> Result<Stat, FileError> {
    let child = read_dir(block_cache, parent)?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, id)| id)
        .ok_or(FileError::NameNotFound)?;

    stat(block_cache, child)
}

pub fn delete_dirent(
    block_cache: &mut BlockCache,
    inode_id: u64,
) -> Result<(), FileError> {
    let mut node = find_inode(block_cache, inode_id)?;

    if is_dir(node.i_mode) {
        let dirents = read_dir(block_cache, inode_id)?;

        for (name, id) in dirents {
            if name == "." || name == ".." {
                continue;
            }

            delete_dirent(block_cache, id)?;
        }
    }

    let mut freed_tree_blocks = Vec::new();

    let freed = {
        let mut free_block = |block_id: u64| -> Result<(), FileError> {
            freed_tree_blocks.push(block_id);
            Ok(())
        };

        delete_extent_range(
            block_cache,
            &mut node.i_extents,
            0,
            u64::MAX,
            &mut free_block,
        )?
    };

    for block_id in freed_tree_blocks {
        mark_block_free(block_cache, block_id)?;
    }

    free_data_extents(block_cache, &freed)?;
    mark_inode_free(block_cache, inode_id)?;

    let superblock = block_cache.get_superblock_mut()?;
    superblock.free_blocks += freed
        .iter()
        .map(|extent| extent.length)
        .sum::<u64>();
    superblock.free_inodes += 1;

    Ok(())
}

pub fn delete(
    block_cache: &mut BlockCache,
    parent_inode: u64,
    name: String,
) -> Result<(), FileError> {
    if name == String::from(".") || name == String::from("..") {
        return Err(FileError::NameExists);
    }

    let mut res = read_dir(block_cache, parent_inode)?;
    let idx = res
        .iter()
        .position(|(entry_name, _)| *entry_name == name)
        .ok_or(FileError::NameNotFound)?;

    let (_, child_inode) = res[idx];
    let child = find_inode(block_cache, child_inode)?;
    let child_is_dir = is_dir(child.i_mode);

    delete_dirent(block_cache, child_inode)?;
    res.remove(idx);
    write_dirents(block_cache, parent_inode, res)?;

    if child_is_dir {
        let mut parent = find_inode(block_cache, parent_inode)?;
        parent.i_links_count -= 1;
        write_inode(block_cache, parent_inode, &parent.serialise())?;
    }

    Ok(())
}

fn allocate_block(block_cache: &mut BlockCache) -> Result<u64, FileError> {
    let new_extent = find_blocks(block_cache, 1)?;

    if new_extent.is_empty() {
        return Err(FileError::NoMoreBlocks);
    }

    mark_blocks_used(block_cache, &new_extent)?;

    let superblock = block_cache.get_superblock_mut()?;
    superblock.free_blocks = superblock
        .free_blocks
        .checked_sub(1)
        .ok_or(FileError::NoMoreBlocks)?;

    Ok(new_extent[0].physical_start)
}

pub fn add_dirent(
    block_cache: &mut BlockCache,
    directory_inode: u64,
    new_name: String,
    mode: u16,
    uid: u32,
    gid: u32,
) -> Result<u64, FileError> {
    if new_name.len() == 0 {
        return Err(FileError::NameExists);
    }

    if new_name.len() > 255 {
        return Err(FileError::NameTooLong);
    }

    let dirents = read_dir(block_cache, directory_inode)?;

    for (name, _) in &dirents {
        if *name == new_name {
            return Err(FileError::NameExists);
        }
    }

    let node = reserve_inode(block_cache, mode, uid, gid)?;
    let mut new_inode = find_inode(block_cache, node)?;
    new_inode.i_links_count = 1;
    write_inode(block_cache, node, &new_inode.serialise())?;

    let mut write_buf = vec![0u8; new_name.len() + 10];
    write_buf[0..2].copy_from_slice(&(new_name.len() as u16).to_le_bytes());
    write_buf[2..2 + new_name.len()].copy_from_slice(new_name.as_bytes());
    write_buf[2 + new_name.len()..].copy_from_slice(&node.to_le_bytes());

    let mut inode = find_inode(block_cache, directory_inode)?;
    let extents = range_lookup(block_cache, &inode.i_extents, 0, u64::MAX)?;
    let next_logical = extents
        .iter()
        .map(|e| e.logical_start + e.length)
        .max()
        .unwrap_or(0);

    let mut write: Option<(u64, usize)> = None;

    'outer: for extent in extents {
        for block_id in extent.physical_start..extent.physical_start + extent.length {
            let candidate = {
                let block = block_cache.get_mut(block_id)?;
                let mut offset = BLOCK_HEADER_SIZE;

                while offset + 2 <= BLOCK_SIZE {
                    let len = u16::from_le_bytes([
                        block.buf[offset],
                        block.buf[offset + 1],
                    ]) as usize;

                    if len == 0 {
                        break;
                    }

                    offset += 2 + len + 8;
                }

                if offset > BLOCK_SIZE || BLOCK_SIZE - offset < write_buf.len() {
                    None
                } else {
                    Some((block_id, offset))
                }
            };

            if let Some(location) = candidate {
                write = Some(location);
                break 'outer;
            }
        }
    }

    if let Some((block_id, offset)) = write {
        let block = block_cache.get_mut(block_id)?;
        block.buf[offset..offset + write_buf.len()].copy_from_slice(&write_buf);
        inode.i_size += write_buf.len() as u64;
    } else {
        let new_block_id = allocate_block(block_cache)?;
        let new_extent = crate::extent_tree::Extent {
            logical_start: next_logical,
            physical_start: new_block_id,
            length: 1,
        };

        let mut tree_blocks = 0u64;

        let mut alloc_block =
            |cache: &mut BlockCache| -> Result<u64, FileError> {
                let block = allocate_block(cache)?;
                tree_blocks += 1;
                Ok(block)
            };

        insert_extent(
            block_cache,
            &mut inode.i_extents,
            new_extent,
            &mut alloc_block,
        )?;

        inode.i_blocks += 1 + tree_blocks;

        let new_block = block_cache.get_mut(new_block_id)?;
        new_block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + write_buf.len()]
            .copy_from_slice(&write_buf);

        inode.i_size += write_buf.len() as u64;
    }

    write_inode(block_cache, directory_inode, &inode.serialise())?;
    Ok(node)
}

pub fn write_dirents(
    block_cache: &mut BlockCache,
    directory_inode: u64,
    dirents: Vec<(String, u64)>,
) -> Result<(), FileError> {
    let mut node = find_inode(block_cache, directory_inode)?;
    let mut freed_tree_blocks = Vec::new();

    let freed = {
        let mut free_block = |block_id: u64| -> Result<(), FileError> {
            freed_tree_blocks.push(block_id);
            Ok(())
        };

        delete_extent_range(
            block_cache,
            &mut node.i_extents,
            0,
            u64::MAX,
            &mut free_block,
        )?
    };

    for block_id in freed_tree_blocks {
        mark_block_free(block_cache, block_id)?;
    }

    free_data_extents(block_cache, &freed)?;

    let payload_size = BLOCK_SIZE - BLOCK_HEADER_SIZE;
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut total = 0u64;

    for (name, child_id) in &dirents {
        let mut entry = Vec::with_capacity(name.len() + 10);
        entry.extend_from_slice(&(name.len() as u16).to_le_bytes());
        entry.extend_from_slice(name.as_bytes());
        entry.extend_from_slice(&child_id.to_le_bytes());

        if !cur.is_empty() && cur.len() + entry.len() > payload_size {
            chunks.push(std::mem::take(&mut cur));
        }

        total += entry.len() as u64;
        cur.extend_from_slice(&entry);
    }

    if !cur.is_empty() {
        chunks.push(cur);
    }

    node.i_size = total;
    let mut tree_blocks = 0u64;

    for (logical, chunk) in chunks.iter().enumerate() {
        let block_id = allocate_block(block_cache)?;
        let new_extent = crate::extent_tree::Extent {
            logical_start: logical as u64,
            physical_start: block_id,
            length: 1,
        };

        let mut alloc_block =
            |cache: &mut BlockCache| -> Result<u64, FileError> {
                let block = allocate_block(cache)?;
                tree_blocks += 1;
                Ok(block)
            };

        insert_extent(
            block_cache,
            &mut node.i_extents,
            new_extent,
            &mut alloc_block,
        )?;

        let block = block_cache.get_mut(block_id)?;
        block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + chunk.len()]
            .copy_from_slice(chunk);
    }

    node.i_blocks = chunks.len() as u64 + tree_blocks;
    write_inode(block_cache, directory_inode, &node.serialise())?;

    Ok(())
}

pub fn lexer(path: &String) -> Vec<String> {
    let mut curr_name = String::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut is_escape = false;

    for c in path.chars() {
        if is_escape {
            curr_name.push(c);
            is_escape = false;
            continue;
        }

        match c {
            '\\' => is_escape = true,
            '/' => {
                if !curr_name.is_empty() {
                    tokens.push(std::mem::take(&mut curr_name));
                }
            }
            _ => curr_name.push(c),
        }
    }

    if !curr_name.is_empty() {
        tokens.push(curr_name);
    }

    tokens
}

pub fn resolve_path(
    block_cache: &mut BlockCache,
    path: String,
    current: u64,
) -> Result<u64, FileError> {
    if path.len() == 0 {
        return Ok(ROOT_INODE_NUM as u64);
    }

    let absolute: bool = path.starts_with('/');
    let tokens = lexer(&path);
    let mut id: u64;

    if absolute {
        id = ROOT_INODE_NUM as u64;
    } else {
        id = current;
    }

    'outer: for token in &tokens {
        let dirents = read_dir(block_cache, id)?;

        for (dirent, inode) in dirents {
            if dirent == *token {
                id = inode;
                continue 'outer;
            }
        }

        return Err(FileError::NameNotFound);
    }

    Ok(id)
}

pub fn make_dir(
    block_cache: &mut BlockCache,
    parent_inode: u64,
    name: String,
    uid: u32,
    gid: u32,
    permissions: u16,
) -> Result<u64, FileError> {
    let new_id = add_dirent(
        block_cache,
        parent_inode,
        name,
        S_IFDIR | (permissions & 0o7777),
        uid,
        gid,
    )?;

    let self_and_parent = vec![
        (".".to_string(), new_id),
        ("..".to_string(), parent_inode),
    ];

    write_dirents(block_cache, new_id, self_and_parent)?;

    let mut new_node = find_inode(block_cache, new_id)?;
    new_node.i_links_count = 2;
    write_inode(block_cache, new_id, &new_node.serialise())?;

    let mut parent_node = find_inode(block_cache, parent_inode)?;
    parent_node.i_links_count += 1;
    write_inode(block_cache, parent_inode, &parent_node.serialise())?;

    Ok(new_id)
}

pub fn rename_dirent(
    block_cache: &mut BlockCache,
    old_parent_inode: u64,
    old_name: String,
    new_parent_inode: u64,
    new_name: String,
) -> Result<(), FileError> {
    if old_name.is_empty()
        || new_name.is_empty()
        || old_name.len() > 255
        || new_name.len() > 255
        || old_name.contains('/')
        || new_name.contains('/')
        || old_name.contains('\0')
        || new_name.contains('\0')
    {
        return Err(FileError::InvalidRequest);
    }

    if old_name == "." || old_name == ".." || new_name == "." || new_name == ".." {
        return Err(FileError::NameExists);
    }

    let same_parent = old_parent_inode == new_parent_inode;
    let mut old_entries = read_dir(block_cache, old_parent_inode)?;

    let source_index = old_entries
        .iter()
        .position(|(name, _)| name == &old_name)
        .ok_or(FileError::NameNotFound)?;

    let source_inode = old_entries[source_index].1;

    if same_parent && old_name == new_name {
        return Ok(());
    }

    let source = find_inode(block_cache, source_inode)?;
    let source_is_dir = is_dir(source.i_mode);

    let mut new_entries = if same_parent {
        old_entries.clone()
    } else {
        read_dir(block_cache, new_parent_inode)?
    };

    let target_inode = new_entries
        .iter()
        .find(|(name, _)| name == &new_name)
        .map(|(_, inode)| *inode);

    let mut target_is_dir = false;

    if let Some(target_id) = target_inode {
        if target_id == source_inode {
            return Ok(());
        }

        let target = find_inode(block_cache, target_id)?;
        target_is_dir = is_dir(target.i_mode);

        if source_is_dir && !target_is_dir {
            return Err(FileError::NotDirectory);
        }

        if !source_is_dir && target_is_dir {
            return Err(FileError::NotFile);
        }

        if target_is_dir {
            let contents = read_dir(block_cache, target_id)?;

            if contents
                .iter()
                .any(|(name, _)| name != "." && name != "..")
            {
                return Err(FileError::NameExists);
            }
        }
    }

    if source_is_dir && old_parent_inode != new_parent_inode {
        let mut ancestor = new_parent_inode;
        let mut seen = Vec::new();

        loop {
            if ancestor == source_inode {
                return Err(FileError::InvalidRequest);
            }

            if seen.contains(&ancestor) {
                return Err(FileError::CorruptedINode);
            }

            seen.push(ancestor);

            let ancestor_entries = read_dir(block_cache, ancestor)?;
            let parent_of_ancestor = ancestor_entries
                .iter()
                .find(|(name, _)| name == "..")
                .map(|(_, inode)| *inode)
                .ok_or(FileError::CorruptedBlock)?;

            if parent_of_ancestor == ancestor {
                break;
            }

            ancestor = parent_of_ancestor;
        }
    }

    if let Some(target_id) = target_inode {
        delete_dirent(block_cache, target_id)?;
    }

    old_entries.retain(|(name, _)| name != &old_name);

    if same_parent {
        old_entries.retain(|(name, _)| name != &new_name);
        old_entries.push((new_name, source_inode));
        write_dirents(block_cache, old_parent_inode, old_entries)?;
    } else {
        new_entries.retain(|(name, _)| name != &new_name);
        new_entries.push((new_name, source_inode));

        write_dirents(block_cache, old_parent_inode, old_entries)?;
        write_dirents(block_cache, new_parent_inode, new_entries)?;

        if source_is_dir {
            let mut moved_entries = read_dir(block_cache, source_inode)?;

            let dotdot = moved_entries
                .iter_mut()
                .find(|(name, _)| name == "..")
                .ok_or(FileError::CorruptedBlock)?;

            dotdot.1 = new_parent_inode;
            write_dirents(block_cache, source_inode, moved_entries)?;
        }
    }

    if same_parent {
        if target_is_dir {
            let mut parent = find_inode(block_cache, old_parent_inode)?;
            parent.i_links_count = parent
                .i_links_count
                .checked_sub(1)
                .ok_or(FileError::CorruptedINode)?;
            write_inode(block_cache, old_parent_inode, &parent.serialise())?;
        }
    } else {
        if source_is_dir {
            let mut old_parent = find_inode(block_cache, old_parent_inode)?;
            old_parent.i_links_count = old_parent
                .i_links_count
                .checked_sub(1)
                .ok_or(FileError::CorruptedINode)?;
            write_inode(block_cache, old_parent_inode, &old_parent.serialise())?;
        }

        if source_is_dir || target_is_dir {
            let mut new_parent = find_inode(block_cache, new_parent_inode)?;

            if target_is_dir {
                new_parent.i_links_count = new_parent
                    .i_links_count
                    .checked_sub(1)
                    .ok_or(FileError::CorruptedINode)?;
            }

            if source_is_dir {
                new_parent.i_links_count = new_parent
                    .i_links_count
                    .checked_add(1)
                    .ok_or(FileError::EOverflow)?;
            }

            write_inode(block_cache, new_parent_inode, &new_parent.serialise())?;
        }
    }

    Ok(())
}
