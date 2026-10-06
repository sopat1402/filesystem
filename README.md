# Filesystem

A custom block-based filesystem written from scratch in Rust, backed by disk images and mounted in user space through the Linux FUSE interface.

The project is a complete Cargo crate. The filesystem implementation is provided as a library, while the FUSE driver and disk-population utility are standalone binaries under `src/bin`.

After roughly a month of development, the project has progressed from an on-disk format experiment into a functional filesystem that can be formatted, mounted, browsed, modified, and used by ordinary Linux programs. It has been manually tested with shell utilities, `nvim`, Git, GCC, and compiled C programs running directly from the mounted filesystem.

The implementation intentionally avoids using a Rust FUSE crate. The FUSE driver communicates directly with the Linux FUSE interface and translates requests into operations on the filesystem library.

The project is still experimental. **Journaling, crash recovery, and concurrency are not implemented.** The filesystem should therefore be treated as a development and learning project rather than a storage system for important data. Use disposable disk images and keep anything valuable somewhere else.

---

## Architecture

The filesystem is organized into several layers:

```text
                    Linux applications
                           │
                           ▼
                     Linux VFS
                           │
                           ▼
                  FUSE kernel interface
                           │
                           ▼
                src/bin/fuse_driver.rs
                           │
                           ▼
                    Filesystem API
                           │
             ┌─────────────┴─────────────┐
             │                           │
          Inodes                     Directories
             │                           │
             └─────────────┬─────────────┘
                           │
                           ▼
                     Extent trees
                           │
                           ▼
                      BlockCache
                           │
                           ▼
                       Disk image
```

The filesystem library is responsible for the on-disk structures and filesystem semantics. The FUSE driver provides the Linux-facing interface, while `BlockCache` provides the common boundary between filesystem structures and persistent storage.

The cache is deliberately below the inode, directory, file, bitmap, and extent-tree layers. Filesystem code therefore does not perform ad-hoc block reads and writes directly against the disk image.

---

## What is implemented

### Disk images and on-disk format

`create_disk` formats a block-aligned disk image containing:

* a superblock
* an inode bitmap
* a block bitmap
* an inode table
* filesystem data blocks
* a root directory

The image size and bytes-per-inode ratio are supplied when formatting. The resulting metadata layout is recorded in the superblock rather than being based on hard-coded disk offsets.

This allows the same filesystem implementation to operate on different image sizes and inode densities.

Blocks are 4 KiB. Every block contains a 14-byte header containing:

* a log sequence number
* a CRC32 checksum
* a block state flag

The state flag currently supports:

```text
Clean
Dirty
Recoverable
Irrecoverable
```

The checksum is validated when a block is read and can detect corrupted block contents. It is not a replacement for journaling, transaction processing, or crash recovery.

The block metadata and state flags are designed with future write-ahead logging in mind, but journaling is not currently implemented.

### Block cache

`BlockCache` provides a write-back LRU cache for filesystem blocks.

The default cache contains 256 entries, with each filesystem block occupying 4 KiB. This gives a default cache size of approximately 1 MiB.

The cache:

* stores regular filesystem blocks and the superblock
* maintains an LRU ordering
* provides O(1)-ish lookup through a hash map
* marks blocks dirty when mutable access is requested
* writes dirty blocks back when they are evicted
* provides an explicit `flush` operation
* synchronizes the underlying disk image during `flush`

Dropping the cache attempts to flush dirty data, although errors during `Drop` cannot be reported to the caller.

All filesystem layers use `BlockCache` for block access. This gives the filesystem a single storage boundary through which block serialization, checksumming, dirty tracking, caching, and writeback are performed.

The cache is currently single-threaded and is not itself a concurrency mechanism.

### Allocation and bitmaps

The filesystem maintains separate bitmaps for inodes and physical blocks.

The bitmap locations and their valid ranges are determined from the superblock. Allocation scans the relevant bitmap for free entries and marks allocated objects accordingly.

Free physical blocks are grouped into contiguous runs and returned as `Extent` structures where possible.

The filesystem also maintains free inode and free block counters in the superblock.

The current allocator is intentionally straightforward. It prioritizes correctness and simplicity rather than implementing advanced locality or fragmentation-reduction strategies.

### Extent trees

Files and directories use extent trees to map logical blocks to physical disk blocks.

An extent represents a contiguous range of physical blocks associated with a contiguous logical range:

```text
logical blocks:   0  1  2  3  4
                  │  │  │  │  │
physical blocks: 50 51 52 53 54
                  └───────────┘
                     extent
```

The root of an extent tree is stored directly in the inode. Larger trees use additional filesystem blocks for internal and leaf nodes.

The extent tree supports:

* lookup of logical ranges
* insertion
* deletion
* splitting nodes when they become full
* merging adjacent extents
* trimming ranges
* reclaiming blocks belonging to removed tree nodes

When both logical and physical ranges are contiguous, extents can be merged rather than represented as separate entries.

This allows files to use contiguous physical allocation without requiring a block-by-block mapping for every file.

