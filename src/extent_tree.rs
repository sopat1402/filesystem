use crate::block::{Block};
use crate::file_errors::FileError;
use std::fs::File;
use std::os::unix::prelude::FileExt;
use crate::constants::*;

const _: () = assert!(
    NODE_HEADER_SIZE + ROOT_MAX_ENTRIES * ENTRY_SIZE + NON_EXTENT_FIELDS_SIZE == INODE_SIZE,
    "extent tree root budget doesn't exactly fill INODE_SIZE"
);

pub enum InsertResult {
    Done { new_min: u32 },
    Split { key: u32, new_block: u32, new_min: u32 },
}

pub enum DeleteResult { 
    Done,
    Underflow,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Extent {
    pub logical_start: u32,
    pub physical_start: u32,
    pub length: u32,
}

impl Extent {
    pub fn to_bytes(&self) -> [u8; ENTRY_SIZE] {
        let mut b = [0u8; ENTRY_SIZE];
        b[0..4].copy_from_slice(&self.logical_start.to_le_bytes());
        b[4..8].copy_from_slice(&self.physical_start.to_le_bytes());
        b[8..12].copy_from_slice(&self.length.to_le_bytes());
        b
    }
    pub fn from_bytes(b: &[u8; ENTRY_SIZE]) -> Self {
        Self {
            logical_start: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            physical_start: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            length: u32::from_le_bytes(b[8..12].try_into().unwrap()),
        }
    }
    fn logical_end(&self) -> u32 { self.logical_start + self.length }
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IndexEntry {
    pub logical_start: u32,
    pub child_block: u32,
    pub _reserved: u32,
}

impl IndexEntry {
    pub fn to_bytes(&self) -> [u8; ENTRY_SIZE] {
        let mut b = [0u8; ENTRY_SIZE];
        b[0..4].copy_from_slice(&self.logical_start.to_le_bytes());
        b[4..8].copy_from_slice(&self.child_block.to_le_bytes());
        b[8..12].copy_from_slice(&self._reserved.to_le_bytes());
        b
    }
    pub fn from_bytes(b: &[u8; ENTRY_SIZE]) -> Self {
        Self {
            logical_start: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            child_block: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            _reserved: u32::from_le_bytes(b[8..12].try_into().unwrap()),
        }
    }
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

