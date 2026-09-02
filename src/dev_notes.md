# Inodes

K so I'm not using the ext2 thing with i_uid_upper and all that crap. just a straight u32 where I can do so. That does
mean however, I'd need my own file driver.

I am going old school with pointers and not extents, at least for now since the main drawback is that there is extra
metadata to read and write but with extents I'd have to make an extent tree and deal with fragmentation as it tries
for contiguous allocation.

I'm using a simple crc32 for my block checksums. Most filesystems don't use this but I want to experiment and see
how crash durable I can make this filesystem with a more advanced WAL than journalling uses. For now though that's the
least of my concerns. I am still yet to write my superblock and make my bitmaps.

There are 10,000 inodes and 128 byte inodes. 4096 byte blocks and 12,800 blocks, to make the test.img disk come to
50 MiB. Once I have the actual logic done, scaling will be easy.

Each block has a 12 byte header of lsn and checksum. For now I'm not calculating checksum even though I did add crc32
because I want something that at least works first.

NVM I switched to extents in my inodes because otherwise a refactor would need a lot of changes especially in allocation.
So, there's inode extent root, which is limited to 15 entries. I changed the inode size to 256 bytes. Now,
there is InodeExtentRoot, which has 15 for the 256 byte limit. It has an array of entries. In this case 15 entries.
But then, each block can have a max of 340 entries or extents, they are both the same size in bytes : 12. Now, if there
are more than 15 extents needed, none of the extents will be in inode root. inode root will instead have entries.
these entries point to blocks. in those blocks, there will either be extents, or there will be entries. The limit
here is 340. So there can be 340 children of this. The total children at this depth is 15*340*340. It scales very
fast and is thus better than using block pointers, where I would have had to write all that metadata. When depth=0,
it is a leaf node and it is extents in the block. Otherwise it is entries.
Depth doesn't even necessarily need to be incremented for all layers. Just change it from 0 if making a child layer.
It is mainly a discriminator.

# Bitmaps

Pretty simple for inode search, check the 8 bytes in each byte using byte & (1<<bit)==0 to see if it is free.
that way, a block for the inodesbitmap has 4096-12=4084 bytes and hence can represent 8*4096 inodes. Same for the
blocks. I'll have to cuustomise it for a larger disk though. the superblock would need a few tweaks too.

I made a create_disk function. primitive, just to have something to work with. I do need to set the first few bits of
the block map to 1 because I do not want to allocate the bitmaps, superblock or the inode table.

My block search will be greedy. It will add contiguous ranges to a vector of extents. Locality will have to be
considered later,

in my create disk, i'll need to block bitmap to set the first few blocks to 1. 
superblock-> 1 bit, inode map -> 1 bit (for now), block map -> 1 bit (for now).
Each block has a 12 byte header. So 4084 accessible bytes for the inode table on each block. each inode is 256 bytes.
So, number of blocks here = ceil(256*10000/4084)=627. However, 15 inodes can fit per block and fractional writes
are not allowed so it becomes 667.

670 bits. that gives 83 bytes that are 0xFF and one byte that is 0xCF. 84 total. 96 bytes past the block header.