The extent tree was implemented as one of the core storage structures of the filesystem and is used by both regular files and directories.

### Inodes

Inodes are fixed-size 256-byte records.

An inode stores information including:

* file type
* Unix permission bits
* UID and GID
* file size
* timestamps
* link count
* block count
* generation number
* extent-tree root

Inode numbers are zero-based internally. In particular, inode `0` is the filesystem root.

The `inode` module handles inode serialization, validation, lookup, and conversion to filesystem `stat` information.

### Directories

Directories are stored as ordinary filesystem data and use the extent-tree mechanism just like files.

Each directory entry contains:

```text
name length
UTF-8 name
inode number
```

Directory names are limited to 255 bytes.

The directory layer provides:

* directory reading
* name lookup
* path resolution
* directory creation
* entry insertion
* entry deletion
* recursive directory deletion
* rename operations
* `.` and `..` handling
* directory link-count updates

Path resolution supports both absolute and relative paths.

Directory deletion recursively removes child entries and releases their associated data blocks and inode allocations.

Rename handling includes checks for invalid names, file/directory type mismatches, non-empty destination directories, and attempts to move a directory into one of its own descendants.

### Files and file I/O

The `files` module provides a path-based `File` abstraction with:

* open
* read
* write
* seek
* tell

It also exposes inode-and-offset based operations used by the FUSE layer.

Writes can:

* overwrite existing data
* extend a file
* append data
* allocate additional physical blocks
* create sparse regions

Unwritten gaps created by seeking beyond the current end of a file read back as zeroes.

File data is mapped through extent trees, and newly required blocks are obtained through the bitmap allocator and inserted into the inode's extent tree.

Basic Unix permission checks are implemented using:

* owner permissions
* group permissions
* supplementary groups
* other permissions

A root-user rule is also implemented.

The current truncate implementation supports reducing a file to zero bytes. Arbitrary-size truncation is not yet implemented.

### Filesystem interface

The `filesystem` module ties the lower-level components together.

It handles opening and creating filesystem images and provides the higher-level operations consumed by the FUSE driver, including:

* `stat`
* `lookup`
* `read_dir`
* `read_at`
* `write_at`

The filesystem layer hides most of the details of the on-disk layout from the FUSE implementation.

---

## FUSE driver

`src/bin/fuse_driver.rs` implements the Linux FUSE userspace interface directly using `libc`.

It does **not** depend on a FUSE Rust crate.

The driver:

* obtains a FUSE file descriptor through the Linux FUSE interface
* reads raw FUSE requests
* parses FUSE request headers and bodies
* dispatches requests by opcode
* translates FUSE node IDs to internal inode numbers
* builds FUSE response structures
* maps filesystem errors to Linux error numbers
* communicates with the mounted filesystem through the filesystem library

The filesystem's root inode is internally numbered `0`, while FUSE exposes the root as node ID `1`. The driver performs this translation at the FUSE boundary.

The implemented request subset includes practical operations for normal filesystem use:

* `LOOKUP`
* `GETATTR`
* file opening
* file reading
* file writing
* directory opening
* `READDIR`
* directory release
* file creation
* directory creation
* unlink
* directory removal
* rename
* attribute updates
* file release

The driver is intentionally incomplete with respect to the entire FUSE and POSIX interfaces. Unsupported operations are not expected to behave like a complete production filesystem.

Nevertheless, the implemented subset is sufficient for substantial real-world interaction from normal Linux userspace.

The filesystem has been tested with tools and applications including shell commands, `nvim`, Git, GCC, and compiled programs.

---

## Test disk population

`src/bin/populate_disk.rs` creates a deterministic test image and a matching host-side reference tree.

By default it creates:

```text
code.img
code-reference/
```

using a 128 MiB filesystem image and the current user's UID and GID.

The generated fixture includes:

* nested directories
* workspace-style directory structures
* configuration files
* ordinary files
* empty files
* permission examples
* data spanning multiple blocks
* a one-megabyte patterned file
* a sparse file
* a Unicode filename
* a 255-byte filename
* a directory containing 300 entries

The host-side reference tree can be used to compare expected contents and metadata against the mounted filesystem.

The utility refuses to overwrite an existing image or reference directory, making accidental destruction of previous test fixtures less likely.

---

## Build and run

The project requires a Linux environment with Rust/Cargo and FUSE support.

Run the test suite:

```bash
cargo test
```

Create a fresh test image and reference tree:

```bash
cargo run --bin populate_disk
```

The population utility accepts optional arguments in this order:

```text
populate_disk [image-path] [reference-directory] [uid] [gid]
```

For example:

```bash
cargo run --bin populate_disk -- test.img test-reference
```

Remove or rename existing output paths before running the utility again.

### Mounting

Create a mount point:

```bash
mkdir -p /mnt/fs-lab
```

Start the FUSE driver:

```bash
cargo run --bin fuse_driver -- code.img /mnt/fs-lab
```

The system must provide Linux FUSE support, access to `/dev/fuse`, and `fusermount3`.

Once mounted, the filesystem can be accessed normally:

```bash
ls /mnt/fs-lab
cd /mnt/fs-lab
cat some-file
```

Files can also be edited using applications such as `nvim`, and programs stored on the filesystem can be compiled and executed normally.

For example, a C source file stored on the filesystem can be compiled with GCC and the resulting executable can be run directly from the mounted filesystem.

Unmount with:

```bash
fusermount3 -u /mnt/fs-lab
```

---

## Development and testing

The project has been developed incrementally, with the filesystem being tested against increasingly realistic workloads as functionality was added.

Testing has included:

* creating and removing files
* creating and removing directories
* recursive directory deletion
* renaming and moving files
* copying files
* changing permissions
* editing files with `nvim`
* compiling C programs stored on the filesystem
* executing compiled binaries from the mounted filesystem
* initializing Git repositories
* staging and committing files inside the filesystem
* storing the filesystem's own source tree inside a filesystem image

The latter is particularly useful as an integration test: the filesystem is capable of storing the source code of the filesystem implementation itself.

The project also maintains a Git history documenting the development of the storage format, inode layer, extent tree, directory implementation, FUSE interface, file operations, and later cache integration.

---

## Design priorities

The implementation has prioritized getting a complete end-to-end system working before optimizing or implementing every filesystem feature.

The major design priorities so far have been:

1. Define a real on-disk format.
2. Implement persistent block and metadata management.
3. Build a usable inode and directory hierarchy.
4. Implement extent-based file storage.
5. Expose the filesystem through Linux FUSE.
6. Make ordinary Linux programs capable of using it.
7. Introduce a centralized write-back block cache.
8. Keep the implementation understandable enough to continue extending.

This means the filesystem deliberately does not attempt to reproduce every feature or optimization of mature filesystems such as ext4.

The project is intended to provide a complete systems-level implementation that can be inspected, modified, and extended rather than a production replacement for an existing Linux filesystem.

---

## Known gaps

The following are currently known limitations.

### No journaling

There is no write-ahead log or journal.

Filesystem operations may involve multiple metadata updates. If the process or machine stops partway through such an operation, the image can potentially be left in an inconsistent state.

The block header already contains state information intended to support future recovery mechanisms, but no recovery system currently uses it.

### No crash recovery

There is no filesystem consistency checker or crash-recovery procedure.

A corrupted or partially updated image should not be considered recoverable merely because individual blocks contain CRC32 checksums.

The checksum can detect corrupted contents; it does not reconstruct lost metadata or roll back incomplete filesystem operations.

### No concurrency

The filesystem currently assumes serialized access.

There is no locking scheme protecting the filesystem from simultaneous mutations, and the current `BlockCache` is not designed for concurrent access.

Do not access the same image concurrently from multiple processes or mounts.

### No operation-level atomicity

Multi-step operations are not transactional.

For example, a filesystem operation may modify directory entries, inodes, extent trees, bitmaps, and superblock counters as separate updates. Without journaling, a failure between those updates can leave only part of the operation persisted.

### Incomplete FUSE/POSIX interface

The FUSE driver implements a practical subset of filesystem operations rather than the complete Linux filesystem interface.

Many less-common operations and features are not implemented, including extended attributes and other advanced metadata interfaces.

### Limited truncation

The current implementation can truncate a file to zero bytes but does not yet support arbitrary-size truncation.

### Simple allocation policy

Block allocation currently prioritizes simplicity and contiguous free-space discovery rather than sophisticated locality and fragmentation heuristics.

More advanced allocation strategies can be added later.

---

## Future work

The largest remaining systems-level features are:

* concurrent access and synchronization
* journaling / write-ahead logging
* crash recovery
* stronger filesystem consistency guarantees
* more sophisticated block allocation and locality
* broader FUSE/POSIX API coverage
* improved handling of edge cases
* further performance testing and profiling
* eventually, potentially, a kernel-side implementation

These are deliberately left as future work rather than being presented as features already provided by the current implementation.

---

## Current status

**Functional experimental filesystem.**

The project currently provides a persistent block-based filesystem with:

```text
✓ Custom on-disk format
✓ 4 KiB blocks
✓ Superblock
✓ CRC32 block checksums
✓ Inode bitmap
✓ Block bitmap
✓ Inode table
✓ Extent trees
✓ File storage
✓ Directory hierarchy
✓ Path resolution
✓ File creation/deletion
✓ Directory creation/deletion
✓ Rename/move
✓ File read/write
✓ Sparse files
✓ Unix permissions
✓ Block caching
✓ Write-back cache
✓ FUSE userspace driver
✓ Linux mounting
✓ Real userspace applications
✓ Git repositories inside the filesystem
✓ Executable programs stored and run from the filesystem

✗ Journaling
✗ Crash recovery
✗ Concurrent access
✗ Complete POSIX/FUSE interface
✗ Arbitrary-size truncation
```

The important milestone for the project is not that every filesystem feature has been implemented. It is that the complete path from **Linux userspace to a custom persistent storage implementation** works.

This started as an attempt to build a filesystem.

It is now one.
