use crate::extent_tree::Extent;

pub fn find_free_inode(bitmap: &[u8]) -> Option<usize> {
    for i in 0..bitmap.len() {
        let byte = bitmap[i];
        if byte == 0xFF {
            continue;
        }
        for bit in 0..8 {
            if byte & (1 << bit) == 0 {
                return Some(i * 8 + bit);
            }
        }
    }
    None
}

pub fn find_blocks(bitmap: &[u8], count: usize) -> Vec<Extent> {
    if count == 0 {
        return Vec::new();
    }
    let mut blocks = Vec::new();
    let mut logical_start: u32 = 0;
    let mut remaining = count;
    let mut run_start = 0usize;
    let mut run_length = 0usize;
    for (i, &byte) in bitmap.iter().enumerate() {
        for bit in 0..8 {
            let absolute_block = i * 8 + bit;
            if byte & (1u8 << bit) == 0 {
                if run_length == 0 {
                    run_start = absolute_block;
                }
                run_length += 1;
                if run_length == remaining {
                    blocks.push(Extent {
                        logical_start,
                        physical_start: run_start as u32,
                        length: run_length as u32,
                    });
                    return blocks;
                }
            } else if run_length > 0 {
                blocks.push(Extent {
                    logical_start,
                    physical_start: run_start as u32,
                    length: run_length as u32,
                });
                logical_start += run_length as u32;
                remaining -= run_length;
                run_length = 0;
            }
        }
    }
    if run_length > 0 {
        blocks.push(Extent {
            logical_start,
            physical_start: run_start as u32,
            length: run_length as u32,
        });
        remaining -= run_length;
    }
    if remaining == 0 {
        blocks
    } else {
        Vec::new()
    }
}

pub fn mark_blocks_used(bitmap: &mut [u8], extents: &[Extent]) {
    for extent in extents {
        let start = extent.physical_start as usize;
        let length = extent.length as usize;
        for block in start..start + length {
            let byte_idx = block / 8;
            let bit_idx = block % 8;
            bitmap[byte_idx] |= 1 << bit_idx;
        }
    }
}

pub fn mark_blocks_free(bitmap: &mut [u8], extents: &[Extent]) {
    for extent in extents {
        let start = extent.physical_start as usize;
        let length = extent.length as usize;
        for block in start..start + length {
            let byte_idx = block / 8;
            let bit_idx = block % 8;
            bitmap[byte_idx] &= !(1 << bit_idx);
        }
    }
}

pub fn mark_block_free(bitmap: &mut [u8], block: usize) {
    bitmap[block / 8] &= !(1 << (block % 8));
}

pub fn mark_inode_used(bitmap: &mut [u8], inode: usize) {
    let byte_idx = inode / 8;
    let bit_idx = inode % 8;
    bitmap[byte_idx] |= 1 << bit_idx;
}

pub fn mark_inode_free(bitmap: &mut [u8], inode: usize) {
    let byte_idx = inode / 8;
    let bit_idx = inode % 8;
    bitmap[byte_idx] &= !(1 << bit_idx);
}
