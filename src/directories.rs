use crate::constants::*;
use crate::file_errors::FileError;
use crate::inode::{find_inode,write_inode,Stat,stat};
use crate::extent_tree::{range_lookup,insert_extent,delete_extent_range};
use crate::block::{Block,SuperBlock};
use crate::filesystem::reserve_inode;
use crate::bitmaps::*;
use std::fs::File;
use std::os::unix::prelude::FileExt;

pub fn is_dir(mode: u16) -> bool {
    mode & 0o170000 == S_IFDIR
}

pub fn read_dir(disk:&File,inode_id:u64)->Result<Vec<(String,u64)>,FileError>{
    let node=find_inode(disk,inode_id)?;
    if !is_dir(node.i_mode){
        return Err(FileError::NotDirectory);
    }
    let extents=range_lookup(disk,&node.i_extents,0,u64::MAX)?;
    let mut dirents:Vec<(String,u64)>=Vec::new();
    for extent in extents {
        for b in extent.physical_start..extent.physical_start+extent.length {
            let block=Block::deserialise(disk,b)?;
            let payload=&block.buf[BLOCK_HEADER_SIZE..];
            let mut offset=0usize;
            while offset+2<=payload.len(){
                let name_len=u16::from_le_bytes([payload[offset],payload[offset+1]]) as usize;
                if name_len==0{
                    break;
                }
                let name_start=offset+2;
                let name_end=name_start+name_len;
                let entry_end=name_end+8;
                if entry_end>payload.len(){
                    return Err(FileError::CorruptedBlock);
                }
                let name=String::from_utf8(payload[name_start..name_end].to_vec())
                    .map_err(|_| FileError::CorruptedBlock)?;
                let inode_num=u64::from_le_bytes(
                    payload[name_end..entry_end].try_into().map_err(|_| FileError::CorruptedBlock)?
                );
                dirents.push((name,inode_num));
                offset=entry_end;
            }
        }
    }
    Ok(dirents)
}

pub fn print_dir(disk:&File,inode_id:u64)->Result<(),FileError>{
    let dirents=read_dir(disk,inode_id)?;
    for (dirent,_) in dirents{
        print!("{}\t",dirent);
    }
    println!();
    Ok(())
}

pub fn free_data_extents(
    disk: &File,
    superblock: &mut SuperBlock,
    extents: &[crate::extent_tree::Extent],
) -> Result<(), FileError> {
    for extent in extents {
        for block_id in extent.physical_start..extent.physical_start + extent.length {
            let mut block = Block::deserialise(disk, block_id)?;
            block.buf[BLOCK_HEADER_SIZE..].fill(0);
            block.write_block(disk)?;
            mark_block_free(disk,block_id)?;
            superblock.free_blocks += 1;
        }
    }
    Ok(())
}

pub fn lookup(disk: &File, parent: u64, name: &str) -> Result<Stat, FileError> {
    let child = read_dir(disk, parent)?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, id)| id)
        .ok_or(FileError::NameNotFound)?;
    stat(disk, child)
}

pub fn delete_dirent(disk: &File, inode_id: u64) -> Result<(), FileError> {
    let mut node = find_inode(disk, inode_id)?;
    let mut superblock = SuperBlock::deserialise(disk)?;
    if is_dir(node.i_mode) {
        let dirents = read_dir(disk, inode_id)?;
        for (name, id) in dirents {
            if name == "." || name == ".." {
                continue;
            }
            delete_dirent(disk, id)?;
            superblock = SuperBlock::deserialise(disk)?;
        }
    }

    let freed = {
        let mut free_block = |block_id: u64| -> Result<(), FileError> {
            mark_block_free(
                disk,
                block_id,
            )?;
            superblock.free_blocks += 1;
            Ok(())
        };
        delete_extent_range(
            disk,
            &mut node.i_extents,
            0,
            u64::MAX,
            &mut free_block,
        )?
    };
    free_data_extents(disk, &mut superblock, &freed)?;
    mark_inode_free(disk,inode_id)?;
    superblock.free_inodes += 1;
    let buf = superblock.serialise();
    disk.write_all_at(&buf, 0)
        .map_err(|_| FileError::WriteError)?;
    Ok(())
}