    pub fn read_node_with_block(disk: &File, block_num: u32) -> Result<(Block, Self), FileError> {
        let block = Block::deserialise(disk, block_num as usize)?;
        let mut offset = BLOCK_HEADER_SIZE;
        let magic = u16::from_le_bytes(block.buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset += 2;
        if magic != TREE_MAGIC {
            return Err(FileError::CorruptedBlock);
        }
        let depth = u16::from_le_bytes(block.buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset += 2;
        let entry_count = u16::from_le_bytes(block.buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset += 2;
        let max_entries = u16::from_le_bytes(block.buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset += 2;
        if entry_count > max_entries || max_entries as usize > BLOCK_MAX_ENTRIES {
            return Err(FileError::CorruptedBlock);
        }
        let mut entries = vec![[0u8; ENTRY_SIZE]; entry_count as usize];
        for entry in &mut entries {
            entry.copy_from_slice(&block.buf[offset..offset + ENTRY_SIZE]);
            offset += ENTRY_SIZE;
        }
        let node = Self { magic, depth, entry_count, max_entries, entries };
        Ok((block, node))
    }

    pub fn read_node(disk: &File, block_num: u32) -> Result<Self, FileError> {
        Self::read_node_with_block(disk, block_num).map(|(_, node)| node)
    }

    pub fn is_leaf(&self) -> bool { self.depth == 0 }

    pub fn get_extent(&self, i: usize) -> Extent { Extent::from_bytes(&self.entries[i]) }

    pub fn get_index(&self, i: usize) -> IndexEntry { IndexEntry::from_bytes(&self.entries[i]) }
}

pub fn write_external_block(
    disk: &File,
    block: &mut Block,
    ext_block: &ExtentTreeNode
) -> Result<(), FileError> {
    let serialised = ext_block.serialise();
    let end = BLOCK_HEADER_SIZE + serialised.len();
    block.buf[BLOCK_HEADER_SIZE..end].copy_from_slice(&serialised);
    block.serialise();
    disk.write_all_at(&block.buf, (block.id * BLOCK_SIZE) as u64).map_err(|_| FileError::WriteError)?;
    Ok(())
}

pub fn range_lookup(
    disk: &File,
    node: &ExtentTreeNode,
    start: u32,
    end: u32,
) -> Result<Vec<Extent>, FileError> {
    let mut out = Vec::new();
    collect_range(disk, node, start, end, &mut out)?;
    Ok(out)
}

fn collect_range(
    disk: &File,
    node: &ExtentTreeNode,
    start: u32,
    end: u32,
    out: &mut Vec<Extent>,
) -> Result<(), FileError> {
    if node.is_leaf() {
        for i in 0..node.entries.len() {
            let e = node.get_extent(i);
            if e.logical_start < end && e.logical_end() > start {
                out.push(e);
            }
        }
        return Ok(());
    }
    let first = find_child(node, start);
    for i in first..node.entries.len() {
        let child_key = node.get_index(i).logical_start;
        if i > first && child_key >= end {
            break;
        }
        let child = ExtentTreeNode::read_node(disk, node.get_index(i).child_block)?;
        collect_range(disk, &child, start, end, out)?;
    }
    Ok(())
}

fn find_child(node: &ExtentTreeNode, logical_start: u32) -> usize {
    (0..node.entries.len())
        .rev()
        .find(|&i| node.get_index(i).logical_start <= logical_start)
        .unwrap_or(0)
}
fn insert_into_node(
    disk: &File,
    node: &mut ExtentTreeNode,
    node_block: Option<u32>,
    new_ext: Extent,
    alloc_block: &mut impl FnMut() -> Result<u32, FileError>,
    freed: &mut Vec<Extent>,
) -> Result<InsertResult, FileError> {
    let max_entries = if node_block.is_some() { BLOCK_MAX_ENTRIES as u16 } else { ROOT_MAX_ENTRIES as u16 };

    if node.is_leaf() {
        return insert_leaf(disk, node, new_ext, max_entries, alloc_block, freed);
    }

    let i = find_child(node, new_ext.logical_start);
    let idx = node.get_index(i);
    let (mut blk, mut child) = ExtentTreeNode::read_node_with_block(disk, idx.child_block)?;

    let mut indices: Vec<IndexEntry> = node.entries.iter().map(IndexEntry::from_bytes).collect();

    let result = match insert_into_node(disk, &mut child, Some(idx.child_block), new_ext, alloc_block, freed)? {
        InsertResult::Done { new_min } => {
            write_external_block(disk, &mut blk, &child)?;
            indices[i].logical_start = new_min;
            InsertResult::Done { new_min: indices[0].logical_start }
        }
        InsertResult::Split { key, new_block, new_min } => {
            write_external_block(disk, &mut blk, &child)?;
            indices[i].logical_start = new_min;

            let pos = indices.partition_point(|e| e.logical_start < key);
            indices.insert(pos, IndexEntry { logical_start: key, child_block: new_block, _reserved: 0 });

            if indices.len() <= max_entries as usize {
                InsertResult::Done { new_min: indices[0].logical_start }
            } else {
                let mid = indices.len() / 2;
                let right = indices.split_off(mid);
                let split_key = right[0].logical_start;

                let right_block = alloc_block()?;
                let right_node = ExtentTreeNode {
                    magic: TREE_MAGIC,
                    depth: node.depth,
                    entry_count: right.len() as u16,
                    max_entries: BLOCK_MAX_ENTRIES as u16,
                    entries: right.iter().map(IndexEntry::to_bytes).collect(),
                };
                let mut rblk = Block::deserialise(disk, right_block as usize)?;
                write_external_block(disk, &mut rblk, &right_node)?;

                InsertResult::Split { key: split_key, new_block: right_block, new_min: indices[0].logical_start }
            }
        }
    };

    node.entries = indices.iter().map(IndexEntry::to_bytes).collect();
    node.entry_count = indices.len() as u16;
    Ok(result)
}

fn insert_leaf(
    disk: &File,
    node: &mut ExtentTreeNode,
    new_ext: Extent,
    max_entries: u16,
    alloc_block: &mut impl FnMut() -> Result<u32, FileError>,
    freed: &mut Vec<Extent>,
) -> Result<InsertResult, FileError> {
    let existing: Vec<Extent> = node.entries.iter().map(Extent::from_bytes).collect();
    let new_start = new_ext.logical_start;
    let new_end = new_ext.logical_end();
    let mut extents: Vec<Extent> = Vec::with_capacity(existing.len() + 2);
    for e in existing {
        let e_start = e.logical_start;
        let e_end = e.logical_end();
        if e_end <= new_start || e_start >= new_end {
            extents.push(e);
            continue;
        }
        let overlap_start = e_start.max(new_start);
        let overlap_end = e_end.min(new_end);
        freed.push(Extent {
            logical_start: overlap_start,
            physical_start: e.physical_start + (overlap_start - e_start),
            length: overlap_end - overlap_start,
        });
        if e_start < new_start {
            extents.push(Extent {
                logical_start: e_start,
                physical_start: e.physical_start,
                length: new_start - e_start,
            });
        }
        if e_end > new_end {
            let logical_offset = new_end - e_start;
            extents.push(Extent {
                logical_start: new_end,
                physical_start: e.physical_start + logical_offset,
                length: e_end - new_end,
            });
        }
    }
    let pos = extents.partition_point(|e| e.logical_start < new_start);
    let merges_with_prev = pos > 0 && {
        let prev = &extents[pos - 1];
        prev.logical_end() == new_start
            && prev.physical_start + prev.length == new_ext.physical_start
    };
    let merges_with_next = pos < extents.len() && {
        let next = &extents[pos];
        new_end == next.logical_start
            && new_ext.physical_start + new_ext.length == next.physical_start
    };
    match (merges_with_prev, merges_with_next) {
        (true, true) => {
            let next = extents.remove(pos);
            extents[pos - 1].length += new_ext.length + next.length;
        }
        (true, false) => {
            extents[pos - 1].length += new_ext.length;
        }
        (false, true) => {
            extents[pos].logical_start = new_start;
            extents[pos].physical_start = new_ext.physical_start;
            extents[pos].length += new_ext.length;
        }
        (false, false) => {
            extents.insert(pos, new_ext);
        }
    }
    if extents.len() <= max_entries as usize {
        node.entries = extents.iter().map(Extent::to_bytes).collect();
        node.entry_count = extents.len() as u16;
        Ok(InsertResult::Done { new_min: extents[0].logical_start })
    } else {
        let mid = extents.len() / 2;
        let right = extents.split_off(mid);
        let key = right[0].logical_start;
        node.entries = extents.iter().map(Extent::to_bytes).collect();
        node.entry_count = extents.len() as u16;

        let right_block = alloc_block()?;
        let right_node = ExtentTreeNode {
            magic: TREE_MAGIC,
            depth: node.depth,
            entry_count: right.len() as u16,
            max_entries: BLOCK_MAX_ENTRIES as u16,
            entries: right.iter().map(Extent::to_bytes).collect(),
        };
        let mut blk = Block::deserialise(disk, right_block as usize)?;
        write_external_block(disk, &mut blk, &right_node)?;
        Ok(InsertResult::Split {
            key,
            new_block: right_block,
            new_min: extents[0].logical_start,
        })
    }
}
pub fn insert_extent(
    disk: &File,
    root: &mut ExtentTreeNode,
    new_ext: Extent,
    alloc_block: &mut impl FnMut() -> Result<u32, FileError>,
) -> Result<Vec<Extent>, FileError> {
    let mut freed: Vec<Extent> = Vec::new();
    match insert_into_node(disk, root, None, new_ext, alloc_block, &mut freed)? {
        InsertResult::Done { .. } => Ok(freed),
        InsertResult::Split { key, new_block, .. } => {
            let left_block = alloc_block()?;
            let left_node = ExtentTreeNode {
                magic: TREE_MAGIC,
                depth: root.depth,
                entry_count: root.entry_count,
                max_entries: BLOCK_MAX_ENTRIES as u16,
                entries: root.entries.clone(),
            };
            let mut blk = Block::deserialise(disk, left_block as usize)?;
            write_external_block(disk, &mut blk, &left_node)?;
            let left_key = if root.is_leaf() {
                left_node.get_extent(0).logical_start
            } else {
                left_node.get_index(0).logical_start
            };
            let left_idx = IndexEntry { logical_start: left_key, child_block: left_block, _reserved: 0 };
            let right_idx = IndexEntry { logical_start: key, child_block: new_block, _reserved: 0 };
            root.depth += 1;
            root.max_entries = ROOT_MAX_ENTRIES as u16;
            root.entry_count = 2;
            root.entries = vec![left_idx.to_bytes(), right_idx.to_bytes()];
            Ok(freed)
        }
    }
}

pub fn lookup_extent(disk: &File, node: &ExtentTreeNode, logical: u32) -> Result<Option<Extent>, FileError> {
    if node.is_leaf() {
        Ok(node.entries.iter()
            .map(Extent::from_bytes)
            .find(|e| e.logical_start <= logical && logical < e.logical_end()))
    } else {
        let i = find_child(node, logical);
        let child = ExtentTreeNode::read_node(disk, node.get_index(i).child_block)?;
        lookup_extent(disk, &child, logical)
    }
}

fn min_entries(max_entries: u16) -> u16 { max_entries / 2 }

fn delete_leaf(node: &mut ExtentTreeNode, start: u32, end: u32, freed: &mut Vec<Extent>) -> DeleteResult {
    let existing: Vec<Extent> = node.entries.iter().map(Extent::from_bytes).collect();
    let mut remaining: Vec<Extent> = Vec::with_capacity(existing.len() + 1);

    for e in existing {
        let (e_start, e_end) = (e.logical_start, e.logical_end());
        if e_end <= start || e_start >= end {
            remaining.push(e);
            continue;
        }
        let overlap_start = e_start.max(start);
        let overlap_end = e_end.min(end);
        freed.push(Extent {
            logical_start: overlap_start,
            physical_start: e.physical_start + (overlap_start - e_start),
            length: overlap_end - overlap_start,
        });
        if e_start < start {
            remaining.push(Extent { logical_start: e_start, physical_start: e.physical_start, length: start - e_start });
        }
        if e_end > end {
            let off = end - e_start;
            remaining.push(Extent { logical_start: end, physical_start: e.physical_start + off, length: e_end - end });
        }
    }
    node.entries = remaining.iter().map(Extent::to_bytes).collect();
    node.entry_count = remaining.len() as u16;

    if node.entry_count < min_entries(node.max_entries) { DeleteResult::Underflow } else { DeleteResult::Done }
}

fn delete_from_node(
    disk: &File,
    node: &mut ExtentTreeNode,
    node_block: Option<u32>,
    start: u32,
    end: u32,
    free_block: &mut impl FnMut(u32) -> Result<(), FileError>,
    freed: &mut Vec<Extent>,
) -> Result<DeleteResult, FileError> {
    let max_entries = if node_block.is_some() { BLOCK_MAX_ENTRIES as u16} else { ROOT_MAX_ENTRIES as u16};
    if node.is_leaf() {
        return Ok(delete_leaf(node, start, end, freed));
    }
    let i = find_child(node, start);
    let idx = node.get_index(i);
    let (mut child_blk, mut child) = ExtentTreeNode::read_node_with_block(disk, idx.child_block)?;
    let child_result = delete_from_node(disk, &mut child, Some(idx.child_block), start, end, free_block, freed)?;
    match child_result {
        DeleteResult::Done => {
            write_external_block(disk, &mut child_blk, &child)?;
            Ok(DeleteResult::Done)
        }
        DeleteResult::Underflow => {
            let mut indices: Vec<IndexEntry> = node.entries.iter().map(IndexEntry::from_bytes).collect();
            let sibling_min = min_entries(BLOCK_MAX_ENTRIES as u16);
            if i > 0 {
                let (mut left_blk, mut left) = ExtentTreeNode::read_node_with_block(disk, indices[i - 1].child_block)?;
                if left.entry_count > sibling_min {
                    let borrowed = left.entries.pop().unwrap();
                    left.entry_count -= 1;
                    child.entries.insert(0, borrowed);
                    child.entry_count += 1;
                    indices[i].logical_start = if child.is_leaf() { child.get_extent(0).logical_start } else { child.get_index(0).logical_start };
                    node.entries = indices.iter().map(IndexEntry::to_bytes).collect();
                    write_external_block(disk, &mut left_blk, &left)?;
                    write_external_block(disk, &mut child_blk, &child)?;
                    return Ok(DeleteResult::Done);
                }
            }
            if i + 1 < indices.len() {
                let (mut right_blk, mut right) = ExtentTreeNode::read_node_with_block(disk, indices[i + 1].child_block)?;
                if right.entry_count > sibling_min {
                    let borrowed = right.entries.remove(0);
                    right.entry_count -= 1;
                    child.entries.push(borrowed);
                    child.entry_count += 1;
                    indices[i + 1].logical_start = if right.is_leaf() { right.get_extent(0).logical_start } else { right.get_index(0).logical_start };
                    node.entries = indices.iter().map(IndexEntry::to_bytes).collect();
                    write_external_block(disk, &mut right_blk, &right)?;
                    write_external_block(disk, &mut child_blk, &child)?;
                    return Ok(DeleteResult::Done);
                }
            }
            if i > 0 {
                let (mut left_blk, mut left) = ExtentTreeNode::read_node_with_block(disk, indices[i - 1].child_block)?;
                left.entries.extend(child.entries.iter().cloned());
                left.entry_count = left.entries.len() as u16;
                write_external_block(disk, &mut left_blk, &left)?;
                free_block(idx.child_block)?;
                indices.remove(i);
            } else {
                let (right_blk, right) = ExtentTreeNode::read_node_with_block(disk, indices[i + 1].child_block)?;
                child.entries.extend(right.entries.iter().cloned());
                child.entry_count = child.entries.len() as u16;
                write_external_block(disk, &mut child_blk, &child)?;
                free_block(indices[i + 1].child_block)?;
                indices.remove(i + 1);
                let _ = right_blk;
            }
            node.entries = indices.iter().map(IndexEntry::to_bytes).collect();
            node.entry_count = indices.len() as u16;
            if node.entry_count < min_entries(max_entries) { Ok(DeleteResult::Underflow) } else { Ok(DeleteResult::Done) }
        }
    }
}

pub fn delete_extent_range(
    disk: &File,
    root: &mut ExtentTreeNode,
    start: u32,
    end: u32,
    free_block: &mut impl FnMut(u32) -> Result<(), FileError>,
) -> Result<Vec<Extent>, FileError> {
    let mut freed = Vec::new();
    if let DeleteResult::Underflow = delete_from_node(disk, root, None, start, end, free_block, &mut freed)? {
        if !root.is_leaf() && root.entry_count == 1 {
            let only_child_block = root.get_index(0).child_block;
            let child = ExtentTreeNode::read_node(disk, only_child_block)?;
            if child.entry_count as usize <= ROOT_MAX_ENTRIES {
                root.depth = child.depth;
                root.entries = child.entries;
                root.entry_count = child.entry_count;
                root.max_entries = ROOT_MAX_ENTRIES as u16;
                free_block(only_child_block)?;
            }
        }
    }
    Ok(freed)
}
