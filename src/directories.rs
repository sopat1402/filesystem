use crate::constants::*;
use crate::file_errors::FileError;
use crate::inode::{find_inode,write_inode};
use crate::extent_tree::{range_lookup,insert_extent,delete_extent_range};
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
            let payload = &block.buf[BLOCK_HEADER_SIZE..];
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
            let mut offset=BLOCK_HEADER_SIZE;
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
    write_inode(disk, directory_inode, &inode_buf)?;
    let buf=superblock.serialise();
    disk.write_all_at(&buf,0).map_err(|_| FileError::WriteError)?;
    Ok(())
}

pub fn print_dir(disk:&File,inode_id:usize)->Result<(),FileError>{
    let dirents=read_dir(disk,inode_id)?;
    for (dirent,_) in dirents{
        print!("{}\t",dirent);
    }
    println!();
    Ok(())
}

pub fn delete_dirent(disk:&File,inode_id:usize)->Result<(),FileError>{
    let mut node=find_inode(disk,inode_id)?;
    let mut superblock=SuperBlock::deserialise(disk)?;
    let mut inode_bitmap=Block::deserialise(disk,superblock.inode_bitmap_start as usize)?;
    if is_dir(node.i_mode){
        let dirents=read_dir(disk,inode_id)?;
        for (_,id) in dirents{
            delete_dirent(disk,id)?;
            superblock = SuperBlock::deserialise(disk)?;
        }
        let extents=range_lookup(disk,&node.i_extents,0,u32::MAX)?;
        for extent in extents{
            'inner : for block_id in extent.physical_start..extent.physical_start+extent.length{
                let mut block=Block::deserialise(disk,block_id as usize)?;
                let buf=[0u8;BLOCK_SIZE-BLOCK_HEADER_SIZE];
                block.buf[BLOCK_HEADER_SIZE..].copy_from_slice(&buf);
                block.write_block(disk)?;
                let mut block_bitmap=Block::deserialise(disk,superblock.block_bitmap_start as usize)?;
                mark_block_free(&mut block_bitmap.buf[BLOCK_HEADER_SIZE..],block_id as usize);
                block_bitmap.write_block(disk)?;
                superblock.free_blocks+=1;
                continue 'inner;
            }
        }
        let freed = {
            let mut free_block = |block_id: u32| -> Result<(), FileError> {
                let mut block_bitmap =
                    Block::deserialise(disk, superblock.block_bitmap_start as usize)?;
                mark_block_free(
                    &mut block_bitmap.buf[BLOCK_HEADER_SIZE..],
                    block_id as usize
                );
                block_bitmap.write_block(disk)?;
                superblock.free_blocks += 1;
                Ok(())
            };
            delete_extent_range(
                disk,
                &mut node.i_extents,
                0,
                u32::MAX,
                &mut free_block
            )?
        };
        for extent in freed {
            for block_id in extent.physical_start
                ..extent.physical_start + extent.length
            {
                let mut block = Block::deserialise(disk, block_id as usize)?;
                let buf = [0u8; BLOCK_SIZE - BLOCK_HEADER_SIZE];
                block.buf[BLOCK_HEADER_SIZE..].copy_from_slice(&buf);
                block.write_block(disk)?;
                let mut block_bitmap =
                    Block::deserialise(disk, superblock.block_bitmap_start as usize)?;
                mark_block_free(
                    &mut block_bitmap.buf[BLOCK_HEADER_SIZE..],
                    block_id as usize
                );
                block_bitmap.write_block(disk)?;
                superblock.free_blocks += 1;
            }
        }
        mark_inode_free(&mut inode_bitmap.buf[BLOCK_HEADER_SIZE..],inode_id);
        inode_bitmap.write_block(disk)?;
        superblock.free_inodes+=1;
        let buf=superblock.serialise();
        disk.write_all_at(&buf,0).map_err(|_| FileError::WriteError)?;
    }else{
        let extents=range_lookup(disk,&node.i_extents,0,u32::MAX)?;
        for extent in extents{
            'inner : for block_id in extent.physical_start..extent.physical_start+extent.length{
                let mut block=Block::deserialise(disk,block_id as usize)?;
                let buf=[0u8;BLOCK_SIZE-BLOCK_HEADER_SIZE];
                block.buf[BLOCK_HEADER_SIZE..].copy_from_slice(&buf);
                block.write_block(disk)?;
                let mut block_bitmap=Block::deserialise(disk,superblock.block_bitmap_start as usize)?;
                mark_block_free(&mut block_bitmap.buf[BLOCK_HEADER_SIZE..],block_id as usize);
                block_bitmap.write_block(disk)?;
                superblock.free_blocks+=1;
                continue 'inner;
            }
        }
        let freed = {
            let mut free_block = |block_id: u32| -> Result<(), FileError> {
                let mut block_bitmap =
                    Block::deserialise(disk, superblock.block_bitmap_start as usize)?;
                mark_block_free(
                    &mut block_bitmap.buf[BLOCK_HEADER_SIZE..],
                    block_id as usize
                );
                block_bitmap.write_block(disk)?;
                superblock.free_blocks += 1;
                Ok(())
            };
            delete_extent_range(
                disk,
                &mut node.i_extents,
                0,
                u32::MAX,
                &mut free_block
            )?
        };
        for extent in freed {
            for block_id in extent.physical_start
                ..extent.physical_start + extent.length
            {
                let mut block = Block::deserialise(disk, block_id as usize)?;
                let buf = [0u8; BLOCK_SIZE - BLOCK_HEADER_SIZE];
                block.buf[BLOCK_HEADER_SIZE..].copy_from_slice(&buf);
                block.write_block(disk)?;
                let mut block_bitmap =
                    Block::deserialise(disk, superblock.block_bitmap_start as usize)?;
                mark_block_free(
                    &mut block_bitmap.buf[BLOCK_HEADER_SIZE..],
                    block_id as usize
                );
                block_bitmap.write_block(disk)?;
                superblock.free_blocks += 1;
            }
        }
        mark_inode_free(&mut inode_bitmap.buf[BLOCK_HEADER_SIZE..],inode_id);
        inode_bitmap.write_block(disk)?;
        superblock.free_inodes+=1;
        let buf=superblock.serialise();
        disk.write_all_at(&buf,0).map_err(|_| FileError::WriteError)?;
    }
    Ok(())
}

pub fn delete(disk:&File,parent_inode:usize,name:String)->Result<(),FileError>{
    let res=read_dir(disk,parent_inode)?;
    for (entry_name,inode) in res{
        if entry_name==name{
            return delete_dirent(disk,inode);
        }
    }
    Err(FileError::NotDirectory)
}