pub fn delete(disk:&File, parent_inode:u64, name:String) -> Result<(), FileError> {
    if name==String::from(".") || name==String::from(".."){
        return Err(FileError::NameExists);
    }
    let mut res = read_dir(disk, parent_inode)?;
    let idx = res.iter().position(|(entry_name, _)| *entry_name == name)
        .ok_or(FileError::NameNotFound)?;
    let (_, child_inode) = res[idx];
    let child = find_inode(disk, child_inode)?;
    let child_is_dir = is_dir(child.i_mode);
    delete_dirent(disk, child_inode)?;
    res.remove(idx);
    write_dirents(disk, parent_inode, res)?;
    if child_is_dir {
        let mut parent = find_inode(disk, parent_inode)?;
        parent.i_links_count -= 1;
        write_inode(disk, parent_inode, &parent.serialise())?;
    }
    Ok(())
}

fn allocate_block(disk: &File, superblock: &mut SuperBlock) -> Result<u64, FileError> {
    let new_extent = find_blocks(disk, 1)?;
    if new_extent.is_empty() {
        return Err(FileError::NoMoreBlocks);
    }
    mark_blocks_used(disk, &new_extent)?;
    superblock.free_blocks = superblock
        .free_blocks
        .checked_sub(1)
        .ok_or(FileError::NoMoreBlocks)?;
    Ok(new_extent[0].physical_start)
}

pub fn add_dirent(disk:&File,directory_inode:u64,new_name:String,mode:u16,uid:u32,gid:u32)->Result<u64,FileError>{
    if new_name.len()==0{
        return Err(FileError::NameExists);
    }
    if new_name.len()>255{
        return Err(FileError::NameTooLong);
    }
    let dirents=read_dir(disk,directory_inode)?;
    for (name,_) in &dirents{
        if *name==new_name{
            return Err(FileError::NameExists);
        }
    }
    let node=reserve_inode(disk,mode,uid,gid)?;
    let mut new_inode=find_inode(disk,node)?;
    new_inode.i_links_count=1;
    write_inode(disk, node, &new_inode.serialise())?;
    let mut superblock=SuperBlock::deserialise(disk)?;
    let mut write_buf=vec![0u8;new_name.len()+10];
    write_buf[0..2].copy_from_slice(&(new_name.len() as u16).to_le_bytes());
    write_buf[2..2+new_name.len()].copy_from_slice(new_name.as_bytes());
    write_buf[2+new_name.len()..].copy_from_slice(&node.to_le_bytes());
    let mut inode=find_inode(disk,directory_inode)?;
    let extents=range_lookup(disk,&inode.i_extents,0,u64::MAX)?;
    let next_logical=extents.iter().map(|e| e.logical_start+e.length).max().unwrap_or(0);
    let mut write:Option<(Block,usize)>=None;
    'outer : for extent in extents{
        for b in extent.physical_start..extent.physical_start+extent.length{
            let block=Block::deserialise(disk,b)?;
            let mut offset=BLOCK_HEADER_SIZE;
            while offset+2<=BLOCK_SIZE{
                let len=u16::from_le_bytes([block.buf[offset],block.buf[offset+1]]) as usize;
                if len==0{
                    break;
                }
                offset+=2+len+8;
            }
            if offset>BLOCK_SIZE || BLOCK_SIZE-offset<write_buf.len(){
                continue;
            }
            write=Some((block,offset));
            break 'outer;
        }
    }
    if let Some((mut block,offset))=write{
        block.buf[offset..offset+write_buf.len()].copy_from_slice(&write_buf);
        inode.i_size+=write_buf.len() as u64;
        block.write_block(disk)?;
    }else{
        let new_block_id=allocate_block(disk,&mut superblock)?;
        let new_extent=crate::extent_tree::Extent{
            logical_start:next_logical,
            physical_start:new_block_id,
            length:1,
        };
        let mut tree_blocks=0u64;
        let mut alloc_block=|| -> Result<u64,FileError> {
            let b=allocate_block(disk,&mut superblock)?;
            tree_blocks+=1;
            Ok(b)
        };
        insert_extent(disk,&mut inode.i_extents,new_extent,&mut alloc_block)?;
        inode.i_blocks+=1+tree_blocks;
        let mut new_block=Block{
            id:new_block_id,
            header:crate::block::BlockHeader{lsn:0,checksum:0,flag:crate::block::Flag::Clean},
            buf:vec![0u8;BLOCK_SIZE],
        };
        new_block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE+write_buf.len()].copy_from_slice(&write_buf);
        inode.i_size+=write_buf.len() as u64;
        new_block.write_block(disk)?;
    }
    write_inode(disk,directory_inode,&inode.serialise())?;
    disk.write_all_at(&superblock.serialise(),0).map_err(|_| FileError::WriteError)?;
    Ok(node)
}

