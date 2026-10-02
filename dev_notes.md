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


# Inode reservation

Before being able to reserve an inode, I need to be able to find the inode buffer and deserialise it. If each block
has a 14 byte header. I changed the root inode to 0. Inode is now 0 indexed too. Just works with buffers. Blow me if
you don't like it.

K so I made a method to read and write inodes and write blocks. Then I edited the create disk to put root num as 0.
Then, I put all those messed up constants in a constants.rs. Such spaghetti. It became significantly cleaner.

Then, I made the reserve inode function the takes uid, mode and gid and returns inode number and also sets up the new
inode with the uid, gid, mode and time and also zeroes out other data and then returns inode id.

So now it is ready for a write methods thing. first directories to add files. Then file IO.

# Extent tree function writeup

I made it look pretty.

Core types

Extent { logical_start, physical_start, length } — a contiguous run of length physical blocks starting at physical_start, representing logical blocks [logical_start, logical_start+length) of a file/directory. repr(C, packed), 12 bytes on disk via to_bytes/from_bytes.

IndexEntry { logical_start, child_block, _reserved } — same 12-byte footprint as Extent, used at non-leaf tree levels. logical_start is the minimum logical block covered by the subtree rooted at child_block.

ExtentTreeNode { magic, depth, entry_count, max_entries, entries: Vec<[u8; 12]> } — a node in the tree. entries is untyped raw bytes; callers reinterpret each slot as Extent or IndexEntry depending on depth. depth == 0 means leaf (entries are extents); depth > 0 means internal (entries are index entries). This dual interpretation is the load-bearing trick that keeps one node type serving both roles.

Reading

ExtentTreeNode::read_node_with_block(disk, block_num) -> (Block, Self) — reads one external node off disk, validates magic == TREE_MAGIC and entry_count <= max_entries <= BLOCK_MAX_ENTRIES, returns both the raw Block (for later rewriting in place) and the parsed node. Used whenever a caller needs to mutate-then-rewrite the same block (insert/delete paths).

ExtentTreeNode::read_node(disk, block_num) -> Self — same, discarding the Block. Used for pure reads (traversal, lookup) that never write back.

range_lookup(disk, node, start, end) -> Vec<Extent> — public entry point for querying a logical range [start, end). Delegates to collect_range.

collect_range(disk, node, start, end, out) — recursive traversal:

Leaf: linear scan of the node's own entries, keeping any extent whose logical range overlaps [start, end).
Internal: finds the first child that could contain start (find_child), then walks forward through sibling index entries, recursing into each child whose key is < end, breaking early once a child's key reaches end (pure pruning — doesn't affect correctness, just avoids visiting children guaranteed not to overlap).
Cost is bounded by nodes actually touched, not by the numeric width of [start, end) — a query with end = u32::MAX is a full traversal, not an unbounded scan, since the loop bounds are entries.len(), not end - start.

lookup_extent(disk, node, logical) -> Option<Extent> — point lookup for one logical block: leaf does a linear find, internal descends into exactly one child via find_child. O(depth), not O(fanout) per level like collect_range.

find_child(node, logical_start) -> usize — shared descent primitive: finds the rightmost index entry whose logical_start <= logical_start, i.e. the child subtree that should contain that logical address. Falls back to index 0 if nothing qualifies (defensive; shouldn't happen in a well-formed tree since the first child should always cover the minimum).

Writing

write_external_block(disk, block, ext_block) — serialises a node into the payload region of an already-loaded Block (after the 14-byte block header), stamps the block header (checksum + flag) via block.serialise(), and writes the whole block to disk in one call. This is the single write path every mutation (insert splits, delete merges/borrows) funnels through — no other function writes external blocks directly.

insert_extent(disk, root, new_ext, alloc_block) -> Vec<Extent> — public insert entry point. Delegates to insert_into_node with node_block = None (signals "this is the inode-resident root, not an external block," which changes the effective max_entries used for split decisions — root uses ROOT_MAX_ENTRIES, external nodes use BLOCK_MAX_ENTRIES). On a root split, allocates a fresh block for the old root's contents (left_block), writes it, and rewrites root in place as a new depth+1 internal node with two children (old root's data, and the new split-off sibling). Returns any Extents displaced/overwritten by the insert (for the caller to hand to the block-bitmap "mark free" step) — insertion can shrink or split existing overlapping extents, and any physical range that's fully superseded gets reported here rather than silently leaked.

