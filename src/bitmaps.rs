use crate::block_cache::BlockCache;
use crate::constants::*;
use crate::extent_tree::Extent;
use crate::file_errors::FileError;

const BITS_PER_BITMAP_BLOCK: u64 = ((BLOCK_SIZE - BLOCK_HEADER_SIZE) as u64) * 8;

#[derive(Clone, Copy)]
struct BitmapLayout {
    inode_bitmap_start: u64,
    block_bitmap_start: u64,
    block_bitmap_end: u64,
    inode_count: u64,
    block_count: u64,
}

fn bitmap_layout(block_cache: &mut BlockCache) -> Result<BitmapLayout, FileError> {
    let sb = block_cache.get_superblock()?;
    Ok(BitmapLayout {
        inode_bitmap_start: sb.inode_bitmap_start,
        block_bitmap_start: sb.block_bitmap_start,
        block_bitmap_end: sb.inode_map_start,
        inode_count: sb.inode_count,
        block_count: sb.block_count,
    })
}

fn write_bit_range(block_cache: &mut BlockCache, bitmap_start: u64, first: u64, len: u64, limit: u64, value: bool) -> Result<(), FileError> {
    let end = first.saturating_add(len).min(limit);
    let mut bit = first;
    while bit < end {
        let bitmap_block = bit / BITS_PER_BITMAP_BLOCK;
        let chunk_end = end.min((bitmap_block + 1) * BITS_PER_BITMAP_BLOCK);
        let block = block_cache.get_mut(bitmap_start + bitmap_block)?;
        let bitmap = &mut block.buf[BLOCK_HEADER_SIZE..];
        for b in bit..chunk_end {
            let offset = b % BITS_PER_BITMAP_BLOCK;
            let byte = (offset / 8) as usize;
            let mask = 1u8 << (offset % 8);
            if value {
                bitmap[byte] |= mask;
            } else {
                bitmap[byte] &= !mask;
            }
        }
        bit = chunk_end;
    }
    Ok(())
}

fn write_extents(block_cache: &mut BlockCache, extents: &[Extent], value: bool) -> Result<(), FileError> {
    let layout = bitmap_layout(block_cache)?;
    for extent in extents {
        write_bit_range(block_cache, layout.block_bitmap_start, extent.physical_start, extent.length, layout.block_count, value)?;
    }
    Ok(())
}

pub fn find_free_inode(block_cache: &mut BlockCache) -> Result<Option<u64>, FileError> {
    let layout = bitmap_layout(block_cache)?;
    let mut idx = 0u64;
    for block_id in layout.inode_bitmap_start..layout.block_bitmap_start {
        let block = block_cache.get(block_id)?;
        for &byte in &block.buf[BLOCK_HEADER_SIZE..] {
            for bit in 0..8 {
                if idx >= layout.inode_count {
                    return Ok(None);
                }
                if byte & (1 << bit) == 0 {
                    return Ok(Some(idx));
                }
                idx += 1;
            }
        }
    }
    Ok(None)
}

pub fn find_blocks(block_cache: &mut BlockCache, count: u64) -> Result<Vec<Extent>, FileError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let layout = bitmap_layout(block_cache)?;
    let mut blocks = Vec::new();
    let mut logical_start = 0u64;
    let mut remaining = count;
    let mut run_start = 0u64;
    let mut run_length = 0u64;
    let mut block_traversal_index = 0u64;

    for block_id in layout.block_bitmap_start..layout.block_bitmap_end {
        let block = block_cache.get(block_id)?;
        for &byte in &block.buf[BLOCK_HEADER_SIZE..] {
            for bit in 0..8 {
                if block_traversal_index >= layout.block_count {
                    return Ok(Vec::new());
                }
                if byte & (1u8 << bit) == 0 {
                    if run_length == 0 {
                        run_start = block_traversal_index;
                    }
                    run_length += 1;
                    if run_length == remaining {
                        blocks.push(Extent { logical_start, physical_start: run_start, length: run_length });
                        return Ok(blocks);
                    }
                } else if run_length > 0 {
                    blocks.push(Extent { logical_start, physical_start: run_start, length: run_length });
                    logical_start += run_length;
                    remaining -= run_length;
                    run_length = 0;
                }
                block_traversal_index += 1;
            }
        }
    }

    if run_length > 0 {
        blocks.push(Extent { logical_start, physical_start: run_start, length: run_length });
        remaining -= run_length;
    }

    if remaining == 0 {
        Ok(blocks)
    } else {
        Ok(Vec::new())
    }
}

pub fn mark_blocks_used(block_cache: &mut BlockCache, extents: &[Extent]) -> Result<(), FileError> {
    write_extents(block_cache, extents, true)
}

pub fn mark_blocks_free(block_cache: &mut BlockCache, extents: &[Extent]) -> Result<(), FileError> {
    write_extents(block_cache, extents, false)
}

pub fn mark_block_free(block_cache: &mut BlockCache, block_to_free: u64) -> Result<(), FileError> {
    let layout = bitmap_layout(block_cache)?;
    write_bit_range(block_cache, layout.block_bitmap_start, block_to_free, 1, layout.block_count, false)
}

pub fn mark_inode_used(block_cache: &mut BlockCache, inode_to_use: u64) -> Result<(), FileError> {
    let layout = bitmap_layout(block_cache)?;
    write_bit_range(block_cache, layout.inode_bitmap_start, inode_to_use, 1, layout.inode_count, true)
}

pub fn mark_inode_free(block_cache: &mut BlockCache, inode_to_free: u64) -> Result<(), FileError> {
    let layout = bitmap_layout(block_cache)?;
    write_bit_range(block_cache, layout.inode_bitmap_start, inode_to_free, 1, layout.inode_count, false)
}

pub fn allocate_block(block_cache: &mut BlockCache) -> Result<u64, FileError> {
    if block_cache.get_superblock()?.free_blocks == 0 {
        return Err(FileError::NoMoreBlocks);
    }
    let extents = find_blocks(block_cache, 1)?;
    let physical_start = extents.first().ok_or(FileError::NoMoreBlocks)?.physical_start;
    mark_blocks_used(block_cache, &extents)?;
    let superblock = block_cache.get_superblock_mut()?;
    superblock.free_blocks -= 1;
    Ok(physical_start)
}
