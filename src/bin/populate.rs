use filesystem::constants::*;
use filesystem::directories::{add_dirent, make_dir, resolve_path};
use filesystem::file_errors::FileError;
use filesystem::files::{Cred, File as FsFile};
use filesystem::filesystem::{create_disk, Filesystem};
use filesystem::inode::{find_inode, write_inode};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const ROOT: u64 = ROOT_INODE_NUM as u64;
const DISK_SIZE: u64 = 128 * 1024 * 1024;
const INODE_RATIO: u64 = 16 * 1024;
const PAYLOAD: usize = BLOCK_SIZE - BLOCK_HEADER_SIZE;

fn pattern(len: usize, seed: u32) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u32).wrapping_mul(31).wrapping_add(seed) % 251) as u8)
        .collect()
}

fn split(path: &str) -> (&str, &str) {
    let (parent, name) = path.rsplit_once('/').expect("absolute path required");
    (if parent.is_empty() { "/" } else { parent }, name)
}

fn set_root_owner(fs: &mut Filesystem, uid: u32, gid: u32) -> Result<(), FileError> {
    let mut root = find_inode(&mut fs.block_cache, ROOT)?;
    root.i_uid = uid;
    root.i_gid = gid;
    root.i_mode = S_IFDIR | 0o755;
    write_inode(&mut fs.block_cache, ROOT, &root.serialise())
}

struct Ctx<'a> {
    fs: &'a mut Filesystem,
    out: &'a Path,
    uid: u32,
    gid: u32,
}

impl Ctx<'_> {
    fn host(&self, path: &str) -> PathBuf {
        self.out.join(path.trim_start_matches('/'))
    }

    fn set_host_mode(&self, path: &str, mode: u16) {
        std::fs::set_permissions(
            self.host(path),
            std::fs::Permissions::from_mode(mode as u32),
        )
        .expect("set reference permissions");
    }

    fn parent_inode(&mut self, path: &str) -> Result<(u64, String), FileError> {
        let (parent, name) = split(path);
        let parent_inode =
            resolve_path(&mut self.fs.block_cache, parent.to_string(), ROOT)?;
        Ok((parent_inode, name.to_string()))
    }

    fn mkdir(&mut self, path: &str, mode: u16) -> Result<(), FileError> {
        let (parent, name) = self.parent_inode(path)?;

        make_dir(
            &mut self.fs.block_cache,
            parent,
            name,
            self.uid,
            self.gid,
            mode,
        )?;

        std::fs::create_dir_all(self.host(path)).expect("create reference directory");
        self.set_host_mode(path, mode);
        Ok(())
    }

    fn create(&mut self, path: &str, mode: u16) -> Result<(), FileError> {
        let (parent, name) = self.parent_inode(path)?;

        add_dirent(
            &mut self.fs.block_cache,
            parent,
            name,
            S_IFREG | mode,
            self.uid,
            self.gid,
        )?;

        Ok(())
    }

    fn open_for_write(&mut self, path: &str) -> Result<FsFile<'_>, FileError> {
        let cred = Cred {
            uid: self.uid,
            gid: self.gid,
            groups: Vec::new(),
        };

        FsFile::open(
            self.fs,
            &path.to_string(),
            ROOT,
            O_WRONLY,
            &cred,
        )
    }

    fn put(&mut self, path: &str, mode: u16, data: &[u8]) -> Result<(), FileError> {
        self.create(path, mode)?;

        if !data.is_empty() {
            let mut file = self.open_for_write(path)?;

            for chunk in data.chunks(64 * 1024) {
                assert_eq!(file.write(chunk)?, chunk.len(), "short write on {path}");
            }

            drop(file);
        }

        std::fs::write(self.host(path), data).expect("write reference file");
        self.set_host_mode(path, mode);
        Ok(())
    }

    fn put_sparse(&mut self, path: &str, hole: usize, tail: &[u8]) -> Result<(), FileError> {
        self.create(path, 0o644)?;

        let mut file = self.open_for_write(path)?;
        file.fseek(hole as u64)?;
        assert_eq!(file.write(tail)?, tail.len());
        drop(file);

        let mut reference = vec![0u8; hole];
        reference.extend_from_slice(tail);
        std::fs::write(self.host(path), reference).expect("write sparse reference file");
        self.set_host_mode(path, 0o644);
        Ok(())
    }
}

