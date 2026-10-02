// Builds a test image with known contents and writes an identical reference tree
// to a host directory, so a mounted copy can be checked with `diff -r`.
//
// usage: cargo run --release --bin populate -- [image] [reference_dir] [uid] [gid]
//        defaults: test.img expected 0 0

use filesystem::constants::*;
use filesystem::directories::{add_dirent, make_dir, resolve_path};
use filesystem::file_errors::FileError;
use filesystem::files::{Cred, File as FsFile};
use filesystem::filesystem::{create_disk, Filesystem};
use std::path::{Path, PathBuf};

const ROOT: u64 = ROOT_INODE_NUM as u64;
const DISK_SIZE: u64 = 32 * 1024 * 1024;
const INODE_RATIO: u64 = 16 * 1024;
const PAYLOAD: usize = BLOCK_SIZE - BLOCK_HEADER_SIZE;

/// Deterministic bytes. The period (251) doesn't divide the block payload (4082),
/// so a wrong offset anywhere shows up as a mismatch instead of lining up by luck.
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u32).wrapping_mul(31).wrapping_add(seed) % 251) as u8)
        .collect()
}

fn root_cred() -> Cred {
    Cred { uid: 0, gid: 0, groups: Vec::new() }
}

/// "/a/b/c" -> ("/a/b", "c"), "/c" -> ("/", "c")
fn split(path: &str) -> (&str, &str) {
    let (parent, name) = path.rsplit_once('/').expect("paths must be absolute");
    (if parent.is_empty() { "/" } else { parent }, name)
}

struct Ctx<'a> {
    fs: &'a Filesystem,
    out: &'a Path,
    uid: u32,
    gid: u32,
}

impl Ctx<'_> {
    fn host(&self, path: &str) -> PathBuf {
        self.out.join(path.trim_start_matches('/'))
    }

    fn parent_inode(&self, path: &str) -> Result<(u64, String), FileError> {
        let (parent, name) = split(path);
        let inode = resolve_path(&self.fs.disk, parent.to_string(), ROOT)?;
        Ok((inode, name.to_string()))
    }

    fn mkdir(&self, path: &str, perm: u16) -> Result<(), FileError> {
        let (parent, name) = self.parent_inode(path)?;
        make_dir(&self.fs.disk, parent, name, self.uid, self.gid, perm)?;
        std::fs::create_dir_all(self.host(path)).expect("create reference dir");
        Ok(())
    }

    /// Create the dirent with the requested owner and mode, no data yet.
    fn create(&self, path: &str, mode: u16) -> Result<(), FileError> {
        let (parent, name) = self.parent_inode(path)?;
        add_dirent(&self.fs.disk, parent, name, S_IFREG | mode, self.uid, self.gid)?;
        Ok(())
    }

    /// Data is written as root so the file's own mode (even 0o000) can't block us.
    fn open_for_write(&self, path: &str) -> Result<FsFile<'_>, FileError> {
        FsFile::open(self.fs, &path.to_string(), ROOT, O_WRONLY, &root_cred())
    }

    fn put(&self, path: &str, mode: u16, data: &[u8]) -> Result<(), FileError> {
        self.create(path, mode)?;
        if !data.is_empty() {
            let mut f = self.open_for_write(path)?;
            for chunk in data.chunks(64 * 1024) {
                assert_eq!(f.write(chunk)?, chunk.len(), "short write on {path}");
            }
        }
        std::fs::write(self.host(path), data).expect("write reference file");
        Ok(())
    }

    /// A real hole: seek past EOF and write, so the gap has no blocks behind it.
    fn put_sparse(&self, path: &str, hole: usize, tail: &[u8]) -> Result<(), FileError> {
        self.create(path, 0o644)?;
        let mut f = self.open_for_write(path)?;
        f.fseek(hole as u64)?;
        assert_eq!(f.write(tail)?, tail.len());
        let mut reference = vec![0u8; hole];
        reference.extend_from_slice(tail);
        std::fs::write(self.host(path), reference).expect("write reference file");
        Ok(())
    }
}

fn main() -> Result<(), FileError> {
    let args: Vec<String> = std::env::args().collect();
    let image = args.get(1).cloned().unwrap_or_else(|| "test.img".to_string());
    let out = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "expected".to_string()));
    let uid: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let gid: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);

    let _ = std::fs::remove_file(&image);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).expect("create reference dir");

    create_disk(&image, DISK_SIZE, INODE_RATIO)?;
    let fs = Filesystem::open(image.clone())?;
    let ctx = Ctx { fs: &fs, out: &out, uid, gid };

    // Small and trivial files
    ctx.put("/hello.txt", 0o644, b"hello from the filesystem\n")?;
    ctx.put("/empty", 0o644, b"")?;
    ctx.put("/secret.txt", 0o600, b"owner only\n")?;

    // Block-boundary shapes: exactly one block, a few blocks plus a partial, and 1 MiB
    ctx.put("/one-block.bin", 0o644, &pattern(PAYLOAD, 1))?;
    ctx.put("/pattern.bin", 0o644, &pattern(3 * PAYLOAD + 123, 2))?;
    ctx.put("/big.bin", 0o644, &pattern(1 << 20, 3))?;

    // Hole spanning several unallocated blocks, then a few bytes
    ctx.put_sparse("/sparse.bin", 3 * PAYLOAD + 10, b"tail")?;

    // Awkward names
    ctx.put("/naïve café.txt", 0o644, "unicode name\n".as_bytes())?;

    // Nested directories
    ctx.mkdir("/docs", 0o755)?;
    ctx.put("/docs/readme.txt", 0o644, b"documentation lives here\n")?;
    ctx.mkdir("/docs/nested", 0o755)?;
    ctx.put("/docs/nested/deep.bin", 0o644, &pattern(2 * PAYLOAD, 4))?;

    // 300 entries: the directory spills into a second block, which exercises
    // READDIR paging and offsets
    ctx.mkdir("/many", 0o755)?;
    for i in 0..300 {
        ctx.put(&format!("/many/f-{i:04}"), 0o644, b"")?;
    }

    println!("image: {image}");
    println!("reference tree: {}", out.display());
    println!("owner of everything except / : uid {uid} gid {gid}");
    Ok(())
}
