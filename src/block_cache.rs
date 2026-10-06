use crate::block::{SuperBlock, Block};
use crate::file_errors::FileError;
use std::collections::HashMap;

const MAX_ENTRIES: usize = 64;

struct CacheEntry {
    key   : u64,
    block : Block,
    dirty : bool,
    prev  : Option<usize>,
    next  : Option<usize>,
}

struct Lru {
    head    : Option<usize>,
    tail    : Option<usize>,
    entries : Vec<Option<CacheEntry>>,
    trash   : Vec<usize>,
    map     : HashMap<u64, usize>,
}

impl Lru {
    fn new() -> Self {
        Self { head: None, tail: None, entries: Vec::new(), trash: Vec::new(), map: HashMap::new() }
    }

    fn unlink(&mut self, idx: usize) {
        let (prev, next) = {
            let e = self.entries[idx].as_ref().unwrap();
            (e.prev, e.next)
        };
        match prev {
            Some(p) => self.entries[p].as_mut().unwrap().next = next,
            None => self.head = next,
        }
        match next {
            Some(n) => self.entries[n].as_mut().unwrap().prev = prev,
            None => self.tail = prev,
        }
    }

    fn push_head(&mut self, idx: usize) {
        let old_head = self.head;
        {
            let e = self.entries[idx].as_mut().unwrap();
            e.prev = None;
            e.next = old_head;
        }
        if let Some(h) = old_head {
            self.entries[h].as_mut().unwrap().prev = Some(idx);
        }
        self.head = Some(idx);
        if self.tail.is_none() {
            self.tail = Some(idx);
        }
    }

    fn move_to_head(&mut self, idx: usize) {
        if self.head == Some(idx) {
            return;
        }
        self.unlink(idx);
        self.push_head(idx);
    }

    fn evict_tail(&mut self) -> Option<CacheEntry> {
        let tail_idx = self.tail?;
        self.unlink(tail_idx);
        let entry = self.entries[tail_idx].take().unwrap();
        self.map.remove(&entry.key);
        self.trash.push(tail_idx);
        Some(entry)
    }

    fn alloc_slot(&mut self) -> usize {
        if let Some(slot) = self.trash.pop() {
            slot
        } else {
            self.entries.push(None);
            self.entries.len() - 1
        }
    }

    fn insert(&mut self, key: u64, block: Block, dirty: bool) -> Option<CacheEntry> {
        let evicted = if self.entries.len() - self.trash.len() >= MAX_ENTRIES {
            self.evict_tail()
        } else {
            None
        };
        let idx = self.alloc_slot();
        self.entries[idx] = Some(CacheEntry { key, block, dirty, prev: None, next: None });
        self.push_head(idx);
        self.map.insert(key, idx);
        evicted
    }

    fn index_of(&mut self, key: u64) -> Option<usize> {
        let idx = *self.map.get(&key)?;
        self.move_to_head(idx);
        Some(idx)
    }
}

pub struct BlockCache {
    disk       : std::fs::File,
    data_start : u64,
    superblock : Option<(SuperBlock, bool)>,
    pinned     : HashMap<u64, CacheEntry>,
    lru        : Lru,
}

impl BlockCache {
    pub fn new(disk: std::fs::File, data_start: u64) -> Self {
        Self { disk, data_start, superblock: None, pinned: HashMap::new(), lru: Lru::new() }
    }

    fn is_pinned(&self, block_id: u64) -> bool {
        block_id < self.data_start
    }

    fn fetch_pinned(&mut self, block_id: u64) -> Result<(), FileError> {
        if self.pinned.contains_key(&block_id) {
            return Ok(());
        }
        let block = Block::deserialise(&self.disk, block_id)?;
        self.pinned.insert(block_id, CacheEntry { key: block_id, block, dirty: false, prev: None, next: None });
        Ok(())
    }

    fn fetch_lru(&mut self, block_id: u64) -> Result<(), FileError> {
        if self.lru.index_of(block_id).is_some() {
            return Ok(());
        }
        let block = Block::deserialise(&self.disk, block_id)?;
        if let Some(mut evicted) = self.lru.insert(block_id, block, false) {
            if evicted.dirty {
                evicted.block.write_block(&self.disk)?;
            }
        }
        Ok(())
    }

    pub fn get(&mut self, block_id: u64) -> Result<&Block, FileError> {
        if self.is_pinned(block_id) {
            self.fetch_pinned(block_id)?;
            return Ok(&self.pinned[&block_id].block);
        }
        self.fetch_lru(block_id)?;
        let idx = self.lru.map[&block_id];
        Ok(&self.lru.entries[idx].as_ref().unwrap().block)
    }

    pub fn get_mut(&mut self, block_id: u64) -> Result<&mut Block, FileError> {
        if self.is_pinned(block_id) {
            self.fetch_pinned(block_id)?;
            let entry = self.pinned.get_mut(&block_id).unwrap();
            entry.dirty = true;
            return Ok(&mut entry.block);
        }
        self.fetch_lru(block_id)?;
        let idx = self.lru.map[&block_id];
        let entry = self.lru.entries[idx].as_mut().unwrap();
        entry.dirty = true;
        Ok(&mut entry.block)
    }

    pub fn get_superblock(&mut self) -> Result<&SuperBlock, FileError> {
        if self.superblock.is_none() {
            let sb = SuperBlock::deserialise(&self.disk)?;
            self.superblock = Some((sb, false));
        }
        Ok(&self.superblock.as_ref().unwrap().0)
    }

    pub fn get_superblock_mut(&mut self) -> Result<&mut SuperBlock, FileError> {
        if self.superblock.is_none() {
            let sb = SuperBlock::deserialise(&self.disk)?;
            self.superblock = Some((sb, false));
        }
        let entry = self.superblock.as_mut().unwrap();
        entry.1 = true;
        Ok(&mut entry.0)
    }

    pub fn flush(&mut self) -> Result<(), FileError> {
        if let Some((sb, dirty)) = self.superblock.as_mut() {
            if *dirty {
                let buf = sb.serialise();
                use std::os::unix::prelude::FileExt;
                self.disk.write_all_at(&buf, 0).map_err(|_| FileError::WriteError)?;
                *dirty = false;
            }
        }
        for entry in self.pinned.values_mut() {
            if entry.dirty {
                entry.block.write_block(&self.disk)?;
                entry.dirty = false;
            }
        }
        for slot in self.lru.entries.iter_mut() {
            if let Some(entry) = slot {
                if entry.dirty {
                    entry.block.write_block(&self.disk)?;
                    entry.dirty = false;
                }
            }
        }
        Ok(())
    }
}