pub fn write_dirents(disk:&File, directory_inode:u64, dirents:Vec<(String,u64)>) -> Result<(), FileError> {
    let mut node = find_inode(disk, directory_inode)?;
    let mut superblock = SuperBlock::deserialise(disk)?;
    let freed = {
        let mut free_block = |block_id: u64| -> Result<(), FileError> {
            mark_block_free(disk, block_id)?;
            superblock.free_blocks += 1;
            Ok(())
        };
        delete_extent_range(disk, &mut node.i_extents, 0, u64::MAX, &mut free_block)?
    };
    free_data_extents(disk, &mut superblock, &freed)?;

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
    let mut tree_blocks=0u64;
    for (logical, chunk) in chunks.iter().enumerate() {
        let block_id = allocate_block(disk, &mut superblock)?;
        let new_extent = crate::extent_tree::Extent {
            logical_start: logical as u64,
            physical_start: block_id,
            length: 1,
        };
        let mut alloc_block = || -> Result<u64, FileError> {
            let b = allocate_block(disk, &mut superblock)?;
            tree_blocks += 1;
            Ok(b)
        };
        insert_extent(disk, &mut node.i_extents, new_extent, &mut alloc_block)?;
        let mut block = Block {
            id: block_id,
            header: crate::block::BlockHeader { lsn: 0, checksum: 0, flag: crate::block::Flag::Clean },
            buf: vec![0u8; BLOCK_SIZE],
        };
        block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + chunk.len()].copy_from_slice(chunk);
        block.write_block(disk)?;
    }
    node.i_blocks=chunks.len() as u64 + tree_blocks;
    write_inode(disk, directory_inode, &node.serialise())?;
    disk.write_all_at(&superblock.serialise(), 0).map_err(|_| FileError::WriteError)?;
    Ok(())
}

pub fn lexer(path:&String)->Vec<String>{
    let mut curr_name=String::new();
    let mut tokens:Vec<String>=Vec::new();
    let mut is_escape=false;
    for c in path.chars(){
        if is_escape{
            curr_name.push(c);
            is_escape=false;
            continue;
        }
        match c{
            '\\'=>is_escape=true,
            '/'=>{
                if !curr_name.is_empty(){
                    tokens.push(std::mem::take(&mut curr_name));
                }
            }
            _=>curr_name.push(c),
        }
    }
    if !curr_name.is_empty(){
        tokens.push(curr_name);
    }
    tokens
}

pub fn resolve_path(disk:&File,path:String,current:u64)->Result<u64,FileError>{
    if path.len()==0{
        return Ok(ROOT_INODE_NUM as u64);
    }
    let absolute:bool=path.starts_with('/');
    let tokens=lexer(&path);
    let mut id:u64;
    if absolute{
        id=ROOT_INODE_NUM as u64;
    }else{
        id=current;
    }
    'outer : for token in &tokens{
        let dirents=read_dir(disk,id)?;
        for (dirent,inode) in dirents{
            if dirent==*token{
                id=inode;
                continue 'outer;
            }
        }
        return Err(FileError::NameNotFound);
    }
    Ok(id)
}

pub fn make_dir(
    disk:&File, 
    parent_inode:u64, 
    name:String, 
    uid:u32, 
    gid:u32,
    permissions:u16) 
-> Result<u64, FileError> {
    let new_id = add_dirent(disk, parent_inode, name, S_IFDIR | (permissions & 0o7777), uid, gid)?;
    let self_and_parent = vec![
        (".".to_string(), new_id),
        ("..".to_string(), parent_inode),
    ];
    write_dirents(disk, new_id, self_and_parent)?;
    let mut new_node = find_inode(disk, new_id)?;
    new_node.i_links_count = 2;
    write_inode(disk, new_id, &new_node.serialise())?;
    let mut parent_node = find_inode(disk, parent_inode)?;
    parent_node.i_links_count += 1;
    write_inode(disk, parent_inode, &parent_node.serialise())?;
    Ok(new_id)
}
