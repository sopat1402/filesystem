use crate::block::{Block, SuperBlock};
use crate::file_errors::FileError;
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::FileExt;

const DEFAULT_CAPACITY: usize = 256;

type SlotId = usize;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum CacheKey {
    Superblock,
    Block(u64),
}

enum Payload {
    Superblock(SuperBlock),
    Block(Block),
}

struct CacheEntry {
    key:     CacheKey,
    payload: Payload,
    dirty:   bool,
    newer:   Option<SlotId>, // towards most recently used
    older:   Option<SlotId>, // towards least recently used
}

impl CacheEntry {
    fn write_back(&mut self, disk: &File) -> Result<(), FileError> {
        match &mut self.payload {
            Payload::Block(block) => block.write_block(disk)?,
            Payload::Superblock(sb) => {
                let buf = sb.serialise();
                disk.write_all_at(&buf, 0).map_err(|_| FileError::WriteError)?;
            }
        }
        self.dirty = false;
        Ok(())
    }
}

pub struct BlockCache {
    disk:       File,
    capacity:   usize,
    entries:    Vec<Option<CacheEntry>>,
    free_slots: Vec<SlotId>,
    index:      HashMap<CacheKey, SlotId>,
    newest:     Option<SlotId>,
    oldest:     Option<SlotId>,
}

impl BlockCache {
    pub fn new(disk: File) -> Self {
        Self::with_capacity(disk, DEFAULT_CAPACITY)
    }

    pub fn with_capacity(disk: File, capacity: usize) -> Self {
        assert!(capacity >= 1, "cache capacity must be at least 1");
        Self {
            disk,
            capacity,
            entries: Vec::new(),
            free_slots: Vec::new(),
            index: HashMap::new(),
            newest: None,
            oldest: None,
        }
    }

    fn entry(&self, slot: SlotId) -> &CacheEntry {
        self.entries[slot].as_ref().expect("slot is empty")
    }

    fn entry_mut(&mut self, slot: SlotId) -> &mut CacheEntry {
        self.entries[slot].as_mut().expect("slot is empty")
    }

    fn detach(&mut self, slot: SlotId) {
        let (newer, older) = {
            let e = self.entry(slot);
            (e.newer, e.older)
        };
        match newer {
            Some(n) => self.entry_mut(n).older = older,
            None => self.newest = older,
        }
        match older {
            Some(o) => self.entry_mut(o).newer = newer,
            None => self.oldest = newer,
        }
    }

    fn attach_newest(&mut self, slot: SlotId) {
        let previous_newest = self.newest;
        {
            let e = self.entry_mut(slot);
            e.newer = None;
            e.older = previous_newest;
        }
        if let Some(p) = previous_newest {
            self.entry_mut(p).newer = Some(slot);
        }
        self.newest = Some(slot);
        if self.oldest.is_none() {
            self.oldest = Some(slot);
        }
    }

    fn mark_used(&mut self, slot: SlotId) {
        if self.newest != Some(slot) {
            self.detach(slot);
            self.attach_newest(slot);
        }
    }

    fn evict_until_room(&mut self) -> Result<(), FileError> {
        while self.index.len() >= self.capacity {
            let Some(slot) = self.oldest else { break };
            if self.entry(slot).dirty {
                self.entries[slot].as_mut().unwrap().write_back(&self.disk)?;
            }
            self.detach(slot);
            let victim = self.entries[slot].take().unwrap();
            self.index.remove(&victim.key);
            self.free_slots.push(slot);
        }
        Ok(())
    }

    fn claim_slot(&mut self) -> SlotId {
        self.free_slots.pop().unwrap_or_else(|| {
            self.entries.push(None);
            self.entries.len() - 1
        })
    }

    fn fetch(&mut self, key: CacheKey) -> Result<SlotId, FileError> {
        if let Some(&slot) = self.index.get(&key) {
            self.mark_used(slot);
            return Ok(slot);
        }

        let payload = match key {
            CacheKey::Superblock => Payload::Superblock(SuperBlock::deserialise(&self.disk)?),
            CacheKey::Block(id) => Payload::Block(Block::deserialise(&self.disk, id)?),
        };

        self.evict_until_room()?;
        let slot = self.claim_slot();
        self.entries[slot] = Some(CacheEntry { key, payload, dirty: false, newer: None, older: None });
        self.attach_newest(slot);
        self.index.insert(key, slot);
        Ok(slot)
    }

    pub fn get(&mut self, block_id: u64) -> Result<&Block, FileError> {
        let slot = self.fetch(CacheKey::Block(block_id))?;
        match &self.entry(slot).payload {
            Payload::Block(block) => Ok(block),
            Payload::Superblock(_) => unreachable!(),
        }
    }

    pub fn get_mut(&mut self, block_id: u64) -> Result<&mut Block, FileError> {
        let slot = self.fetch(CacheKey::Block(block_id))?;
        let entry = self.entry_mut(slot);
        entry.dirty = true;
        match &mut entry.payload {
            Payload::Block(block) => Ok(block),
            Payload::Superblock(_) => unreachable!(),
        }
    }

    pub fn get_superblock(&mut self) -> Result<&SuperBlock, FileError> {
        let slot = self.fetch(CacheKey::Superblock)?;
        match &self.entry(slot).payload {
            Payload::Superblock(sb) => Ok(sb),
            Payload::Block(_) => unreachable!(),
        }
    }

    pub fn get_superblock_mut(&mut self) -> Result<&mut SuperBlock, FileError> {
        let slot = self.fetch(CacheKey::Superblock)?;
        let entry = self.entry_mut(slot);
        entry.dirty = true;
        match &mut entry.payload {
            Payload::Superblock(sb) => Ok(sb),
            Payload::Block(_) => unreachable!(),
        }
    }

    pub fn flush(&mut self) -> Result<(), FileError> {
        for entry in self.entries.iter_mut().flatten() {
            if entry.dirty {
                entry.write_back(&self.disk)?;
            }
        }
        self.disk.sync_all().map_err(|_| FileError::WriteError)
    }
}

impl Drop for BlockCache {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