fn main() -> Result<(), FileError> {
    let args: Vec<String> = std::env::args().collect();
    let image = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "code.img".to_string());
    let out = PathBuf::from(
        args.get(2)
            .cloned()
            .unwrap_or_else(|| "code-reference".to_string()),
    );
    let uid = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getuid() });
    let gid = args
        .get(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getgid() });

    if Path::new(&image).exists() || out.exists() {
        return Err(FileError::NameExists);
    }

    std::fs::create_dir_all(&out).expect("create reference directory");
    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))
        .expect("set reference root permissions");

    create_disk(&image, DISK_SIZE, INODE_RATIO)?;

    let mut fs = Filesystem::open(image.clone())?;
    set_root_owner(&mut fs, uid, gid)?;

    let mut ctx = Ctx {
        fs: &mut fs,
        out: &out,
        uid,
        gid,
    };

    ctx.mkdir("/etc", 0o755)?;
    ctx.mkdir("/tmp", 0o1777)?;
    ctx.mkdir("/home", 0o755)?;
    ctx.mkdir("/home/dev", 0o750)?;
    ctx.mkdir("/home/dev/.config", 0o700)?;
    ctx.mkdir("/home/dev/.config/fs-lab", 0o700)?;
    ctx.mkdir("/home/dev/workspace", 0o750)?;
    ctx.mkdir("/home/dev/workspace/src", 0o750)?;
    ctx.mkdir("/home/dev/workspace/tests", 0o750)?;
    ctx.mkdir("/home/dev/workspace/docs", 0o750)?;
    ctx.mkdir("/home/dev/workspace/config", 0o750)?;
    ctx.mkdir("/home/dev/workspace/data", 0o750)?;
    ctx.mkdir("/home/dev/workspace/logs", 0o750)?;
    ctx.mkdir("/var", 0o755)?;
    ctx.mkdir("/var/log", 0o755)?;
    ctx.mkdir("/var/lib", 0o755)?;
    ctx.mkdir("/var/lib/fs-lab", 0o755)?;
    ctx.mkdir("/var/lib/fs-lab/fixtures", 0o755)?;
    ctx.mkdir("/var/lib/fs-lab/fixtures/many", 0o755)?;

    ctx.put(
        "/README.md",
        0o644,
        b"Development filesystem image\n\nWritable workspace: /home/dev/workspace\nBoundary and directory fixtures: /var/lib/fs-lab/fixtures\nTemporary files: /tmp\n",
    )?;
    ctx.put("/etc/hostname", 0o644, b"fs-lab\n")?;
    ctx.put(
        "/etc/fs-lab.conf",
        0o644,
        b"mountpoint = \"/mnt/fs-lab\"\nlog_level = \"info\"\n",
    )?;
    ctx.put(
        "/home/dev/.config/fs-lab/config.toml",
        0o600,
        b"mountpoint = \"/mnt/fs-lab\"\nlog_level = \"debug\"\n",
    )?;
    ctx.put(
        "/home/dev/workspace/README.md",
        0o644,
        b"# Filesystem Lab\n\nA small Rust workspace for experimenting with storage and FUSE.\n",
    )?;
    ctx.put(
        "/home/dev/workspace/Cargo.toml",
        0o644,
        b"[package]\nname = \"fs-playground\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    )?;
    ctx.put("/home/dev/workspace/.gitignore", 0o644, b"/target\n*.img\n")?;
    ctx.put(
        "/home/dev/workspace/src/main.rs",
        0o644,
        b"fn main() {\n    println!(\"filesystem lab ready\");\n}\n",
    )?;
    ctx.put(
        "/home/dev/workspace/src/lib.rs",
        0o644,
        b"pub fn describe_size(bytes: usize) -> String {\n    format!(\"{bytes} bytes\")\n}\n",
    )?;
    ctx.put(
        "/home/dev/workspace/tests/roundtrip.rs",
        0o644,
        b"#[test]\nfn empty_payload_has_zero_length() {\n    assert_eq!(Vec::<u8>::new().len(), 0);\n}\n",
    )?;
    ctx.put(
        "/home/dev/workspace/docs/layout.md",
        0o644,
        b"# Image layout\n\nThe image separates user workspace, configuration, logs, and filesystem test fixtures.\n",
    )?;
    ctx.put(
        "/home/dev/workspace/config/fs-lab.toml",
        0o640,
        b"[filesystem]\nblock_size = 4096\nread_only = false\n",
    )?;
    ctx.put(
        "/home/dev/workspace/data/records.csv",
        0o644,
        b"id,name,state\n1,alpha,ready\n2,beta,queued\n3,gamma,complete\n",
    )?;
    ctx.put(
        "/home/dev/workspace/logs/session.log",
        0o640,
        b"INFO mount initialized\nINFO root inode loaded\n",
    )?;
    ctx.put(
        "/var/log/fs-lab.log",
        0o640,
        b"INFO image opened\nINFO metadata checks passed\n",
    )?;
    ctx.put(
        "/var/lib/fs-lab/fixtures/hello.txt",
        0o644,
        b"hello from fs-lab\n",
    )?;
    ctx.put("/var/lib/fs-lab/fixtures/empty", 0o644, b"")?;
    ctx.put(
        "/var/lib/fs-lab/fixtures/private.txt",
        0o600,
        b"owner-only fixture\n",
    )?;
    ctx.put(
        "/var/lib/fs-lab/fixtures/one-block.bin",
        0o644,
        &pattern(PAYLOAD, 1),
    )?;
    ctx.put(
        "/var/lib/fs-lab/fixtures/cross-block.bin",
        0o644,
        &pattern(3 * PAYLOAD + 123, 2),
    )?;
    ctx.put(
        "/var/lib/fs-lab/fixtures/one-megabyte.bin",
        0o644,
        &pattern(1 << 20, 3),
    )?;
    ctx.put_sparse(
        "/var/lib/fs-lab/fixtures/sparse.bin",
        3 * PAYLOAD + 10,
        b"tail",
    )?;
    ctx.put(
        "/var/lib/fs-lab/fixtures/résumé.txt",
        0o644,
        "unicode filename\n".as_bytes(),
    )?;
    ctx.put(
        &format!(
            "/var/lib/fs-lab/fixtures/{}",
            "x".repeat(255)
        ),
        0o644,
        b"255-byte filename\n",
    )?;

    for i in 0..300 {
        ctx.put(
            &format!("/var/lib/fs-lab/fixtures/many/entry-{i:04}"),
            0o644,
            b"",
        )?;
    }

    println!("image: {image}");
    println!("reference tree: {}", out.display());
    println!("owner: uid {uid} gid {gid}");
    Ok(())
}
