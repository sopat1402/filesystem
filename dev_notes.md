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
can look at extent_tree.rs. Until then, cry me a river, you can't have 4+GB files on a 50MiB disk.

Ugh read is where this stuff stops being cute and wipes its makeup off to reveal a fat programmer. Binary search first
to find the lower extent. Then have to find the higher extent based on length. Then, I have to read the extents, put
them into a buffer and based on that I have to trim it as needed and then return it.
fuck me. the fucking range lookup. why the fuck was I searching the whole thing and filtering it myself? Anyways, I used
range lookup and massively cleaned up my code. Before I was doing 0 to u32::MAX without thinking about it and was then
binary searching on it lmfao.
