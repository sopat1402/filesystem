use std::fs::File;
use std::os::unix::prelude::FileExt;
use crate::file_errors::FileError;

pub const BLOCK_SIZE:usize=4096;
pub const NUM_BLOCKS:usize=12800;
const MAGIC:u32=69420;

#[repr(u16)]
pub enum Flag{
    Clean,
    Dirty,
    Recoverable,
    Irrecoverable,
}

pub struct BlockHeader{
    pub lsn         :   u64,
    pub checksum    :   u32,
    pub flag       :   Flag,
}

//make this check the checksum on its own

impl BlockHeader{
    pub fn deserialise(block:&[u8])->Result<Self,FileError>{
        let lsn=u64::from_le_bytes(block[0..8].try_into().map_err(|_| FileError::CorruptedBlock)?);
        let checksum=u32::from_le_bytes(block[8..12].try_into().map_err(|_| FileError::CorruptedBlock)?);
        let flag=match u16::from_le_bytes(block[12..14].try_into().map_err(|_| FileError::CorruptedBlock)?){
            0=>Flag::Clean,
            1=>Flag::Dirty,
            2=>Flag::Recoverable,
            3=>Flag::Irrecoverable,
            _=>return Err(FileError::CorruptedBlock),
        };
        Ok(Self{lsn,
            checksum,
            flag,
            })
    }

    pub fn serialise(&self)->Vec<u8>{
        let mut buf=vec![0u8;14];
        let lsn_bytes=self.lsn.to_le_bytes();
        let checksum_bytes=self.checksum.to_le_bytes();
        let flag:u16=match self.flag{
            Flag::Clean=>0,
            Flag::Dirty=>1,
            Flag::Recoverable=>2,
            Flag::Irrecoverable=>3,
        };
        let flag_bytes=flag.to_le_bytes();
        buf[0..8].copy_from_slice(&lsn_bytes);
        buf[8..12].copy_from_slice(&checksum_bytes);
        buf[12..14].copy_from_slice(&flag_bytes);
        buf
    }
}

pub struct SuperBlock{
    pub header                  :   BlockHeader,
    pub magic                   :   u32,
    pub version                 :   u32,
    pub total_size              :   u32,
    pub block_size              :   u16,
    pub inode_size              :   u16,
    pub block_count             :   u32,
    pub inode_count             :   u32,
    pub free_blocks             :   u32, //unallocated
    pub free_inodes             :   u32, //unallocated
    pub inode_bitmap_start      :   u16,
    pub block_bitmap_start      :   u16,
    pub inode_map_start         :   u16,
    pub data_start              :   u16,
    pub state                   :   u8, // 0-dirty, 1-clean
    pub root_inode              :   u32,
}

impl SuperBlock{
    pub fn deserialise(disk:&File)->Result<Self,FileError>{
        let mut block=vec![0u8;BLOCK_SIZE];
        disk.read_at(&mut block,0).map_err(|_| FileError::ReadError)?;
        let header=BlockHeader::deserialise(&block)?;
        let mut offset=14;
        let magic=u32::from_le_bytes(block[offset..offset+4].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=4;
        if magic!=MAGIC{
            return Err(FileError::CorruptedBlock);
        }
        let version=u32::from_le_bytes(block[offset..offset+4].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=4;
        let total_size=u32::from_le_bytes(block[offset..offset+4].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=4;
        let block_size=u16::from_le_bytes(block[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=2;
        let inode_size=u16::from_le_bytes(block[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=2;
        let block_count=u32::from_le_bytes(block[offset..offset+4].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=4;
        let inode_count=u32::from_le_bytes(block[offset..offset+4].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=4;
        let free_blocks=u32::from_le_bytes(block[offset..offset+4].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=4;
        let free_inodes=u32::from_le_bytes(block[offset..offset+4].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=4;
        let inode_bitmap_start=u16::from_le_bytes(block[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=2;
        let block_bitmap_start=u16::from_le_bytes(block[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=2;
        let inode_map_start=u16::from_le_bytes(block[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=2;
        let data_start=u16::from_le_bytes(block[offset..offset+2].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=2;
        let state=u8::from_le_bytes(block[offset..offset+1].try_into().map_err(|_| FileError::CorruptedBlock)?);
        offset+=1;
        let root_inode=u32::from_le_bytes(block[offset..offset+4].try_into().map_err(|_| FileError::CorruptedBlock)?);
        let superblock=Self{
            header,
            magic,
            version,
            total_size,
            block_size,
            inode_size,
            block_count,
            inode_count,
            free_blocks,
            free_inodes,
            inode_bitmap_start,
            block_bitmap_start,
            inode_map_start,
            data_start,
            state,
            root_inode,
        };
        Ok(superblock)
    }

    pub fn serialise(&self) -> Vec<u8> {
        let mut block = vec![0u8; BLOCK_SIZE];
        let mut offset: usize = 0;
        let header = self.header.serialise();
        block[offset..offset+14].copy_from_slice(&header);
        offset += 14;
        block[offset..offset+4].copy_from_slice(&self.magic.to_le_bytes());
        offset += 4;
        block[offset..offset+4].copy_from_slice(&self.version.to_le_bytes());
        offset += 4;
        block[offset..offset+4].copy_from_slice(&self.total_size.to_le_bytes());
        offset += 4;
        block[offset..offset+2].copy_from_slice(&self.block_size.to_le_bytes());
        offset += 2;
        block[offset..offset+2].copy_from_slice(&self.inode_size.to_le_bytes());
        offset += 2;
        block[offset..offset+4].copy_from_slice(&self.block_count.to_le_bytes());
        offset += 4;
        block[offset..offset+4].copy_from_slice(&self.inode_count.to_le_bytes());
        offset += 4;
        block[offset..offset+4].copy_from_slice(&self.free_blocks.to_le_bytes());
        offset += 4;
        block[offset..offset+4].copy_from_slice(&self.free_inodes.to_le_bytes());
        offset += 4;
        block[offset..offset+2].copy_from_slice(&self.inode_bitmap_start.to_le_bytes());
        offset += 2;
        block[offset..offset+2].copy_from_slice(&self.block_bitmap_start.to_le_bytes());
        offset += 2;
        block[offset..offset+2].copy_from_slice(&self.inode_map_start.to_le_bytes());
        offset += 2;
        block[offset..offset+2].copy_from_slice(&self.data_start.to_le_bytes());
        offset += 2;
        block[offset..offset+1].copy_from_slice(&self.state.to_le_bytes());
        offset += 1;
        block[offset..offset+4].copy_from_slice(&self.root_inode.to_le_bytes());
        block
    }
}

pub struct Block{
    pub id      :   usize, //0 indexed from superblock
    pub header  :   BlockHeader,
    pub buf     :   Vec<u8>,
}

impl Block{
    pub fn serialise(&mut self){
        let header_bytes=self.header.serialise();
        self.buf[0..14].copy_from_slice(&header_bytes);
    }
    pub fn deserialise(disk:&File,id:usize)->Result<Self,FileError>{
        if id==0{
            return Err(FileError::ReadError); //not for superblock
        }
        let offset:u64=(id*BLOCK_SIZE) as u64;
        let mut buf=vec![0u8;BLOCK_SIZE];
        disk.read_at(&mut buf,offset).map_err(|_| FileError::ReadError)?;
        let header=BlockHeader::deserialise(&buf)?;
        Ok(Self{id,header,buf})
    }
}