insert_into_node (private) — recursive core. Leaf case delegates to insert_leaf. Internal case: descends to the correct child, recurses, then handles the child's result — either just updating the parent's key for that child (Done), or absorbing a new sibling index entry and possibly splitting itself if that pushes entry_count past its own max_entries (Split). Split point is a simple midpoint (indices.len() / 2) — not weighted by size or logical spread, so it's not necessarily an even split of address-space coverage.

insert_leaf (private) — the actual extent-merging logic:

Clips any existing extent(s) that overlap the new insert's logical range, pushing the overlapping physical portion into freed (so old block mappings that get overwritten don't leak — the caller's alloc_block/bitmap logic is expected to reclaim them).
Attempts to merge the new extent with its immediate logical+physical neighbors (merges_with_prev/merges_with_next) — this only fires when both logical and physical contiguity hold, so it correctly avoids merging two extents that happen to be logically adjacent but physically scattered.
If the resulting entry count still fits max_entries, done. Otherwise splits at the midpoint, writes the right half to a freshly allocated block, and returns Split up to the caller.
Deleting

delete_extent_range(disk, root, start, end, free_block) -> Vec<Extent> — public delete entry point, mirrors insert_extent's structure. After delete_from_node signals Underflow at the root, checks whether the root has collapsed to a single child — if so, and that child's contents fit within ROOT_MAX_ENTRIES, pulls the child's data up into the root in place and frees the now-empty child block. This is the tree-shrinking counterpart to insert's tree-growing split.

