use crate::extent_tree::Extent;
use crate::block::{SuperBlock,Block};
use crate::constants::*;
use crate::file_errors::FileError;

pub fn find_free_inode(disk:&std::fs::File) -> Result<Option<u64>,FileError> {
    let superblock=SuperBlock::deserialise(disk)?;
    let mut inode_traversal_index=0u64;
    for block_id in superblock.inode_bitmap_start..superblock.block_bitmap_start{
        let block=Block::deserialise(disk,block_id)?;
        let bitmap=block.buf[BLOCK_HEADER_SIZE..].to_vec();
        for i in 0..bitmap.len() {
            let byte = bitmap[i];
            if byte == 0xFF {
                inode_traversal_index+=8;
                continue;
            }
            for bit in 0..8 {
                if byte & (1 << bit) == 0 {
                    return Ok(Some(inode_traversal_index));
                }else{
                    inode_traversal_index+=1;
                    if inode_traversal_index>=superblock.inode_count{
                        return Ok(None);
                    }
                }
            }
        }
    }
    Ok(None)
}

pub fn find_blocks(
    disk: &std::fs::File,
    count: u64
) -> Result<Vec<Extent>,FileError> {
    if count == 0 {
        return Ok(Vec::new());
    }

    let superblock=SuperBlock::deserialise(disk)?;
    let mut blocks = Vec::new();
    let mut logical_start=0u64;
    let mut remaining = count;
    let mut run_start = 0u64;
    let mut run_length = 0u64;
    let mut block_traversal_index=0u64;

    for block_id in superblock.block_bitmap_start..superblock.inode_map_start{
        let block=Block::deserialise(disk,block_id)?;
        let bitmap=block.buf[BLOCK_HEADER_SIZE..].to_vec();

        for i in 0..bitmap.len() {
            let byte = bitmap[i];

            for bit in 0..8 {
                if block_traversal_index>=superblock.block_count{
                    return Ok(Vec::new());
                }

                if byte & (1u8 << bit) == 0 {
                    if run_length == 0 {
                        run_start = block_traversal_index;
                    }

                    run_length += 1;

                    if run_length == remaining {
                        blocks.push(Extent {
                            logical_start,
                            physical_start: run_start,
                            length: run_length,
                        });

                        return Ok(blocks);
                    }
                } else if run_length > 0 {
                    blocks.push(Extent {
                        logical_start,
                        physical_start: run_start,
                        length: run_length,
                    });

                    logical_start += run_length;
                    remaining -= run_length;
                    run_length = 0;
                }

                block_traversal_index+=1;
            }
        }
    }

    if run_length > 0 {
        blocks.push(Extent {
            logical_start,
            physical_start: run_start,
            length: run_length,
        });

        remaining -= run_length;
    }

    if remaining == 0 {
        Ok(blocks)
    } else {
        Ok(Vec::new())
    }
}

pub fn mark_blocks_used(
    disk:&std::fs::File,
    extents:&[Extent]
) -> Result<(),FileError> {
    let superblock=SuperBlock::deserialise(disk)?;
    let mut block_traversal_index=0u64;

    for block_id in superblock.block_bitmap_start..superblock.inode_map_start{
        let mut block=Block::deserialise(disk,block_id)?;
        let bitmap=&mut block.buf[BLOCK_HEADER_SIZE..];

        for i in 0..bitmap.len() {
            for bit in 0..8 {
                if extents.iter().any(|extent|
                    block_traversal_index >= extent.physical_start &&
                    block_traversal_index <
                        extent.physical_start + extent.length
                ) {
                    bitmap[i] |= 1 << bit;
                }

                block_traversal_index+=1;
            }
        }

        block.write_block(disk)?;
    }

    Ok(())
}

pub fn mark_blocks_free(
    disk:&std::fs::File,
    extents:&[Extent]
) -> Result<(),FileError> {
    let superblock=SuperBlock::deserialise(disk)?;
    let mut block_traversal_index=0u64;

    for block_id in superblock.block_bitmap_start..superblock.inode_map_start{
        let mut block=Block::deserialise(disk,block_id)?;
        let bitmap=&mut block.buf[BLOCK_HEADER_SIZE..];

        for i in 0..bitmap.len() {
            for bit in 0..8 {
                if extents.iter().any(|extent|
                    block_traversal_index >= extent.physical_start &&
                    block_traversal_index <
                        extent.physical_start + extent.length
                ) {
                    bitmap[i] &= !(1 << bit);
                }

                block_traversal_index+=1;
            }
        }

        block.write_block(disk)?;
    }

    Ok(())
}

pub fn mark_block_free(
    disk:&std::fs::File,
    block_to_free:u64
) -> Result<(),FileError> {
    let superblock=SuperBlock::deserialise(disk)?;
    let mut block_traversal_index=0u64;

    for block_id in superblock.block_bitmap_start..superblock.inode_map_start{
        let mut block=Block::deserialise(disk,block_id)?;
        let bitmap=&mut block.buf[BLOCK_HEADER_SIZE..];

        for i in 0..bitmap.len() {
            for bit in 0..8 {
                if block_traversal_index==block_to_free{
                    bitmap[i] &= !(1 << bit);
                }

                block_traversal_index+=1;
            }
        }

        block.write_block(disk)?;
    }

    Ok(())
}

pub fn mark_inode_used(
    disk:&std::fs::File,
    inode_to_use:u64
) -> Result<(),FileError> {
    let superblock=SuperBlock::deserialise(disk)?;
    let mut inode_traversal_index=0u64;

    for block_id in superblock.inode_bitmap_start..superblock.block_bitmap_start{
        let mut block=Block::deserialise(disk,block_id)?;
        let bitmap=&mut block.buf[BLOCK_HEADER_SIZE..];

        for i in 0..bitmap.len() {
            for bit in 0..8 {
                if inode_traversal_index==inode_to_use {
                    bitmap[i] |= 1 << bit;
                }

                inode_traversal_index+=1;
            }
        }

        block.write_block(disk)?;
    }

    Ok(())
}

pub fn mark_inode_free(
    disk:&std::fs::File,
    inode_to_free:u64
) -> Result<(),FileError> {
    let superblock=SuperBlock::deserialise(disk)?;
    let mut inode_traversal_index=0u64;

    for block_id in superblock.inode_bitmap_start..superblock.block_bitmap_start{
        let mut block=Block::deserialise(disk,block_id)?;
        let bitmap=&mut block.buf[BLOCK_HEADER_SIZE..];

        for i in 0..bitmap.len() {
            for bit in 0..8 {
                if inode_traversal_index==inode_to_free{
                    bitmap[i] &= !(1 << bit);
                }

                inode_traversal_index+=1;
            }
        }

        block.write_block(disk)?;
    }

    Ok(())
}
