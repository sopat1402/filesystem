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
blocks. I'll have to customise it for a larger disk though. the superblock would need a few tweaks too.

I made a create_disk function. primitive, just to have something to work with. I do need to set the first few bits of
the block map to 1 because I do not want to allocate the bitmaps, superblock or the inode table.

My block search will be greedy. It will add contiguous ranges to a vector of extents. Locality will have to be
considered later,

in my create disk, i'll need to block bitmap to set the first few blocks to 1. 
superblock-> 1 bit, inode map -> 1 bit (for now), block map -> 1 bit (for now).
Each block has a 12 byte header. So 4084 accessible bytes for the inode table on each block. each inode is 256 bytes.
So, number of blocks here = ceil(256*10000/4084)=627. However, 15 inodes can fit per block and fractional writes
are not allowed so it becomes 667.

670 bits. that gives 83 bytes that are 0xFF and one byte that is 0x3F. 84 total. 96 bytes past the block header.

# Extent tree

Block max extries is actually 339 not 340. It's because of the 12 byte header I added to every block. Matching it
with the ext4 structure to know what I'm doing won't fly anymore. It's fine ig I got the hang of it. It's a fucking
tree again but there won't be much depth due to the crazy fanout. I thought I would never have to make a B+ tree again
in my life but here we are. 
Thankfully it's not a pure B+ tree. Actually that's a thing for more despair because I already wrote a full B+ tree but
now, I have to build a special one. But balancing and splitting won't be needed in the same way because it will
be rare to see something need even 2 layers.

I got rid of inode extent root and external extent root. at a local scale it seems nice but fuck it i got rid of 
repr c, packed and just made it vectors that way it's just one data type. refactored all over and it is much cleaner.

I put read external block into the impl for tree node. It reads and stores entries.BTW it's 339 because of the block header. (4096-12-8)/12. 8 is the extent node header data like entry count and magic. Now, this caused me quite a lot of a
headache to finalise because of all the noise out there : my inode will load all 15 entries, even if unused because
it is fixed size. but the external ones which are all extent tree nodes just with a different max entries will load
only the necessary entries.

# Block header refactor

Blocks needs flags of clean, dirty, recoverable corruption, irrecoverable corruption. Need to add a u16, which increases
the size from 12 bytes to 14 bytes. 4096-14=4082 bytes. Each inode is 256 bytes. Still 15 per block. So ig that bitmap
part is the same, thankfully. Also the flag header pushes it to 14..98 for the bits.

# Extent tree algorithms

AAAAAH I fucking hate trees. This shit is just like a B+ tree but then I don't need to worry as much about crazy depth
because of the fanout. On top of the basic insertion, I had to make my extents merge when I could make them but
also have to check for overlap and also when a block is freed in the process of insertion, it needs to be pushed to
a freed vector for my bitmap to then mark as freed.

I forgot to make a mark as free function in my bitmap but after checking the insertion logic here, I'll write it.

This extent tree is easily the WORST part of this project just like the B+ tree was the worst part of my database. I
hate trees. I hate trees. I hate trees. I hate trees.

Also, there's a crash window I have to fix with atomicity in my ARIES where a block when split may end up orphaned if
a crash occurs before success. Ordering tricks won't fix that and it'll only be clear after actually seeing the bug in
action later when the filesystem actually does something other than give its developer a throbbing headache.

K I added it. I'll do a commit here and btw I also added methods to mark inodes as used and free too.

Deletion is done. So is traversal. And range lookup. I have to read my code a few times over to figure out what the
fuck it is that I finally managed to write because tests aren't really possible yet.
