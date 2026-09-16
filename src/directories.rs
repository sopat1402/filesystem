use crate::constants::*;
use crate::file_errors::FileError;
use crate::inode::{find_inode,write_inode};
use crate::extent_tree::{range_lookup,insert_extent};
use crate::block::{Block,SuperBlock};
use crate::filesystem::reserve_inode;
use crate::bitmaps::*;
use std::fs::File;
use std::os::unix::prelude::FileExt;

pub fn is_dir(mode: u16) -> bool {
    mode & 0o170000 == S_IFDIR
}

pub fn read_dir(disk:&File,inode_id:usize)->Result<Vec<(String,usize)>,FileError>{
    let node=find_inode(disk,inode_id)?;
    if !is_dir(node.i_mode){
        return Err(FileError::NotDirectory);
    }
    let extents=range_lookup(disk,&node.i_extents,0,u32::MAX)?;
    let mut dir_buf: Vec<u8> = Vec::with_capacity(node.i_size as usize);
    let mut remaining = node.i_size as usize;
    'outer: for extent in extents {
        for b in extent.physical_start..extent.physical_start + extent.length {
            if remaining == 0 {
                break 'outer;
            }
            let block = Block::deserialise(disk, b as usize)?;
            let payload = &block.buf[14..];
            let take = payload.len().min(remaining);
            dir_buf.extend_from_slice(&payload[..take]);
            remaining -= take;
        }
    }
    let mut dirents:Vec<(String,usize)>=Vec::new();
    let mut offset:usize=0;
    while offset<dir_buf.len(){
        let name_len=u16::from_le_bytes(dir_buf[offset..offset+2].try_into().map_err(|_| FileError::CorruptedINode)?);
        offset+=2;
        let mut buf=vec![0u8;name_len as usize];
        buf.copy_from_slice(&dir_buf[offset..offset+name_len as usize]);
        let name=String::from_utf8(buf).map_err(|_| FileError::CorruptedINode)?;
        offset+=name_len as usize;
        let inode_num=u32::from_le_bytes(dir_buf[offset..offset+4].try_into().map_err(|_| FileError::CorruptedINode)?);
        dirents.push((name,inode_num as usize));
        offset+=4;
    }
    Ok(dirents)
}

fn reserve_block(buf:&mut [u8],disk:&File)->Result<u32,FileError>{
    let new_extent=find_blocks(buf,1);
    if new_extent.len()==0{
        return Err(FileError::NoMoreBlocks);
    }
    mark_blocks_used(buf,&new_extent);
    let offset=BLOCK_SIZE*BLOCK_BITMAP_START+BLOCK_HEADER_SIZE;
    disk.write_all_at(buf,offset as u64).map_err(|_| FileError::WriteError)?;
    Ok(new_extent[0].physical_start)
}

pub fn add_dirent(disk:&File,directory_inode:usize,new_name:String,mode:u16,uid:u16,gid:u16)->Result<(),FileError>{
    let dirents=read_dir(disk,directory_inode)?;
    for (name,_) in &dirents{
        if *name==new_name{
            return Err(FileError::NameExists);
        }
    }
    let node=reserve_inode(disk,mode,uid,gid)?;
    let mut superblock=SuperBlock::deserialise(disk)?;
    let mut write_buf=vec![0u8;new_name.len()+6];
    let len=(new_name.len() as u16).to_le_bytes();
    write_buf[0..2].copy_from_slice(&len);
    write_buf[2..2+new_name.len()].copy_from_slice(new_name.as_bytes());
    let id=(node as u32).to_le_bytes();
    write_buf[2+new_name.len()..].copy_from_slice(&id);
    let mut inode=find_inode(disk,directory_inode)?;
    let extents=range_lookup(disk,&inode.i_extents,0,u32::MAX)?;
    let mut write:Option<(Block,usize)>=None;
    'outer : for extent in extents{
        'inner : for b in extent.physical_start..extent.physical_start+extent.length{
            let block=Block::deserialise(disk,b as usize)?;
            let mut offset=14usize;
            loop{
                let len=u16::from_le_bytes(block.buf[offset..offset+2]
                    .try_into()
                    .map_err(|_| FileError::CorruptedBlock)?
                );
                if len==0{
                    break;
                }
                offset+=2+len as usize+4;
            }
            if 4096-offset<write_buf.len(){
                continue 'inner;
            }else{
                write=Some((block,offset));
                break 'outer;
            }
        }
    }
    if let Some((mut block,offset))=write{
        block.buf[offset..offset+write_buf.len()].copy_from_slice(&write_buf);
        inode.i_size+=write_buf.len() as u64;
        block.write_block(disk)?;
    }
    else{
        let bitmap_block=Block::deserialise(disk,superblock.block_bitmap_start as usize)?;
        let mut buf=vec![0u8;BLOCK_SIZE-BLOCK_HEADER_SIZE];
        buf.copy_from_slice(&bitmap_block.buf[BLOCK_HEADER_SIZE..]);
        let new_extent=find_blocks(&buf,1);
        if new_extent.len()==0{
            return Err(FileError::NoMoreBlocks);
        }
        mark_blocks_used(&mut buf,&new_extent);
        let offset=BLOCK_SIZE*BLOCK_BITMAP_START+BLOCK_HEADER_SIZE;
        disk.write_all_at(&buf,offset as u64).map_err(|_| FileError::WriteError)?;
        let new_extent=new_extent[0];
        let mut alloc_block = || reserve_block(&mut buf,disk);
        let mut new_block=Block::deserialise(disk,new_extent.physical_start as usize)?;
        insert_extent(disk,&mut inode.i_extents,new_extent,&mut alloc_block)?;
        superblock.free_blocks-=1;
        new_block.buf[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE+write_buf.len()].copy_from_slice(&write_buf);
        inode.i_size+=write_buf.len() as u64;
        new_block.write_block(disk)?;
    }
    let inode_buf=inode.serialise();
    write_inode(disk,node,&inode_buf)?;
    let buf=superblock.serialise();
    disk.write_all_at(&buf,0).map_err(|_| FileError::WriteError)?;
    Ok(())
}