delete_from_node (private) — recursive core, structurally parallel to insert_into_node. Leaf case delegates to delete_leaf. Internal case: descends, and on Underflow from the child, tries three rebalancing strategies in order — borrow from left sibling, borrow from right sibling, merge with a sibling (left preferred, right as fallback) — each checked against min_entries (half of BLOCK_MAX_ENTRIES, fixed regardless of whether the sibling is itself a root — a minor asymmetry worth noting, since the root's own min-entries threshold uses whatever max_entries was passed in, but sibling checks always use the block-level threshold).

delete_leaf (private) — trims/splits existing extents against the delete range [start, end), pushing every excised physical sub-range into freed. Signals Underflow if what remains drops below min_entries(node.max_entries).

min_entries(max_entries) -> u16 — flat max_entries / 2 floor threshold; same policy used for both root and external nodes, just parameterized by whichever max_entries is in scope.

Architectural notes

One node type, two interpretations. ExtentTreeNode.entries: Vec<[u8; 12]> is reinterpreted as Extent or IndexEntry based on depth, avoiding a second node type entirely — this is what let me drop repr(C, packed)-based fixed structs earlier and collapse inode-root vs. external-block nodes into one type.

Two different max-entries regimes coexist in the same tree. Root (inode-resident) caps at ROOT_MAX_ENTRIES (15, budget-limited by INODE_SIZE); every external block caps at BLOCK_MAX_ENTRIES (339, budget-limited by BLOCK_SIZE minus block+node headers). Every function that needs a threshold branches on node_block.is_some() (or equivalent) to pick the right one — this is the main place a future refactor could silently break if a call site forgets the distinction.

Freed-space bookkeeping is push-based, not derived. Neither insert nor delete computes "what became free" after the fact — both accumulate it incrementally into a freed: &mut Vec<Extent> as clipping happens, which the bitmap layer consumes separately. This keeps the tree logic decoupled from bitmap logic entirely (the tree never touches a bitmap directly), at the cost of every caller being responsible for actually applying freed — nothing enforces that today.

No rebalancing on insert beyond simple midpoint splits — unlike some B+-tree implementations, there's no attempt to redistribute into a sibling before splitting, so a tree can end up with node occupancies as low as max_entries/2 + 1 immediately after a split. Given the fanout (339 at external nodes), this is unlikely to matter in practice.

# Directories

Directories are read in one go from a range lookup from 0 to u32 max. they're first read as an inode provided in a 
usize to the function read_directory. Now, the i_mode of the inode is what is to be used as the flag to see both type
and the directory or file status.

In add_dirent, I'm reading dirents and then iterating over the whole dirents to check if such a name exists. If it
does it returns an error. Anyways, after that it reserves an inode and adds it to dirents and then calls a function to
write dirents. but that solves that. Maybe instead of write_dirents I should have write_dirent because otherwise, it'll
write the whole thing all over again when I really just need to add an extent.
Yeah so I got rid of write_dirent altogether. It's specific to add dirent so meh. Also inode number is expected to
be less that u32::MAX. So I will be getting all extents of the directory inode and for each block in that I'll be
checking if it has space for my new entry. if it does, write and break. No partial writes. Too much book keeping, not
much of a payoff.
if NONE of the blocks have space, I'll make a new extent and insert it. While this seems like it should be a whole
function, directory is only appending like this once so nah. Maybe later I can make it a helper if needed.
see it isn't possible for something to be all zeroes for an entry. so I just need to find a contiguous range of zeroes in a block. lol that's clever. solves my hole management too. see my entry format is name_size,name,inode. so it can't be 0 realistically. what's the max name size is 65k but obviously fucking not because the block is 4082 bytes with the header out. so I'll make the max length 255 bytes but here's the thing, I don't even need to check all 255. I just need to see if the first 2 bytes are 0 or not because name length can't be 0.

this means with deletion I'd have to compact. That's important af. So essentially, name length can't be 0. lmfao that's
smart af.

Ok so adding a dirent itself it a mammoth task requiring me to later bring atomicity all over but also I'm getting
fucked because there's so many layers to cross reference already. It has only been 100 ish lines of code but it has
taken hours and is sooo complex. I have to mark blocks and used omfgggg.

Lol I forgot to make my disk have all the other block headers in there. Anyways, I made delete as a function but in 
delete_dirent I'm going recursively if it is a dir but also, delete the function checks with name but it needs to clear
the parent's stuff too. Which means finding where in its extents the name exists. but inserting a dirent works. I also
made all the modes constants.

Holy shit the past week as been busy. Past 2 weeks. First midsems and then an invasive visit. Anyways, my reserve block
was buggy so I made an allocate block and also add dirent was forgetting to flush a block before deserialising it!! So
now delete should be working, I'll test it. It's tiring because I have to rebuild the disk on each corrupted because I
do not have corruption recovery yet. Allocate block takes a superblock reference so doesn't change its free block. The
caller does. It is basically to make a helper for the bitmap mess. This is coming together slowly. TBH the mess will
be implementing something for all or even most of the POSIX APIs when I make my driver.
K dirent deletion passed the test. Yay.

I made a lexer for splitting a path into tokens. then to resolve a path there is a function that returns the inode
number of the path or it gives a name not found error. Now, I am also making a make_dir function. That can't just be
add dirent because I need . and .. as links.

That completes the namespace layer.

# Files

So, most editors read the whole file as a buffer, which is what I will implement first. Then, deletion and changes
occur in memory. After that they atomically delete the old file and write one with the same name. Makes it easy for me
and that is what actual systems do. The atomically part is a keyword ig. The temp file thing is none of my business
over here. I just need to make rename, append, write and read. At that point I only need journalling, caching and
multithreading. Journalling will be the real big thing with atomicity for all of it plus I'm doing semi ARIES like in
my database.

K so I made a file struct. It has an impl with open and read for now. Also, since this is a 50MiB disk, and also because
my extent tree uses u32 for the logical start, the file size for now is limited to 4.29 gigabytes. When I scale up, I
can look at extent_tree.rs. Until then, cry me a river, you can't have 4+ GB files on a 50MiB disk.

Ugh read is where this stuff stops being cute and wipes its makeup off to reveal a fat programmer. Binary search first
to find the lower extent. Then have to find the higher extent based on length. Then, I have to read the extents, put
them into a buffer and based on that I have to trim it as needed and then return it.
fuck me. the fucking range lookup. why the fuck was I searching the whole thing and filtering it myself? Anyways, I used
range lookup and massively cleaned up my code. Before I was doing 0 to u32::MAX without thinking about it and was then
binary searching on it lmfao.

K so for write I did a massive sprint. But first I also made fseek and ftell. Straightforward. I then for write first
wrote the append and file length==0 (inode.i_size) branches as that is writing at the end with a bit for tail writing.
Then came overwrites. It was more interaction with my extent tree than I would have liked. After that I made a 
Filesystem struct and it opens the file from a path and then returns itself and the File struct now uses it inside it
during the process of opening of a file as a param in the function and in the rest it is stored in the struct using
'a as a lifetime specifier.
I did feel however the spark in the project diminished a bit so to bring it back I'll write a FUSE driver so that I can
actually see this project doing something even though there's no block cache or journalling yet. Otherwise, I have built
crazy amounts of infrastructure already but seen about fuck all for it up until at least the write function was done for
a file. The directories part was motivating though.

# FUSE

fuser CAN GO FUCK ITSELF!!! I will not use an external dependency. Even if it means talking to hardware. I will either
use libc or declare war on abstraction itself and write the syscall stubs from scratch.

So I'll write using libc for now to talk to FUSE that's already in the kernel space but at a later point I'll write
something in C to talk directly to the VFS and kernel space. Different project? Not really. But that'll come later.
The fuse driver will let me keep it user space and test for easily. I'll also eventually refactor to variable sized
disks because 50 MiB is fucking absurd.

# Variable size disk refactor

K so I first grouped the constants in constants.rs into those affected by a variable sized disk and those that are
constant. This refactor will be in 2 steps : first I need to make the constants change and I'd have to have a function
to extract the bitmap buffers to feed to block searches. Data start would also change. So that means instead of using
constants, the superblock would have to be referred to. This is fine tbh because later with reader writer mutexes
the reader thing for checking which blocks are concerned with the bitmaps and data and inode maps there won't really be
a write operation unless there's an allocation. But inode search and stuff still needs to check and to know which blocks
to check,it needs to work with the superblock's data. It's tempting to cache the superblock now itself but there will
be a block cache later anyways.

ugh my superblock disk format is changing. have to change total size to a u64 stored in there and block count
from u16 to u64 too. That sets a ceiling of several EiB.

Ok so I changed a whole bunch of fields to u32 and u64 where applicable. I updated the superblock's variable sizes and
on disk representations but basically I have to one shot this or preserve my train of thought perfectly in notes like
this. I also deleted the obsolete constants so that I get errors where they were used and the compiler itself tells me
where the changes are needed.

now, u64::MAX is 18,446,744,073,709,551,615. That's the max number of bytes the disk can have. That's 16 EiB or about
18.4467 exabytes. I'll use EiB since 1024 is nice. So, the max number of blocks is 4,503,599,627,370,496 or about 4.5
quadrillion. Well under u64::MAX. The inode ratio is provided on create disk but basically it too is less than
u64::MAX, which is 2^64 - 1. I have to dynamically calculate the block and inode bitmap sizes.

For x entries to either one, there's a 14 byte header in each block so 4082 bytes per block. Each byte can represent
8 entries. so, x/8 + (x%8>0)*1 bytes needed. with the max blocks, that comes out to 137910326659 blocks!! So that
is more than u32::MAX and it seems even the inode and block bitmap starts need to be u64 then. As well as the inode
table map. K that is done. data start is u64 too.

also, each inode is 256 bytes. With x inodes, that's 256*x bytes. So with 4082 bytes in each block, the number of
blocks needed for the inode table is (256*x)/4082 + ((256*x)%4082)>0*1 blocks. By >0 , it is meant that 1 is true and
0 is false.

I have massive amount of rewriting to do. Create disk needs to now chunk the bitmap stuff block by block. Mark block
and inode free/used in bitmaps.rs needs to now perhaps take the superblock and chunk it on its own. instead of that
maybe just pass disk to it and let it deserialise the superblock on its own. it only need the total blocks/inodes and
the bitmap starts anyways. that'd massively clean code up too. but disk creation first. then the obsolete algorithms
to act on it.

I spent a long time changing signatures and rewriting the bitmap functions. A few panics where i forgot a u32 in hiding
on the test but the disk is successfully created. the inode ratio is bytes per inode. so total inodes is disk size/ratio
anyways, ironing out the kinks.

Most of the bugs at this point are things that can only be fixed with a WAL i.e atomicity issues. Also fuck me IDK how
but I kept spotting correctness bugs like in match arms in the extent tree. headache fr. the code blew up from about 600
lines to almost 1300.

So there's about 10 bugs from a series of tests. At first they gave me the same issue : no more blocks because the
disk creation was using an obsolete and wrong ratio. it should have been using bytes per inode. I blame that on my
browser. When I searched kernel.org that crap gave an AI summary. I went and disabled AI now. Absolute trash that pulls
stuff out of its ass. Has no fucking clue what it is talking about but it has to meet a token requirement ig?

# Bug Fixes

3 of them are too tedious to do tonight so I'll do them tomorrow. Out of 10 I'll be doing 7 tonight itself. 2 left
as of now. I realised that most of what I forgot was those tiny enforcing things, except in the extent tree things,
where idfk what happened.
1 of the 7 left now. That's the block write when 2 free blocks. The bug before this was for adding links. Add dirent
wasn't adding links and reserve inode sets it to 0. Make dir though sets it to 2.
Bug number one (the one left) was also tree related. It was asking for 2 blocks for some reason. I'll have to trace
it when I'm rested. For today though, I have done a crazy amount and managed a full variable sized disk refactor
with the only nugs being 3 tree related ones, which are natural given the size of that code there and 1 for 
unimplemented credentials. The credentials thing will come after the whole tree stuff because that's an auth layer
using uid, gid and groups.

New tests

Extent tree:
an insert spanning three leaves
a delete spanning two leaves
delete-all
merge only when physically contiguous
overwriting the middle of an extent
a depth-2 tree with 1500 extents
range delete on that depth-2 tree
Permissions:
owner, group and other class selection, including supplementary groups
the root bypass
a denied O_TRUNC leaving the file intact, which checks the ordering fix
create needing write permission on the parent
search permission on every path component
File data:
two files with interleaved writes, forcing a depth-1 tree and checking i_blocks
a failed write on a full disk changing nothing
no stale data after truncate and a sparse write
a flipped byte reported as CorruptedBlock
Bookkeeping:
assert_accounting, a helper that compares the superblock free counts with the real bitmaps, used across the file
create/write/delete cycles returning to baseline
draining a 220-entry directory
inode reuse with clean state
inode exhaustion
. and .. resolution

K so currently only the directory_i_blocks_tracks_growth_through_add_dirent test is failing. Others pass.
When add_dirent allocates a new directory block, it never increments i_blocks. Only write_dirents sets it. 
The fix is to bump inode.i_blocks in the else branch, plus the tree blocks if insert_extent allocates any. But at this
point, a massive chunk of crap is done. I can finally start with the driver once I fix that bug. I added credentials
too with a kernel style check. So, I'm dreading the multithreading part lmao.

# Driver

Gaps in the current API
- Inode numbers: FUSE's root is nodeid 1 and ino 0 is invalid, but my root is 0. I'll add 1 on the way out and 
    subtract 1 on the way in, at the driver boundary only.
- File is path-based and holds &Filesystem. FUSE is inode-based with explicit offsets. I'll pull the bodies of read
    and write out into read_at(inode, offset, len) and write_at(inode, offset, buf, append). Keep a driver-owned fh 
    -> (inode, flags) table instead of storing File<'a> values, which would be a self-referential mess.
- Attributes: I need a function that returns an inode's attributes. FUSE blocks counts 512-byte units, 
    so multiply the i_blocks by 8. Take uid and gid from the request header on CREATE and MKDIR.
- rmdir and unlink: My delete is recursive. If I map rmdir straight onto it, rmdir on a non-empty directory 
    destroys the contents. Check ENOTEMPTY for rmdir and EISDIR for unlink before calling it.
- SETATTR: Editors use ftruncate, and I only have truncate_to_zero. I need truncate to an arbitrary size, 
    which both shrinks and extends, plus chmod, chown and utimens.
- readdir: FUSE passes an offset cookie. Using the index into read_dir vec works. Each fuse_dirent is padded to 8 bytes,
    and the type comes from mode >> 12.
- Generation numbers: reserve_inode sets i_generation = 0. Make it increment on reuse, because the kernel pairs 
    (nodeid, generation) to detect a recycled inode.
- Error mapping: NameNotFound → ENOENT, NameExists → EEXIST, NotDirectory → ENOTDIR, NotFile → EISDIR, NoMoreBlocks 
    and NoInodes → ENOSPC, PermissionDenied → EACCES, InvalidFlags → EINVAL, Overflow → ENAMETOOLONG for names, 
    and the corruption and I/O variants → EIO

I added a stat field. It uses raw inode number. Damn. Maybe I should have started with inode 1. But eh. I'll just
make the driver layer increment it.The driver fills in the rest of fuse_attr itself: blksize = 4096, rdev = 0, 
and the nanosecond fields as 0, since I only store seconds. Also my uid and gid are u16 but u32 are accepted? Idk
what to do about that. That'll mean changing the inode size. I'll look into it but I marked it here. Added lookup too.

Ok so I first added more functions as an impl to Filesystem and then widened uid and gid to u32. Then, I added read_at
and shortened File::read. Also changed errors. Overflow was overloaded so it is split into name too long and eoverflow.

from /usr/include/linux/fuse.h

struct fuse_in_header {
	uint32_t	len;
	uint32_t	opcode;
	uint64_t	unique;
	uint64_t	nodeid;
	uint32_t	uid;
	uint32_t	gid;
	uint32_t	pid;
	uint16_t	total_extlen; /* length of extensions in 8byte units */
	uint16_t	padding;
};

struct fuse_out_header {
	uint32_t	len;
	int32_t		error;
	uint64_t	unique;
};

I had to use fuse_init_in and fuse_init_out too. So when I first get a fuse fd, I read a 104 byte buffer from the kernel
through the socket fd. 40 byte header. rest is fuse_init_in. My kernel is using 7.41 but I'll set fuse_init_out to
negotiate 7.31 to not claim to support new features.

262:struct fuse_attr {
263-	uint64_t	ino;
264-	uint64_t	size;
265-	uint64_t	blocks;
266-	uint64_t	atime;
267-	uint64_t	mtime;
268-	uint64_t	ctime;
269-	uint32_t	atimensec;
270-	uint32_t	mtimensec;
271-	uint32_t	ctimensec;
272-	uint32_t	mode;
273-	uint32_t	nlink;
274-	uint32_t	uid;
275-	uint32_t	gid;
276-	uint32_t	rdev;
277-	uint32_t	blksize;
278-	uint32_t	flags;
279-};

687-struct fuse_getattr_in {
688-	uint32_t	getattr_flags;
689-	uint32_t	dummy;
690-	uint64_t	fh;
691-};
692-
693-#define FUSE_COMPAT_ATTR_OUT_SIZE 96
694-
695:struct fuse_attr_out {
696-	uint64_t	attr_valid;	/* Cache timeout for the attributes */
697-	uint32_t	attr_valid_nsec;
698-	uint32_t	dummy;
699:	struct fuse_attr attr;
700-};

662:struct fuse_entry_out {
663-	uint64_t	nodeid;		/* Inode ID */
664-	uint64_t	generation;	/* Inode generation: nodeid:gen must
665-					   be unique for the fs's lifetime */
666-	uint64_t	entry_valid;	/* Cache timeout for the name */
667-	uint64_t	attr_valid;	/* Cache timeout for the attributes */
668-	uint32_t	entry_valid_nsec;
669-	uint32_t	attr_valid_nsec;
670-	struct fuse_attr attr;
671-};

These are structs from my linux's (Debian 13.5) fuse.h. the numbers there are line numbers. there will be mode. but
anyways, I cleaned up the driver a lot. There's a main loop that dispatches a request, parses FileError to an error
number. libc is really cool.

FUCK my lookup is returning ENOENT for some reason. Ah fuck me my opcode map was wrong. 27 is open dir and 28 is read
dir. Anyways, cd and ls are working now.

Ok so following adding opcode 15 for reading a file, cat works and nvim cat work with that disk!!! I literally ran
the driver, cd into the mounted disk, ls, cat, nested cd and then opened nvim and checked files. As write isn't done
yet it isn't concrete but read only operations work.

Waaah! I can't create files but can write them but the fucking premade disk is read only. I'll delete it and make a 
blank one eventually. It'll be cool. So it's mostly just finding an opcode and writing it. But for some reason the
protected file secrets.txt doesn't let me open it as SU either? in fact the whole fucking disk locks me out. It's 
supposed to be the other way around. Did I just make an anarchist disk? No. I probably just shat something out when
populating the disk. I'll look into it once I can make touch work (create file).

One fucking thing after another. root was hard coded as a wrong value. hence the permission crap. but now I still
can't write to a file because before opcode 16, it calls opcode 4 to set attr.

ugh the fucking shell handles recursive delete. lol I gracefully implemented it with inodes and it turns out some 
little shit's shell just does that. Anyways, now opcode 4 wasn't working for an hour because I was trying to do
echo hi > test.txt, which was a stale inode from an old disk version that I couldn't delete. But I'm doing deletion
now. Unlink, I should say. rmdir is next as opcode 11.

OH FUCK NOW DELETION IS WORKING AND FUCKING LOOK AT WHAT I JUST DID THIS BEAUTIFUL CHILD OF MINE, MY FILESYSTEM RAN ITS FIRST C PROGRAM AND EXECUTED USING GCC!.

#include <stdio.h>

int main(){
	printf("This is Sohum's first C code on this disk!!!\n");
	printf("BANZAI !!!");
	return 0;
}

It actually just ran code. It deleted a directory but since opcode 42 does not exist and needs no response but I didn't
put it in there, it didn't work. The driver panicked. Now that I fixed it, it turns out that rmdir can delete even
full directories. I'll look into it. I imagined the BANZAI in Mr. Miyagi's voice. This disk will be pushed eventually
as the first ever disk on this system that worked.

I tried git init but that needs opcode 12 for renaming and oops I forgot to implement renaming a dirent.
HOLY SHIT. I implemented rename. Opcodes 21, 22, 23, 24 for XATTR aren't there but I was able to make a git repo
INSIDE MY FILESYSTEM, stage files and commit in there. mv works too.
